//! An album's tracks fetched one at a time, each on its own: a track a
//! source does not have is left out and the rest go on; a source that
//! asks to wait is waited for (a while, longer each time) and asked again;
//! a source that turns us away stops the album there, without asking it
//! for every track left. What came is kept ([`Fetched::Partial`]), so the
//! rest can come later or from another source.

use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;

use nikgob_core::source::{Fetched, SourceError};

use crate::text::title_keys;

/// How long a fetch waits for a source that asked to wait.
#[derive(Clone, Copy, Debug)]
pub struct Patience {
    /// The first wait, when the source did not say how long.
    pub first: Duration,
    /// A wait longer than this is not waited out: the album stops there
    /// and its rest is asked for later.
    pub longest: Duration,
    /// Asks again of one track at most.
    pub tries: u32,
}

impl Default for Patience {
    fn default() -> Self {
        Self {
            first: Duration::from_secs(5),
            longest: Duration::from_secs(120),
            tries: 3,
        }
    }
}

/// The `(id, file name)` of the tracks whose title is not among `got`
/// (the titles of the tracks that have a file already).
pub fn left_out<T>(tracks: Vec<(T, String, String)>, got: &[String]) -> Vec<(T, String)> {
    let have: Vec<String> = got.iter().flat_map(|g| title_keys(g)).collect();
    tracks
        .into_iter()
        .filter(|(_, _, title)| !title_keys(title).iter().any(|k| have.contains(k)))
        .map(|(id, name, _)| (id, name))
        .collect()
}

/// `get` for each of `tracks` (`(id, name)`), in turn.
pub async fn fetch_each<T, F, Fut>(tracks: Vec<(T, String)>, patience: Patience, get: F) -> Result<Fetched, SourceError>
where
    T: Clone,
    F: Fn(T, String) -> Fut,
    Fut: Future<Output = Result<PathBuf, SourceError>>,
{
    if tracks.is_empty() {
        return Err(SourceError::NothingToFetch);
    }
    let mut files = Vec::new();
    let mut missing = Vec::new();
    let mut refused: Option<SourceError> = None;
    let mut miss: Option<SourceError> = None;
    for (id, name) in tracks {
        if refused.is_some() {
            missing.push(name);
            continue;
        }
        let mut tries = 0;
        let mut pause = patience.first;
        loop {
            match get(id.clone(), name.clone()).await {
                Ok(path) => {
                    files.push(path);
                    break;
                }
                Err(e @ SourceError::RateLimited { .. }) => {
                    let wait = e.wait().unwrap_or(pause).max(pause);
                    if tries >= patience.tries || wait > patience.longest {
                        log::info!("{name}: the source asks to wait {}s, the rest of the album later", wait.as_secs());
                        refused = Some(e);
                        missing.push(name);
                        break;
                    }
                    log::info!("{name}: the source asks to wait, again in {}s", wait.as_secs());
                    tokio::time::sleep(wait).await;
                    tries += 1;
                    pause = (pause * 2).min(patience.longest);
                }
                Err(e) if e.is_refusal() => {
                    log::info!("{name}: {e}; the rest of the album is not asked for");
                    refused = Some(e);
                    missing.push(name);
                    break;
                }
                Err(e) => {
                    // The source has no file of this one: the next track.
                    log::info!("{name}: {e}");
                    miss.get_or_insert(e);
                    missing.push(name);
                    break;
                }
            }
        }
    }
    let error = refused.or(miss);
    match error {
        None => Ok(Fetched::Files(files)),
        Some(e) if files.is_empty() => Err(e),
        Some(error) => Ok(Fetched::Partial { files, missing, error }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn quick() -> Patience {
        Patience {
            first: Duration::from_millis(5),
            longest: Duration::from_millis(200),
            tries: 3,
        }
    }

    fn tracks(n: u32) -> Vec<(u32, String)> {
        (1..=n).map(|i| (i, format!("{i:02} - Song {i}"))).collect()
    }

    /// Track 3 answers 429 twice, then gives its file: waited for, not lost.
    #[tokio::test]
    async fn a_source_asking_to_wait_is_asked_again_after_the_wait() {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let started = std::time::Instant::now();
        let got = fetch_each(tracks(4), quick(), |i, name| {
            let asked = asked.clone();
            async move {
                let n = {
                    let mut a = asked.lock().unwrap();
                    a.push(i);
                    a.iter().filter(|x| **x == i).count()
                };
                if i == 3 && n <= 2 {
                    Err(SourceError::RateLimited { retry_after: Some(Duration::from_millis(20)) })
                } else {
                    Ok(PathBuf::from(name))
                }
            }
        })
        .await
        .unwrap();
        let Fetched::Files(files) = got else { panic!("{got:?}") };
        assert_eq!(files.len(), 4);
        assert_eq!(*asked.lock().unwrap(), [1, 2, 3, 3, 3, 4]);
        assert!(started.elapsed() >= Duration::from_millis(40), "it waited");
    }

    /// A wait longer than the patience is not waited out: what came stays.
    #[tokio::test]
    async fn a_long_wait_stops_the_album_and_keeps_what_came() {
        let got = fetch_each(tracks(4), quick(), |i, name| async move {
            if i >= 3 {
                Err(SourceError::RateLimited { retry_after: Some(Duration::from_secs(1800)) })
            } else {
                Ok(PathBuf::from(name))
            }
        })
        .await
        .unwrap();
        let Fetched::Partial { files, missing, error } = got else { panic!("{got:?}") };
        assert_eq!(files.len(), 2);
        assert_eq!(missing, ["03 - Song 3", "04 - Song 4"]);
        assert!(matches!(error, SourceError::RateLimited { .. }));
    }

    /// A 403 at track 2: the source is not asked for 3 and 4; track 1 stays.
    #[tokio::test]
    async fn a_refusal_stops_asking_and_keeps_the_tracks_that_came() {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let got = fetch_each(tracks(4), quick(), |i, name| {
            let asked = asked.clone();
            async move {
                asked.lock().unwrap().push(i);
                if i == 2 { Err(SourceError::Http { status: 403 }) } else { Ok(PathBuf::from(name)) }
            }
        })
        .await
        .unwrap();
        let Fetched::Partial { files, missing, error } = got else { panic!("{got:?}") };
        assert_eq!(files, [PathBuf::from("01 - Song 1")]);
        assert_eq!(missing.len(), 3);
        assert_eq!(error, SourceError::Http { status: 403 });
        assert_eq!(*asked.lock().unwrap(), [1, 2], "no hammering");
    }

    /// A track the source does not have is asked for once, and the album
    /// goes on.
    #[tokio::test]
    async fn a_missing_track_is_not_asked_again() {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let got = fetch_each(tracks(3), quick(), |i, name| {
            let asked = asked.clone();
            async move {
                asked.lock().unwrap().push(i);
                if i == 2 { Err(SourceError::Http { status: 404 }) } else { Ok(PathBuf::from(name)) }
            }
        })
        .await
        .unwrap();
        let Fetched::Partial { files, missing, .. } = got else { panic!("{got:?}") };
        assert_eq!((files.len(), missing), (2, vec!["02 - Song 2".to_owned()]));
        assert_eq!(*asked.lock().unwrap(), [1, 2, 3]);
        // Nothing at all: the error, not an empty success.
        let none = fetch_each(tracks(2), quick(), |_, _| async { Err::<PathBuf, _>(SourceError::Http { status: 403 }) }).await;
        assert_eq!(none.unwrap_err(), SourceError::Http { status: 403 });
    }

    #[test]
    fn the_tracks_that_came_are_left_out() {
        let all = vec![
            (1, "01 - Intro".to_owned(), "Intro".to_owned()),
            (2, "02 - Cadillac".to_owned(), "Cadillac".to_owned()),
            (3, "03 - Розовое вино".to_owned(), "Розовое вино".to_owned()),
        ];
        let left = left_out(all, &["CADILLAC".into(), "Intro".into()]);
        assert_eq!(left, [(3, "03 - Розовое вино".to_owned())]);
    }
}

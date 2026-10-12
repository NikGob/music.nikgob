//! One folder, one album.
//!
//! Files are identified one by one, so a folder of one album can come back with
//! several answers: a MusicBrainz match that picked another edition for a few
//! tracks, a track whose artist field carries a guest, a missing album-artist
//! tag. [`plan`] looks at the whole folder and, when most of it agrees on one
//! release, returns an [`Anchor`] that [`Anchor::apply`] stamps on every file, so
//! the folder ends up as a single release.
//!
//! The anchor only carries what belongs to the release (title, album artist,
//! year, MBIDs...); titles, track artists and numbers stay per track.

use std::path::{Path, PathBuf};

use nikgob_core::model::ReleaseType;
use nikgob_metadata::text::{album_key, artist_key};

use crate::identity::{IdentifiedBy, Identity, parse_credit};
use crate::layout::{VARIOUS_ARTISTS, is_various_artists};

/// The file and what the tags alone said about it.
pub(crate) struct Member<'a> {
    pub id: &'a Identity,
    pub base: &'a Identity,
}

/// What every file of the folder shares.
#[derive(Clone, Debug)]
pub(crate) struct Anchor {
    album: Option<String>,
    album_artist: Option<String>,
    year: Option<i32>,
    date: Option<String>,
    release_type: ReleaseType,
    compilation: bool,
    release_mbid: Option<String>,
    release_group_mbid: Option<String>,
    track_total: Option<u32>,
    disc_total: Option<u32>,
    cover_url: Option<String>,
    /// Album title keys the folder's files go by (tags and the matched release).
    titles: Vec<String>,
}

/// Same album title, or one the other plus more words ("X" and "X Deluxe").
fn same_title(a: &str, b: &str) -> bool {
    let within = |long: &str, short: &str| long.strip_prefix(short).is_some_and(|r| r.starts_with(' '));
    a == b || within(a, b) || within(b, a)
}

impl Anchor {
    /// Whether the file may join the folder's release: nothing it carries says
    /// it is another album (a different album title, a different album artist,
    /// a release of another release group).
    pub fn fits(&self, id: &Identity, base: &Identity) -> bool {
        if self.release_mbid.is_some() && id.release_mbid == self.release_mbid {
            return true;
        }
        let title = album_of(base).map(album_key).filter(|k| !k.is_empty());
        if title.as_ref().is_some_and(|k| !self.titles.iter().any(|t| same_title(t, k))) {
            return false;
        }
        let own = base.album_artist.as_deref().map(primary_credit).filter(|a| !a.is_empty());
        let mine = self.album_artist.as_deref().map(primary_credit);
        if let (false, Some(own), Some(mine)) = (self.compilation, own, mine)
            && artist_key(own) != artist_key(mine)
        {
            return false;
        }
        if let (Some(a), Some(b)) = (&id.release_group_mbid, &self.release_group_mbid)
            && a != b
            && title.is_none()
        {
            return false;
        }
        true
    }

    pub fn apply(&self, id: &mut Identity, base: &Identity) {
        // Another edition numbers its tracks differently: the file's own tags
        // know better where it sits on this one.
        let foreign = id.identified_by != IdentifiedBy::Tags
            && (id.release_mbid != self.release_mbid || id.album != self.album);
        if foreign && base.track_no != 0 {
            id.track_no = base.track_no;
            id.disc_no = base.disc_no;
        }
        id.album.clone_from(&self.album);
        id.album_artist.clone_from(&self.album_artist);
        id.year = self.year;
        id.date.clone_from(&self.date);
        id.release_type = self.release_type;
        id.compilation = self.compilation;
        id.release_mbid.clone_from(&self.release_mbid);
        id.release_group_mbid.clone_from(&self.release_group_mbid);
        id.track_total = self.track_total;
        id.disc_total = self.disc_total;
        id.cover_url = self
            .cover_url
            .clone()
            .or_else(|| id.cover_url.take().filter(|_| !foreign));
    }
}

/// The folder an album file belongs to: its directory, or the album directory
/// above a `CD1` / `Disc 2` subfolder.
pub(crate) fn album_group_dir(path: &Path) -> PathBuf {
    let dir = path.parent().unwrap_or(path);
    let disc_dir = dir
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .is_some_and(|n| {
            ["cd", "disc", "disk", "диск"].iter().any(|p| {
                n.strip_prefix(p)
                    .is_some_and(|r| r.trim().chars().next().is_some_and(|c| c.is_ascii_digit()))
            })
        });
    match dir.parent() {
        Some(up) if disc_dir => up.to_path_buf(),
        _ => dir.to_path_buf(),
    }
}

/// The first name of a credit line: `A`, `A, B`, `A & B` and `A feat. B` are
/// all filed under `A`.
pub(crate) fn primary_credit(line: &str) -> &str {
    let line = line.trim();
    let first = parse_credit(line).first().map_or(line.len(), |(_, n)| n.len());
    let head = line.get(..first).unwrap_or(line);
    let cut = [", ", " & ", " / "]
        .iter()
        .filter_map(|s| head.find(s))
        .min()
        .unwrap_or(head.len());
    head[..cut].trim()
}

/// Most common key, with its most common spelling and how often it occurs;
/// ties go to whatever came first.
fn top<K: PartialEq, T: Copy + PartialEq>(items: impl Iterator<Item = (K, T)>) -> Option<(K, T, usize)> {
    let mut groups: Vec<(K, Vec<T>)> = Vec::new();
    for (key, raw) in items {
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => v.push(raw),
            None => groups.push((key, vec![raw])),
        }
    }
    let at = (0..groups.len()).max_by_key(|&i| (groups[i].1.len(), std::cmp::Reverse(i)))?;
    let (key, raws) = groups.swap_remove(at);
    let mut spellings: Vec<(T, usize)> = Vec::new();
    for r in &raws {
        match spellings.iter_mut().find(|(s, _)| s == r) {
            Some((_, c)) => *c += 1,
            None => spellings.push((*r, 1)),
        }
    }
    let best = (0..spellings.len()).max_by_key(|&i| (spellings[i].1, std::cmp::Reverse(i)))?;
    Some((key, spellings[best].0, raws.len()))
}

fn album_of(id: &Identity) -> Option<&str> {
    id.album.as_deref().map(str::trim).filter(|a| !a.is_empty())
}

/// Album artist of the folder: what the tags say, else the artist most tracks
/// share, else "Various Artists". The flag tells it is the latter or a spelling
/// of it.
fn album_artist_of(members: &[Member<'_>]) -> (Option<String>, bool) {
    let tagged = top(members.iter().filter_map(|m| {
        let a = m.base.album_artist.as_deref()?.trim();
        (!a.is_empty()).then(|| (artist_key(a), a))
    }));
    if let Some((_, name, _)) = tagged {
        return (Some(name.to_owned()), is_various_artists(name));
    }
    let lead = top(members.iter().filter_map(|m| {
        let a = primary_credit(&m.base.artist_line);
        (!a.is_empty()).then(|| (artist_key(a), a))
    }));
    match lead {
        Some((_, name, n)) if n * 2 >= members.len() => (Some(name.to_owned()), false),
        Some(_) => (Some(VARIOUS_ARTISTS.to_owned()), true),
        None => (None, false),
    }
}

/// Decides whether the files form one album and, if so, which one. `None` when
/// the folder is a mix (loose files in one directory) or too small to tell.
pub(crate) fn plan(members: &[Member<'_>]) -> Option<Anchor> {
    let n = members.len();
    if n < 2 {
        return None;
    }
    // What the file tags say: two thirds of the folder name the same album.
    let tagged = top(
        members
            .iter()
            .filter_map(|m| album_of(m.base).map(|a| (album_key(a), a)))
            .filter(|(k, _)| !k.is_empty()),
    )
    .filter(|(_, _, c)| *c >= 2 && c * 3 >= n * 2);

    // A MusicBrainz release most of the folder was matched to (or a smaller
    // share of it when that release is the album the tags name).
    let mb = top(
        members
            .iter()
            .filter_map(|m| m.id.release_mbid.as_deref().map(|r| (r, r))),
    )
    .and_then(|(mbid, _, c)| {
        let w = members
            .iter()
            .find(|m| m.id.release_mbid.as_deref() == Some(mbid) && album_of(m.id).is_some())?
            .id;
        let named_by_tags = tagged
            .as_ref()
            .is_some_and(|(k, _, _)| album_of(w).map(album_key).as_ref() == Some(k));
        (c >= 2 && (c * 2 >= n || named_by_tags)).then_some(w)
    });

    let (artist, various) = album_artist_of(members);
    if let Some(w) = mb {
        let (album_artist, various) = match w.album_artist.as_deref().map(str::trim).filter(|a| !a.is_empty()) {
            Some(a) => (Some(a.to_owned()), is_various_artists(a)),
            None => (artist, various),
        };
        return Some(Anchor {
            album: w.album.clone(),
            album_artist,
            year: w.year,
            date: w.date.clone(),
            release_type: w.release_type,
            compilation: w.compilation || various,
            release_mbid: w.release_mbid.clone(),
            release_group_mbid: w.release_group_mbid.clone(),
            track_total: w.track_total,
            disc_total: w.disc_total,
            cover_url: w.cover_url.clone(),
            titles: album_of(w)
                .map(album_key)
                .into_iter()
                .chain(tagged.as_ref().map(|(k, _, _)| k.clone()))
                .collect(),
        });
    }

    let (key, album, _) = tagged?;
    let ours = || members.iter().filter(|m| album_of(m.base).map(album_key).as_ref() == Some(&key));
    let release_type = if various {
        ReleaseType::Compilation
    } else {
        top(ours().map(|m| (m.base.release_type, m.base.release_type)))
            .map_or(ReleaseType::Album, |(t, _, _)| t)
    };
    Some(Anchor {
        album: Some(album.to_owned()),
        album_artist: artist,
        year: top(ours().filter_map(|m| m.base.year.map(|y| (y, y)))).map(|(y, _, _)| y),
        date: top(ours().filter_map(|m| m.base.date.as_deref().map(|d| (d, d)))).map(|(d, _, _)| d.to_owned()),
        release_type,
        compilation: release_type == ReleaseType::Compilation,
        release_mbid: None,
        release_group_mbid: None,
        track_total: top(ours().filter_map(|m| m.base.track_total.map(|t| (t, t)))).map(|(t, _, _)| t),
        disc_total: top(ours().filter_map(|m| m.base.disc_total.map(|t| (t, t)))).map(|(t, _, _)| t),
        cover_url: None,
        titles: vec![key.clone()],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::identity_from_tags;
    use crate::tags::TagSet;

    fn tags(title: &str, no: u32, artist: &str, album_artist: Option<&str>, album: &str) -> TagSet {
        TagSet {
            title: Some(title.into()),
            artist: Some(artist.into()),
            album_artist: album_artist.map(Into::into),
            album: Some(album.into()),
            track_no: Some(no),
            year: Some(2022),
            ..TagSet::default()
        }
    }

    fn run(bases: Vec<Identity>, ids: Vec<Identity>) -> Vec<Identity> {
        let members: Vec<_> = ids.iter().zip(&bases).map(|(id, base)| Member { id, base }).collect();
        let anchor = plan(&members);
        let mut out = ids.clone();
        if let Some(a) = anchor {
            for (id, base) in out.iter_mut().zip(&bases) {
                if a.fits(id, base) {
                    a.apply(id, base);
                }
            }
        }
        out
    }

    fn id(t: &TagSet) -> Identity {
        identity_from_tags(t, Path::new("x.flac"))
    }

    #[test]
    fn credit_heads() {
        assert_eq!(primary_credit("RADWIMPS, Kazuma Jinnouchi"), "RADWIMPS");
        assert_eq!(primary_credit("A feat. B"), "A");
        assert_eq!(primary_credit("A & B"), "A");
        assert_eq!(primary_credit("  Кино "), "Кино");
        assert_eq!(primary_credit(""), "");
    }

    #[test]
    fn disc_folders_belong_to_the_album_folder() {
        assert_eq!(album_group_dir(Path::new("/m/Album/CD1/01.flac")), Path::new("/m/Album"));
        assert_eq!(album_group_dir(Path::new("/m/Album/Disc 2/01.flac")), Path::new("/m/Album"));
        assert_eq!(album_group_dir(Path::new("/m/Album/Discography/01.flac")), Path::new("/m/Album/Discography"));
        assert_eq!(album_group_dir(Path::new("/m/Album/01.flac")), Path::new("/m/Album"));
    }

    #[test]
    fn a_guest_track_without_album_artist_stays_on_the_album() {
        let mut bases: Vec<Identity> = (1..=8)
            .map(|n| id(&tags(&format!("T{n}"), n, "Radwimps", Some("Radwimps"), "Suzume")))
            .collect();
        bases.push(id(&tags("Guest", 9, "RADWIMPS, Kazuma Jinnouchi", None, "Suzume")));
        let out = run(bases.clone(), bases);
        assert!(out.iter().all(|i| i.album_artist.as_deref() == Some("Radwimps")), "{out:?}");
        assert_eq!(out[8].artist_line, "RADWIMPS, Kazuma Jinnouchi", "the track credit is untouched");
    }

    #[test]
    fn without_album_artist_tags_the_dominant_artist_leads() {
        let mut bases: Vec<Identity> = (1..=5)
            .map(|n| id(&tags(&format!("T{n}"), n, "Radwimps", None, "Suzume")))
            .collect();
        bases.push(id(&tags("Guest", 6, "RADWIMPS feat. Someone", None, "Suzume")));
        let out = run(bases.clone(), bases);
        assert!(out.iter().all(|i| i.album_artist.as_deref() == Some("Radwimps")), "{out:?}");
    }

    #[test]
    fn many_artists_make_a_compilation() {
        let bases: Vec<Identity> = ["A", "B", "C", "D"]
            .iter()
            .enumerate()
            .map(|(i, a)| id(&tags("T", i as u32 + 1, a, None, "Hits")))
            .collect();
        let out = run(bases.clone(), bases);
        assert!(out.iter().all(|i| i.compilation && i.release_type == ReleaseType::Compilation));
        assert!(out.iter().all(|i| i.album_artist.as_deref() == Some(VARIOUS_ARTISTS)));
    }

    #[test]
    fn the_musicbrainz_release_most_tracks_agree_on_wins() {
        let bases: Vec<Identity> = (1..=6)
            .map(|n| id(&tags(&format!("T{n}"), n, "Kevin Penkin", Some("Kevin Penkin"), "Made in Abyss OST 2")))
            .collect();
        let mut ids = bases.clone();
        for (i, x) in ids.iter_mut().enumerate() {
            x.identified_by = IdentifiedBy::MusicBrainz;
            if i < 4 {
                x.release_mbid = Some("main".into());
                x.album = Some("MADE IN ABYSS ORIGINAL SOUNDTRACK 2".into());
                x.year = Some(2020);
            } else {
                x.release_mbid = Some(format!("other{i}"));
                x.album = Some(format!("Edition {i}"));
                x.track_no += 20;
            }
        }
        let out = run(bases, ids);
        assert!(out.iter().all(|i| i.release_mbid.as_deref() == Some("main")), "{out:?}");
        assert!(out.iter().all(|i| i.album.as_deref() == Some("MADE IN ABYSS ORIGINAL SOUNDTRACK 2")));
        assert!(out.iter().all(|i| i.year == Some(2020)));
        assert_eq!(out[5].track_no, 6, "numbering of another edition is replaced by the file's own");
    }

    #[test]
    fn title_variants_form_one_album_but_another_album_does_not_join() {
        let mut bases: Vec<Identity> = (1..=5)
            .map(|n| id(&tags(&format!("T{n}"), n, "Kevin Penkin", Some("Kevin Penkin"), "Made in Abyss Original Soundtrack 2")))
            .collect();
        bases.push(id(&tags("T6", 6, "Kevin Penkin", Some("Kevin Penkin"), "MADE IN ABYSS ORIGINAL SOUNDTRACK 2 Bonus")));
        bases.push(id(&tags("T7", 7, "Kevin Penkin", Some("Kevin Penkin"), "Made in Abyss: Fukaki Tamashii no Reimei")));
        let out = run(bases.clone(), bases.clone());
        assert!(out[..6].iter().all(|i| i.album.as_deref() == Some("Made in Abyss Original Soundtrack 2")), "{out:?}");
        assert_eq!(out[6], bases[6], "a different album title is left as it is");
    }

    #[test]
    fn a_file_of_another_album_in_a_two_against_one_folder_is_untouched() {
        let bases = vec![
            id(&tags("A1", 1, "Artist A", Some("Artist A"), "Album A")),
            id(&tags("A2", 2, "Artist A", Some("Artist A"), "Album A")),
            id(&tags("B1", 1, "Artist B", Some("Artist B"), "Album B")),
        ];
        let out = run(bases.clone(), bases.clone());
        assert_eq!(out, bases);
        // Even with a title that happens to fit, another explicit album artist keeps it out.
        let mut odd = bases.clone();
        odd[2] = id(&tags("B1", 3, "Artist B", Some("Artist B"), "Album A"));
        let out = run(odd.clone(), odd.clone());
        assert_eq!(out[2].album_artist.as_deref(), Some("Artist B"));
    }

    #[test]
    fn mixed_folders_and_lone_files_are_left_alone() {
        let one = vec![id(&tags("T", 1, "A", Some("A"), "X"))];
        assert_eq!(run(one.clone(), one.clone()), one);
        let mixed: Vec<Identity> = (1..=4)
            .map(|n| id(&tags("T", n, &format!("Artist {n}"), Some(&format!("Artist {n}")), &format!("Album {n}"))))
            .collect();
        assert_eq!(run(mixed.clone(), mixed.clone()), mixed);
        let two = vec![id(&tags("T", 1, "A", Some("A"), "X")), id(&tags("U", 1, "B", Some("B"), "Y"))];
        assert_eq!(run(two.clone(), two.clone()), two);
    }
}

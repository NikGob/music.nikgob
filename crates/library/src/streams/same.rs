//! One song, one track: a stream track without a known remote id or ISRC
//! lands on the library track of the same song (docs/stage3.md §1), so the
//! song liked on Spotify and on YouTube Music is one track with two
//! sources, not two.
//!
//! The same song: the same main artist and title after [`song_key`] (or
//! the title with and without its romanization, «夏子 - Natsuko» and «夏子»:
//! [`same_dual_title`]), both lengths known and within [`SLACK_MS`],
//! compatible albums when both have one, no two different ISRCs, and the
//! library track has no live source of the stream's service yet (a service
//! never gives one song twice; the ISRC handles its relinks). Lengths within
//! [`TIGHT_SLACK_MS`] need no album agreement: the album in another
//! language, the single next to the album, a reworded title. The folding
//! drops only what services add without changing the recording
//! ("Remastered 2011", "Album Version", "feat. X"); "Live", "Remix", "TV
//! Size", "Instrumental", "Radio Edit" stay in the key, so other versions
//! stay other tracks.
//!
//! [`Library::merge_duplicate_stream_tracks`] applies the same rule once to
//! tracks created before it existed.

use nikgob_core::clip_title::same_dual_title;
use nikgob_core::model::{SourceKind, score};
use nikgob_core::streaming::{Service, StreamTrack, TrackKind};
use rusqlite::{Connection, OptionalExtension, params};

use super::normal_isrc;
use crate::catalog::merge_track_in;
use crate::db::Library;
use crate::error::Result;
use crate::util::{meta_get, meta_set, sort_key};

/// How far two lengths of one song may be apart.
pub(crate) const SLACK_MS: i64 = 3_000;

/// Lengths this close (a stream gives whole seconds) make the song without
/// agreeing albums.
pub(crate) const TIGHT_SLACK_MS: i64 = 1_500;

/// `meta` key: the repair of [`Library::merge_duplicate_stream_tracks`] ran
/// with the rule of today (`v1` had no dual titles and no tight slack).
const REPAIRED_KEY: &str = "stream_duplicates_merged.v2";

/// Words a bracket or " - " tail may consist of and still be release noise.
const NOISE: &[&str] = &[
    "remaster",
    "remastered",
    "remastering",
    "ремастер",
    "ремастеринг",
    "digital",
    "digitally",
    "explicit",
    "clean",
    "mono",
    "stereo",
    "album",
    "version",
    "single",
    "original",
    "deluxe",
    "edition",
    "bonus",
    "track",
    "anniversary",
    "expanded",
    "reissue",
];

fn words(s: &str) -> impl Iterator<Item = &str> {
    s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty())
}

fn is_feat(s: &str) -> bool {
    let s = s.trim_start();
    ["feat.", "feat ", "ft.", "ft ", "featuring ", "with ", "при участии "]
        .iter()
        .any(|p| s.starts_with(p))
}

/// Noise only: every word a noise word or a number ("2011", "20th").
fn is_noise(body: &str) -> bool {
    let mut any = false;
    for w in words(body) {
        any = true;
        if !(NOISE.contains(&w) || w.starts_with(|c: char| c.is_ascii_digit())) {
            return false;
        }
    }
    any || body.trim().is_empty()
}

/// Lower case, `ё` → `е`, noise brackets / tails and `feat.` dropped,
/// punctuation gone, spaces collapsed (see the module docs).
pub(crate) fn song_key(raw: &str) -> String {
    let lower: String = raw
        .chars()
        .flat_map(char::to_lowercase)
        .map(|c| if c == 'ё' { 'е' } else { c })
        .collect();
    // Brackets: noise and guests go, anything else stays as words.
    let mut kept = String::with_capacity(lower.len());
    let mut rest = lower.as_str();
    while let Some(open) = rest.find(['(', '[']) {
        kept.push_str(&rest[..open]);
        let close_ch = if rest[open..].starts_with('(') { ')' } else { ']' };
        let after = &rest[open + 1..];
        let (body, next) = match after.find(close_ch) {
            Some(close) => (&after[..close], &after[close + 1..]),
            None => (after, ""),
        };
        if !(is_feat(body) || is_noise(body)) {
            kept.push(' ');
            kept.push_str(body);
            kept.push(' ');
        }
        rest = next;
    }
    kept.push_str(rest);
    // " - 2011 Remaster", " - feat. X": the same; " - Live" stays.
    let mut parts = kept.split_whitespace().collect::<Vec<_>>().join(" ");
    for dash in [" - ", " – ", " — "] {
        let segs: Vec<&str> = parts.split(dash).collect();
        if segs.len() > 1 {
            let mut out = segs[0].to_owned();
            for s in &segs[1..] {
                if !(is_feat(s) || is_noise(s)) {
                    out.push(' ');
                    out.push_str(s);
                }
            }
            parts = out;
        }
    }
    // A bare "feat. X" tail.
    for cut in [" feat. ", " feat ", " ft. ", " featuring "] {
        if let Some(at) = parts.find(cut) {
            parts.truncate(at);
        }
    }
    words(&parts).collect::<Vec<_>>().join(" ")
}

/// The main artist of a name or credit line ("A, B", "A & B", "A feat. B" → "a").
pub(crate) fn main_artist_key(name: &str) -> String {
    let lower = name.to_lowercase();
    let mut end = lower.len();
    for sep in [", ", " & ", " feat", " ft.", " featuring ", " x "] {
        if let Some(at) = lower.find(sep)
            && at > 0
        {
            end = end.min(at);
        }
    }
    song_key(&lower[..end])
}

/// What two tracks are compared by, already folded.
#[derive(Clone, Debug)]
pub(crate) struct Song {
    pub(crate) title: String,
    /// The title as given, for [`same_dual_title`].
    pub(crate) raw_title: String,
    pub(crate) artist: String,
    pub(crate) album: Option<String>,
    pub(crate) duration_ms: Option<i64>,
    pub(crate) isrc: Option<String>,
}

impl Song {
    pub(crate) fn new(
        title: &str,
        artist: &str,
        album: Option<&str>,
        duration_ms: Option<i64>,
        isrc: Option<&str>,
    ) -> Self {
        Self {
            title: song_key(title),
            raw_title: title.to_owned(),
            artist: main_artist_key(artist),
            album: album.map(song_key).filter(|a| !a.is_empty()),
            duration_ms,
            isrc: normal_isrc(isrc),
        }
    }

    pub(crate) fn of_stream(t: &StreamTrack) -> Self {
        let artist = t
            .artists
            .first()
            .map_or_else(|| t.artist_line(), |a| a.name.clone());
        Self::new(
            &t.title,
            &artist,
            t.album.as_ref().map(|a| a.title.as_str()),
            t.duration_ms.and_then(|d| i64::try_from(d).ok()),
            t.isrc.as_deref(),
        )
    }

    /// See the module docs.
    pub(crate) fn same(&self, o: &Self) -> bool {
        if self.title.is_empty() || self.artist.is_empty() {
            return false;
        }
        if let (Some(a), Some(b)) = (&self.isrc, &o.isrc)
            && a != b
        {
            return false;
        }
        let Some(apart) = self.duration_ms.zip(o.duration_ms).map(|(a, b)| (a - b).abs()) else {
            return false;
        };
        apart <= SLACK_MS
            && self.artist == o.artist
            && (self.title == o.title || same_dual_title(&self.raw_title, &o.raw_title, song_key))
            && (apart <= TIGHT_SLACK_MS
                || match (&self.album, &o.album) {
                    (Some(a), Some(b)) => albums_agree(a, b),
                    _ => true,
                })
    }
}

/// The same album title, or one an extension of the other ("Discovery"
/// and "Discovery Bonus Mix"). A single named after the song is not taken
/// for its album here (it cannot be told from the album named after it);
/// only [`TIGHT_SLACK_MS`] joins the two.
fn albums_agree(a: &str, b: &str) -> bool {
    let starts = |long: &str, short: &str| {
        long == short || long.strip_prefix(short).is_some_and(|r| r.starts_with(' '))
    };
    starts(a, b) || starts(b, a)
}

/// A library track the song already is, for a stream track whose remote id
/// and ISRC are unknown here; `None` for videos, unknown lengths and
/// anything doubtful.
pub(crate) fn same_song_track(conn: &Connection, t: &StreamTrack) -> Result<Option<String>> {
    if t.kind == TrackKind::Video {
        return Ok(None);
    }
    let song = Song::of_stream(t);
    let Some(prefix) = t.artists.first().map(|a| sort_key(&a.name)) else {
        return Ok(None);
    };
    let found = candidates(conn, &song, &prefix, None)?
        .into_iter()
        .find(|(id, _, _)| !has_service(conn, id, t.id.service).unwrap_or(true))
        .map(|(id, _, _)| id);
    Ok(found)
}

/// Live tracks of the same song: `(id, has_local, seq)`, a track with a file
/// first, then the oldest; without `exclude`.
fn candidates(
    conn: &Connection,
    song: &Song,
    artist_prefix: &str,
    exclude: Option<&str>,
) -> Result<Vec<(String, bool, i64)>> {
    let Some(d) = song.duration_ms else {
        return Ok(Vec::new());
    };
    if artist_prefix.is_empty() {
        return Ok(Vec::new());
    }
    let upper = format!("{artist_prefix}\u{10FFFF}");
    let mut stmt = conn.prepare_cached(
        "SELECT t.id, t.title, t.artist_credit, t.duration_ms, r.title, t.isrc, t.has_local, t.seq
           FROM track t LEFT JOIN release r ON r.id = t.release_id
          WHERE t.artist_key >= ?1 AND t.artist_key < ?2
            AND t.duration_ms BETWEEN ?3 AND ?4 AND t.deleted_at IS NULL
            AND t.id IS NOT ?5
          ORDER BY t.has_local DESC, t.seq",
    )?;
    let rows = stmt.query_map(
        params![artist_prefix, upper, d - SLACK_MS, d + SLACK_MS, exclude],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<i64>>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, bool>(6)?,
                r.get::<_, i64>(7)?,
            ))
        },
    )?;
    let mut out = Vec::new();
    for row in rows {
        let (id, title, credit, duration, album, isrc, local, seq) = row?;
        let artist = main_credit_name(&credit);
        let other = Song::new(&title, &artist, album.as_deref(), duration, isrc.as_deref());
        if song.same(&other) {
            out.push((id, local, seq));
        }
    }
    Ok(out)
}

/// The first credited name of a stored credit (json).
fn main_credit_name(credit: &str) -> String {
    serde_json::from_str::<nikgob_core::model::ArtistCredit>(credit)
        .ok()
        .and_then(|c| c.0.first().map(|p| p.name.clone()))
        .unwrap_or_default()
}

/// The track has a live source of this service.
fn has_service(conn: &Connection, track: &str, service: Service) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM source WHERE track_id = ?1 AND kind = ?2 AND trashed_at IS NULL)",
        params![track, service.source_kind().as_str()],
        |r| r.get(0),
    )?)
}

/// The services a track has live sources of.
fn services_of(conn: &Connection, track: &str) -> Result<Vec<Service>> {
    let mut stmt = conn.prepare_cached(
        "SELECT DISTINCT kind FROM source WHERE track_id = ?1 AND kind != 'local_file'
            AND trashed_at IS NULL",
    )?;
    let kinds = stmt.query_map([track], |r| r.get::<_, String>(0))?;
    let mut out = Vec::new();
    for k in kinds {
        if let Some(s) = SourceKind::parse(&k?).and_then(SourceKind::service) {
            out.push(s);
        }
    }
    Ok(out)
}

/// Stream-only live tracks: `(id, title, credit, duration, album, isrc, seq)`.
type StreamOnlyRow = (String, String, String, Option<i64>, Option<String>, Option<String>, i64);

/// Stream-only live tracks that are songs: a music video (its source
/// scored [`score::STREAM_YOUTUBE_CLIP`]) is never its song by name.
fn stream_only_rows(conn: &Connection) -> Result<Vec<StreamOnlyRow>> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.title, t.artist_credit, t.duration_ms, r.title, t.isrc, t.seq
           FROM track t LEFT JOIN release r ON r.id = t.release_id
          WHERE t.deleted_at IS NULL AND t.duration_ms IS NOT NULL
            AND NOT EXISTS (SELECT 1 FROM source s WHERE s.track_id = t.id AND s.kind = 'local_file')
            AND EXISTS (SELECT 1 FROM source s WHERE s.track_id = t.id AND s.kind != 'local_file'
                           AND s.trashed_at IS NULL)
            AND NOT EXISTS (SELECT 1 FROM source s WHERE s.track_id = t.id AND s.kind = 'youtube'
                               AND s.quality_score = ?1 AND s.trashed_at IS NULL)
          ORDER BY t.seq",
    )?;
    let rows = stmt.query_map([score::STREAM_YOUTUBE_CLIP], |r| {
        Ok((
            r.get(0)?,
            r.get(1)?,
            r.get(2)?,
            r.get(3)?,
            r.get(4)?,
            r.get(5)?,
            r.get(6)?,
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// A music video without a file (see [`stream_only_rows`]).
fn is_clip(conn: &Connection, track: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT NOT EXISTS(SELECT 1 FROM source WHERE track_id = ?1 AND kind = 'local_file')
            AND EXISTS(SELECT 1 FROM source WHERE track_id = ?1 AND kind = 'youtube'
                          AND quality_score = ?2 AND trashed_at IS NULL)",
        params![track, score::STREAM_YOUTUBE_CLIP],
        |r| r.get(0),
    )?)
}

fn track_exists(conn: &Connection, id: &str) -> Result<bool> {
    Ok(conn
        .query_row("SELECT 1 FROM track WHERE id = ?1", [id], |_| Ok(()))
        .optional()?
        .is_some())
}

impl Library {
    /// Merges stream-only tracks that are one song by the rule of
    /// [`same_song_track`] (made before the rule existed: the song liked on
    /// both services as two tracks; music videos stay apart). The track with a file, else the older
    /// one, stays; sources, ids in the services, likes, playlist entries,
    /// history and wishlist items move onto it (one wishlist item per track).
    /// Runs once per library (a `meta` mark); returns how many tracks went.
    pub fn merge_duplicate_stream_tracks(&self) -> Result<usize> {
        if self.read(|c| meta_get(c, REPAIRED_KEY))?.is_some() {
            return Ok(0);
        }
        let device = self.device_id();
        let now = self.tick();
        self.write(|tx| {
            let mut merged = 0;
            for (id, title, credit, duration, album, isrc, seq) in stream_only_rows(tx)? {
                if !track_exists(tx, &id)? {
                    continue; // folded into an earlier one
                }
                let artist = main_credit_name(&credit);
                let song = Song::new(&title, &artist, album.as_deref(), duration, isrc.as_deref());
                let mine = services_of(tx, &id)?;
                let mut other = None;
                for (cand, local, cseq) in candidates(tx, &song, &sort_key(&artist), Some(&id))? {
                    if is_clip(tx, &cand)? {
                        continue;
                    }
                    let theirs = services_of(tx, &cand)?;
                    if !theirs.iter().any(|s| mine.contains(s)) {
                        other = Some((cand, local || cseq < seq));
                        break;
                    }
                }
                let Some((cand, keep_theirs)) = other else { continue };
                let (from, into) = if keep_theirs { (&id, &cand) } else { (&cand, &id) };
                if merge_track_in(tx, super::parse(from)?, super::parse(into)?, device.as_str(), now)? {
                    log::info!("library: stream track {from} is {into}, merged");
                    merged += 1;
                }
            }
            meta_set(tx, REPAIRED_KEY, &now.to_string())?;
            Ok(merged)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folding_drops_release_noise_and_keeps_versions() {
        assert_eq!(song_key("Группа крови"), "группа крови");
        assert_eq!(song_key("Группа Крови (Remastered 2019)"), "группа крови");
        assert_eq!(song_key("One More Time - 2001 Remaster"), "one more time");
        assert_eq!(song_key("Song (feat. Someone)"), "song");
        assert_eq!(song_key("Song feat. Someone"), "song");
        assert_eq!(song_key("Song [Album Version]"), "song");
        assert_eq!(song_key("Ёлка"), "елка");
        for version in [
            "Song (Live)",
            "Song - Live",
            "Song (Remix)",
            "Song (TV Size)",
            "Song (Instrumental)",
            "Song (Radio Edit)",
            "Song (Live 1995)",
        ] {
            assert_ne!(song_key(version), "song", "{version}");
        }
        assert_eq!(song_key("Song (Live)"), song_key("Song - Live"));
        assert_eq!(main_artist_key("Daft Punk, Romanthony"), "daft punk");
        assert_eq!(main_artist_key("Кино & Друзья"), "кино");
        assert_eq!(main_artist_key("A feat. B"), "a");
    }

    #[test]
    fn same_song_needs_length_artist_title_and_agreeing_albums() {
        let a = Song::new("Automatic", "宇多田ヒカル", Some("First Love"), Some(306_000), Some("ZZFAK9900001"));
        let yt = Song::new("Automatic", "宇多田ヒカル", Some("First Love (Remastered)"), Some(305_000), None);
        assert!(a.same(&yt));
        let far = Song { duration_ms: Some(310_000), ..yt.clone() };
        assert!(!a.same(&far), "4 s apart");
        let unknown = Song { duration_ms: None, ..yt.clone() };
        assert!(!a.same(&unknown), "length unknown");
        let other_album = Song { album: Some("live in budokan".into()), duration_ms: Some(304_000), ..yt.clone() };
        assert!(!a.same(&other_album), "another album 2 s off");
        let single = Song { album: Some("automatic".into()), duration_ms: Some(304_000), ..yt.clone() };
        assert!(!a.same(&single), "a single 2 s off is another release");
        let no_album = Song { album: None, ..yt.clone() };
        assert!(a.same(&no_album));
        let isrc = Song { isrc: Some("ZZFAK9900009".into()), ..yt.clone() };
        assert!(!a.same(&isrc), "two different ISRCs");
        let remix = Song::new("Automatic (Remix)", "宇多田ヒカル", Some("First Love"), Some(306_000), None);
        assert!(!a.same(&remix));
        let artist = Song::new("Automatic", "Someone Else", Some("First Love"), Some(306_000), None);
        assert!(!a.same(&artist));
    }

    #[test]
    fn lengths_within_the_tight_slack_need_no_album() {
        let local = Song::new("夏子", "FUNKY MONKEY BΛBY'S", Some("夏子"), Some(212_800), None);
        let yt = Song::new("夏子 - Natsuko", "Funky Monkey Bλby's", Some("Natsuko"), Some(213_000), None);
        assert!(local.same(&yt), "the dual title on a single in another script");
        let ost = Song::new("Old Stories", "Kevin Penkin", Some("MADE IN ABYSS ORIGINAL SOUNDTRACK 3"), Some(120_400), None);
        let jp = Song::new("Old Stories", "Kevin Penkin", Some("TVアニメ「メイドインアビス 烈日の黄金郷」オリジナルサウンドトラック"), Some(121_000), None);
        assert!(ost.same(&jp));
        let off = Song { duration_ms: Some(122_000), ..jp.clone() };
        assert!(!ost.same(&off), "1.6 s off: the albums have to agree again");
        let isrc = Song { isrc: Some("ZZFAK9900009".into()), ..jp.clone() };
        assert!(ost.same(&isrc), "one ISRC alone is no veto");
        let both = Song { isrc: Some("ZZFAK9900001".into()), ..ost.clone() };
        assert!(!both.same(&isrc), "two different ISRCs");
        let live = Song::new("夏子 - Live", "Funky Monkey Bλby's", Some("Natsuko"), Some(213_000), None);
        assert!(!local.same(&live), "a version");
    }
}

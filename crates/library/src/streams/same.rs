//! One song, one track: a stream track without a known remote id or ISRC
//! lands on the library track of the same song, so the
//! song liked on Spotify and on YouTube Music is one track with two
//! sources, not two.
//!
//! The same song: a shared artist (the main one, or any credited name:
//! «Raj Ramayya» and «Kevin Penkin & Raj Ramayya») and the same title after [`song_key`] (or
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
//! Whatever the titles, two tracks at one place of one release (the same
//! release id, disc and number) with lengths within [`PLACE_SLACK_MS`] and
//! a shared artist are one song: a service names it in kanji («滑落亭»),
//! the file in romaji («KATSURAKUTEI»). Credits are compared name by name
//! («Kevin Penkin、immi» and «Kevin Penkin & immi»: [`credit_keys`]). With
//! the same title and lengths within [`TIGHT_SLACK_MS`] no shared artist is
//! needed (the file says «Ohara Yuiko», the service 大原ゆい子), and a
//! track there that has the stream's service already is that song too: a
//! service does give one song twice, under the album's id and the
//! single's. A track is its place: candidates on
//! the stream's own release come first.
//!
//! A music video is its song's file: a liked clip
//! whose title, without what clips add ([`song_titles`]), is the title of a
//! track with a file, by one of the names the clip credits (its uploader or
//! a name in its title: [`credited_names`]), and is that long or up to
//! [`CLIP_EXTRA_MS`] longer (an intro, an outro) lands on that track: the
//! file plays.
//!
//! Titles in two scripts that cannot be compared («ベラフの子守歌» of a
//! service, «Belaf's Lullaby» of the file: [`other_scripts`]) make one song
//! only on the evidence of the rest: a stream without a file is the file of
//! its album (the same release, or the same album title: «… SOUNDTRACK 2»
//! is not «… SOUNDTRACK») by a shared
//! artist with lengths within [`TIGHT_SLACK_MS`], when no other track of
//! that album is within [`SLACK_MS`] of its length and the file has no
//! source of the stream's service yet. The file's title stays, the other
//! one is the track's alias ([`Library::merge_other_script_titles`]).
//!
//! A playlist that follows an album's order tells the rest
//! ([`Library::fold_playlist_gaps`]): streams without a file between two
//! files of one disc of an album, as many as the places between them,
//! whose files the playlist lacks, are those files in turn — each as long
//! as its file within [`PLACE_SLACK_MS`], or, of unknown length, under a
//! title in another script than the file's («ベラフの子守歌» between
//! «Capital of the Unreturned» 2-13 and «Doll x Dragon» 2-15 is «Belaf's
//! Lullaby» 2-14). A run that does not fill its gap exactly, or of which
//! one stream does not fit, stays as it is.
//!
//! [`Library::merge_duplicate_stream_tracks`] applies the same rules once to
//! tracks created before they existed, and folds one song filed twice on
//! one release under titles a sound mark apart («パウロ VS ルディ» and
//! «バウロ VS ルディ», lengths within [`TYPO_SLACK_MS`]): the second file
//! stays a spare copy.

use nikgob_core::clip_title::{credited_names, same_dual_title, song_titles};
use nikgob_core::translit::{prefer_cyrillic, same_translit};
use nikgob_core::model::{PlaylistId, SourceKind, score};
use nikgob_core::streaming::{EntityKind, Service, StreamTrack, TrackKind};
use rusqlite::{Connection, OptionalExtension, params};

use super::normal_isrc;
use crate::catalog::merge_track_in;
use crate::db::Library;
use crate::error::Result;

/// How far two lengths of one song may be apart.
pub(crate) const SLACK_MS: i64 = 3_000;

/// Lengths this close (a stream gives whole seconds) make the song without
/// agreeing albums.
pub(crate) const TIGHT_SLACK_MS: i64 = 1_500;

/// Lengths this close at one place of one release make the song whatever
/// the titles.
pub(crate) const PLACE_SLACK_MS: i64 = 2_000;

/// How much longer than its song a music video may be (an intro, an outro).
pub(crate) const CLIP_EXTRA_MS: i64 = 30_000;

/// Two files of one release this close, titles a sound mark apart, are one
/// song (one rip of it under two names).
pub(crate) const TYPO_SLACK_MS: i64 = 50;

/// `meta` key of [`Library::merge_other_script_titles`].
const OTHER_SCRIPTS_KEY: &str = "other_script_titles_merged.v1";

/// `meta` key: the repair of [`Library::merge_duplicate_stream_tracks`] ran
/// with the rule of today (`v1` had no dual titles and no tight slack, `v2`
/// no shared artists and no clips, `v3` no transliterations, `v4` no places
/// on a release, `v5` no twins of one service at a place, no artists in
/// two scripts at a place, no typos, `v6` no places numbered past the first
/// disc: [`real_place`]).
const REPAIRED_KEY: &str = "stream_duplicates_merged.v7";

/// What separates the artists of a credit line (lower case).
const CREDIT_SEPARATORS: &[&str] = &[
    ", ", " & ", "、", "，", " × ", " x ", " / ", "; ", " feat. ", " feat ", " ft. ", " featuring ",
];

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

/// The main artist of a name or credit line ("A, B", "A & B", "A feat. B",
/// "A、B" → "a").
pub(crate) fn main_artist_key(name: &str) -> String {
    let lower = name.to_lowercase();
    let mut end = lower.len();
    for sep in [", ", " & ", " feat", " ft.", " featuring ", " x ", "、", "，", " × "] {
        if let Some(at) = lower.find(sep)
            && at > 0
        {
            end = end.min(at);
        }
    }
    song_key(&lower[..end])
}

/// Every artist a name or credit line names, folded ("Kevin Penkin、immi"
/// → "kevin penkin", "immi").
pub(crate) fn credit_keys(name: &str) -> Vec<String> {
    let mut parts = vec![name.to_lowercase()];
    for sep in CREDIT_SEPARATORS {
        parts = parts.iter().flat_map(|p| p.split(sep)).map(str::to_owned).collect();
    }
    let mut out: Vec<String> = Vec::new();
    for p in parts {
        let k = song_key(&p);
        if !k.is_empty() && !out.contains(&k) {
            out.push(k);
        }
    }
    out
}

/// Where a track is on its release: `(release id, disc, number)`.
pub(crate) type Place = (String, i64, i64);

/// The place of a track that has a number on a release.
pub(crate) fn place_of(release: Option<String>, disc: i64, track_no: i64) -> Option<Place> {
    release.filter(|_| track_no > 0).map(|r| (r, disc.max(1), track_no))
}

/// The place a service means by a number past the first disc of an album
/// whose files fill more discs: it numbers them all as one («Doll x Dragon»,
/// 2-15 of the files, is its 30 after the 15 of disc 1). The discs are as
/// long as their files say; any other place stays.
pub(crate) fn real_place(conn: &Connection, place: Place) -> Result<Place> {
    if place.1 != 1 {
        return Ok(place);
    }
    let mut stmt = conn.prepare_cached(
        "SELECT disc_no, max(track_no) FROM track
          WHERE release_id = ?1 AND has_local = 1 AND deleted_at IS NULL AND track_no > 0
          GROUP BY disc_no ORDER BY disc_no",
    )?;
    let discs: Vec<(i64, i64)> = stmt
        .query_map([&place.0], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    if discs.len() < 2 || discs[0].0 != 1 {
        return Ok(place);
    }
    let mut no = place.2;
    for (disc, size) in &discs {
        if no <= *size {
            return Ok(if *disc == 1 { place } else { (place.0, *disc, no) });
        }
        no -= size;
    }
    Ok(place)
}

/// What two tracks are compared by, already folded.
#[derive(Clone, Debug)]
pub(crate) struct Song {
    pub(crate) title: String,
    /// The title as given, for [`same_dual_title`].
    pub(crate) raw_title: String,
    pub(crate) artist: String,
    /// The main artist and every other credited name, folded.
    pub(crate) artists: Vec<String>,
    pub(crate) album: Option<String>,
    pub(crate) duration_ms: Option<i64>,
    pub(crate) isrc: Option<String>,
    /// Where it is on its release, when known (see the module docs).
    pub(crate) place: Option<Place>,
}

impl Song {
    pub(crate) fn new(
        title: &str,
        artist: &str,
        album: Option<&str>,
        duration_ms: Option<i64>,
        isrc: Option<&str>,
    ) -> Self {
        let artist = main_artist_key(artist);
        Self {
            title: song_key(title),
            raw_title: title.to_owned(),
            artists: if artist.is_empty() { Vec::new() } else { vec![artist.clone()] },
            artist,
            album: album.map(song_key).filter(|a| !a.is_empty()),
            duration_ms,
            isrc: normal_isrc(isrc),
            place: None,
        }
    }

    pub(crate) fn at(mut self, place: Option<Place>) -> Self {
        self.place = place;
        self
    }

    /// Every credited name counts as an artist of the song, not only the main one.
    pub(crate) fn with_names<S: AsRef<str>>(mut self, names: &[S]) -> Self {
        for n in names {
            for k in credit_keys(n.as_ref()) {
                if !self.artists.contains(&k) {
                    self.artists.push(k);
                }
            }
        }
        self
    }

    pub(crate) fn of_stream(t: &StreamTrack) -> Self {
        let artist = t
            .artists
            .first()
            .map_or_else(|| t.artist_line(), |a| a.name.clone());
        let names: Vec<&str> = t.artists.iter().map(|a| a.name.as_str()).collect();
        Self::new(
            &t.title,
            &artist,
            t.album.as_ref().map(|a| a.title.as_str()),
            t.duration_ms.and_then(|d| i64::try_from(d).ok()),
            t.isrc.as_deref(),
        )
        .with_names(&names)
    }

    /// The main artist is the same, or one credited name is on both (also
    /// when one side spells it in Latin letters: «MORGENSHTERN»).
    fn shares_artist(&self, o: &Self) -> bool {
        let same = |a: &String, b: &String| a == b || same_translit(a, b);
        same(&self.artist, &o.artist) || self.artists.iter().any(|a| o.artists.iter().any(|b| same(a, b)))
    }

    fn same_title(&self, o: &Self) -> bool {
        self.title == o.title || same_translit(&self.title, &o.title)
    }

    /// A twin at one place: the same title, lengths within
    /// [`TIGHT_SLACK_MS`] (one song, whatever services both have).
    pub(crate) fn twin(&self, o: &Self) -> bool {
        self.place.is_some()
            && self.place == o.place
            && !self.title.is_empty()
            && self.same_title(o)
            && self.duration_ms.zip(o.duration_ms).is_some_and(|(a, b)| (a - b).abs() <= TIGHT_SLACK_MS)
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
        if self.place.is_some() && self.place == o.place {
            if apart <= PLACE_SLACK_MS && self.shares_artist(o) {
                return true;
            }
            if apart <= TIGHT_SLACK_MS && self.same_title(o) {
                return true;
            }
            // A character's song: the service credits the character
            // («ベラフ（CV：…）»), the file the composer. Neither the titles
            // nor the names can be compared, the place and length tell.
            if apart <= TIGHT_SLACK_MS
                && other_scripts(&self.raw_title, &o.raw_title)
                && other_scripts(&self.artist, &o.artist)
            {
                return true;
            }
        }
        apart <= SLACK_MS
            && self.shares_artist(o)
            && (self.title == o.title
                || same_dual_title(&self.raw_title, &o.raw_title, song_key)
                || same_translit(&self.title, &o.title))
            && (apart <= TIGHT_SLACK_MS
                || match (&self.album, &o.album) {
                    (Some(a), Some(b)) => albums_agree(a, b),
                    _ => true,
                })
    }
}

/// Two titles in scripts that cannot be compared: one in Latin letters,
/// the other without a Latin or Cyrillic letter (kana, kanji, Hangul…).
/// Cyrillic and Latin are compared by their transliteration instead.
pub fn other_scripts(a: &str, b: &str) -> bool {
    let latin = |s: &str| nikgob_core::names::is_latin(s);
    let none = |s: &str| {
        s.chars().any(char::is_alphabetic)
            && !s.chars().any(|c| c.is_ascii_alphabetic() || ('\u{400}'..='\u{52f}').contains(&c))
    };
    (latin(a) && none(b)) || (latin(b) && none(a))
}

/// The track with a file that the stream-only track `id` (`title`, by the
/// names of `credit`, `duration` long, on `release` called `album`) is
/// under a title in another script (see the module docs), if one alone is.
#[allow(clippy::too_many_arguments)]
fn other_script_file(
    conn: &Connection,
    id: &str,
    title: &str,
    credit: &str,
    duration: i64,
    release: Option<&str>,
    album: Option<&str>,
) -> Result<Option<String>> {
    let names = credit_names(credit);
    let artist = names.first().cloned().unwrap_or_default();
    let song = Song::new(title, &artist, album, Some(duration), None).with_names(&names);
    if song.artists.is_empty() {
        return Ok(None);
    }
    let mut stmt = conn.prepare_cached(
        "SELECT t.id, t.title, t.artist_credit, t.duration_ms, t.release_id, r.title FROM track t
           LEFT JOIN release r ON r.id = t.release_id
          WHERE t.has_local = 1 AND t.deleted_at IS NULL AND t.id != ?1
            AND t.duration_ms BETWEEN ?2 AND ?3",
    )?;
    let rows = stmt.query_map(params![id, duration - TIGHT_SLACK_MS, duration + TIGHT_SLACK_MS], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, Option<String>>(4)?,
            r.get::<_, Option<String>>(5)?,
        ))
    })?;
    let mut found: Vec<(String, String)> = Vec::new();
    for row in rows {
        let (file, file_title, file_credit, file_duration, file_release, file_album) = row?;
        if !other_scripts(title, &file_title) {
            continue;
        }
        let same_release = release.is_some() && release == file_release.as_deref();
        let albums = match (&song.album, file_album.as_deref().map(song_key)) {
            (Some(a), Some(b)) => !b.is_empty() && *a == b,
            _ => false,
        };
        let file_names = credit_names(&file_credit);
        let other = Song::new(&file_title, file_names.first().map_or("", String::as_str), file_album.as_deref(), Some(file_duration), None)
            .with_names(&file_names);
        if (same_release || albums) && song.shares_artist(&other) {
            if let Some(r) = file_release {
                found.push((file, r));
            }
        }
    }
    let [(file, file_release)] = &found[..] else { return Ok(None) };
    // Alone within the slack among the album's places (its files and
    // numbered tracks), and no source of the stream's services yet.
    let crowd: i64 = conn.query_row(
        "SELECT count(*) FROM track WHERE release_id = ?1 AND deleted_at IS NULL AND id NOT IN (?2, ?5)
            AND (has_local = 1 OR track_no > 0) AND duration_ms BETWEEN ?3 AND ?4",
        params![file_release, file, duration - SLACK_MS, duration + SLACK_MS, id],
        |r| r.get(0),
    )?;
    if crowd > 0 {
        return Ok(None);
    }
    for s in services_of(conn, id)? {
        if has_service(conn, file, s)? {
            return Ok(None);
        }
    }
    Ok(Some(file.clone()))
}

/// The same album title, or one an extension of the other ("Discovery"
/// and "Discovery Bonus Mix"). A single named after the song is not taken
/// for its album here (it cannot be told from the album named after it);
/// only [`TIGHT_SLACK_MS`] joins the two.
pub(crate) fn albums_agree(a: &str, b: &str) -> bool {
    let starts = |long: &str, short: &str| {
        long == short || long.strip_prefix(short).is_some_and(|r| r.starts_with(' '))
    };
    starts(a, b) || starts(b, a) || same_translit(a, b)
}

/// The track `id` (and its album) takes the Cyrillic spelling of `other`'s
/// title (album) when its own is that title transliterated («ChETYRE
/// UKRAINKI» → «ЧЕТЫРЕ УКРАИНКИ»).
pub(crate) fn prefer_cyrillic_in(conn: &Connection, id: &str, other_title: &str, other_album: Option<&str>) -> Result<()> {
    let Some((title, release, album)) = conn
        .query_row(
            "SELECT t.title, t.release_id, r.title FROM track t LEFT JOIN release r ON r.id = t.release_id
              WHERE t.id = ?1",
            [id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?)),
        )
        .optional()?
    else {
        return Ok(());
    };
    let better = prefer_cyrillic(&title, other_title);
    if better != title {
        conn.execute(
            "UPDATE track SET title = ?2, title_key = ?3 WHERE id = ?1",
            params![id, better, crate::util::sort_key(better)],
        )?;
    }
    if let (Some(release), Some(album), Some(other)) = (release, album, other_album) {
        let better = prefer_cyrillic(&album, other);
        if better != album {
            let key = crate::util::sort_key(better);
            conn.execute("UPDATE release SET title = ?2, title_key = ?3 WHERE id = ?1", params![release, better, key])?;
            conn.execute("UPDATE track SET album_key = ?2 WHERE release_id = ?1", params![release, key])?;
        }
    }
    Ok(())
}

/// A library track the song already is, for a stream track whose remote id
/// and ISRC are unknown here; `None` for unknown lengths and anything
/// doubtful. A music video finds only the file of its song ([`Clip`]).
pub(crate) fn same_song_track(conn: &Connection, t: &StreamTrack) -> Result<Option<String>> {
    if t.kind == TrackKind::Video {
        let names: Vec<String> = t.artists.iter().map(|a| a.name.clone()).collect();
        let Some(d) = t.duration_ms.and_then(|d| i64::try_from(d).ok()) else {
            return Ok(None);
        };
        return clip_song_file(conn, &Clip::new(&t.title, &names, d), None);
    }
    let song = Song::of_stream(t);
    let home = match t.album.as_ref().and_then(|a| a.id.as_ref()) {
        Some(album) => super::linked(conn, "release", album.service, EntityKind::Album, &album.id)?,
        None => None,
    };
    let found = candidates(conn, &song, None, home.as_deref())?
        .into_iter()
        .find(|c| !has_service(conn, &c.id, t.id.service).unwrap_or(true))
        .map(|c| c.id);
    Ok(found)
}

/// The track of `release` (not at a place: another number, no number)
/// that the stream `t` is by the rule of [`same_song_track`], without a
/// source of the stream's service yet.
pub(crate) fn same_song_on_release(conn: &Connection, t: &StreamTrack, release: &str) -> Result<Option<String>> {
    if t.kind == TrackKind::Video {
        return Ok(None);
    }
    let song = Song::of_stream(t);
    for c in candidates(conn, &song, None, Some(release))? {
        if c.release.as_deref() != Some(release) {
            break; // the release's own come first
        }
        if !has_service(conn, &c.id, t.id.service)? {
            return Ok(Some(c.id));
        }
    }
    Ok(None)
}

/// The track at `place` of its release that the stream `t` is (see the
/// module docs: the titles may differ), when it has no source of the
/// stream's service yet; a track with a file first.
pub(crate) fn same_place_track(conn: &Connection, t: &StreamTrack, place: &Place) -> Result<Option<String>> {
    if t.kind == TrackKind::Video {
        return Ok(None);
    }
    let song = Song::of_stream(t).at(Some(place.clone()));
    let mut stmt = conn.prepare_cached(
        "SELECT id, title, artist_credit, duration_ms FROM track
          WHERE release_id = ?1 AND disc_no = ?2 AND track_no = ?3 AND deleted_at IS NULL
          ORDER BY has_local DESC, seq",
    )?;
    let rows = stmt.query_map(params![place.0, place.1, place.2], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<i64>>(3)?))
    })?;
    for row in rows {
        let (id, title, credit, duration) = row?;
        let names = credit_names(&credit);
        let artist = names.first().cloned().unwrap_or_default();
        let other = Song::new(&title, &artist, None, duration, None)
            .with_names(&names)
            .at(Some(place.clone()));
        if song.same(&other) && (song.twin(&other) || !has_service(conn, &id, t.id.service)?) {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

/// A live track of the same song (see [`candidates`]).
struct Candidate {
    id: String,
    local: bool,
    seq: i64,
    release: Option<String>,
    /// At the song's place under its title ([`Song::twin`]).
    twin: bool,
}

/// Live tracks of the same song: those on `home` (the song's own release)
/// first, then a track with a file, then the oldest; without `exclude`.
fn candidates(conn: &Connection, song: &Song, exclude: Option<&str>, home: Option<&str>) -> Result<Vec<Candidate>> {
    let Some(d) = song.duration_ms else {
        return Ok(Vec::new());
    };
    if song.artists.is_empty() {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare_cached(
        "SELECT t.id, t.title, t.artist_credit, t.duration_ms, r.title, t.isrc, t.has_local, t.seq,
                t.release_id, t.disc_no, t.track_no
           FROM track t LEFT JOIN release r ON r.id = t.release_id
          WHERE t.duration_ms BETWEEN ?1 AND ?2 AND t.deleted_at IS NULL
            AND t.id IS NOT ?3
          ORDER BY (?4 IS NOT NULL AND t.release_id = ?4) DESC, t.has_local DESC, t.seq",
    )?;
    let home = home.or(song.place.as_ref().map(|p| p.0.as_str()));
    let rows = stmt.query_map(params![d - SLACK_MS, d + SLACK_MS, exclude, home], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, Option<i64>>(3)?,
            r.get::<_, Option<String>>(4)?,
            r.get::<_, Option<String>>(5)?,
            r.get::<_, bool>(6)?,
            r.get::<_, i64>(7)?,
            place_of(r.get(8)?, r.get(9)?, r.get(10)?),
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, title, credit, duration, album, isrc, local, seq, place) = row?;
        let names = credit_names(&credit);
        let artist = names.first().cloned().unwrap_or_default();
        let release = place.as_ref().map(|p| p.0.clone());
        let other = Song::new(&title, &artist, album.as_deref(), duration, isrc.as_deref())
            .with_names(&names)
            .at(place);
        if song.same(&other) {
            let release = match release {
                Some(r) => Some(r),
                None => conn.query_row("SELECT release_id FROM track WHERE id = ?1", [&id], |r| r.get(0))?,
            };
            out.push(Candidate { twin: song.twin(&other), id, local, seq, release });
        }
    }
    Ok(out)
}

/// The release has a track with a file: a library album, whose tracks
/// are its places (no stream row of it goes into a file elsewhere).
fn has_files(conn: &Connection, release: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM track WHERE release_id = ?1 AND has_local = 1 AND deleted_at IS NULL)",
        [release],
        |r| r.get(0),
    )?)
}

/// Pairs `(from, into)` of tracks with files on one release that are one
/// song under titles a sound mark apart (see the module docs). `into` is
/// the one alone at its place, else the older one.
fn typo_twins(conn: &Connection) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare_cached(
        "SELECT a.id, a.title, b.id, b.title,
                (SELECT count(*) FROM track o WHERE o.release_id = a.release_id AND o.disc_no = a.disc_no
                    AND o.track_no = a.track_no AND o.id != a.id AND o.deleted_at IS NULL AND a.track_no > 0),
                (SELECT count(*) FROM track o WHERE o.release_id = b.release_id AND o.disc_no = b.disc_no
                    AND o.track_no = b.track_no AND o.id != b.id AND o.deleted_at IS NULL AND b.track_no > 0)
           FROM track a JOIN track b ON b.release_id = a.release_id AND b.seq > a.seq
          WHERE a.has_local = 1 AND b.has_local = 1 AND a.deleted_at IS NULL AND b.deleted_at IS NULL
            AND abs(a.duration_ms - b.duration_ms) <= ?1
          ORDER BY a.seq, b.seq",
    )?;
    let rows = stmt.query_map([TYPO_SLACK_MS], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, i64>(4)?,
            r.get::<_, i64>(5)?,
        ))
    })?;
    let key = |t: &str| song_key(&nikgob_core::names::voiceless(t));
    let mut out = Vec::new();
    for row in rows {
        let (a, ta, b, tb, crowd_a, crowd_b) = row?;
        let k = key(&ta);
        if k.is_empty() || k != key(&tb) {
            continue;
        }
        // The older one stays, unless it shares its place with another song.
        out.push(if crowd_a > 0 && crowd_b == 0 { (a, b) } else { (b, a) });
    }
    Ok(out)
}

/// A music video as the song it shows (see the module docs).
#[derive(Clone, Debug)]
pub(crate) struct Clip {
    /// Its title without what clips add (each reading), folded by [`song_key`].
    titles: Vec<String>,
    /// The uploader and the names its title credits, folded.
    artists: Vec<String>,
    duration_ms: i64,
}

impl Clip {
    pub(crate) fn new(title: &str, uploader: &[String], duration_ms: i64) -> Self {
        let lead = uploader.first().map(String::as_str).unwrap_or_default();
        let mut titles = vec![song_key(title)];
        for t in song_titles(title, lead) {
            let k = song_key(&t);
            if !k.is_empty() && !titles.contains(&k) {
                titles.push(k);
            }
        }
        let mut artists: Vec<String> = Vec::new();
        for n in uploader.iter().cloned().chain(credited_names(title, lead)) {
            let k = main_artist_key(&n);
            if !k.is_empty() && !artists.contains(&k) {
                artists.push(k);
            }
        }
        titles.retain(|t| !t.is_empty());
        Self { titles, artists, duration_ms }
    }

    /// The song `song` (its title, credited names and length) is what the
    /// clip shows.
    pub(crate) fn shows(&self, title: &str, names: &[String], duration_ms: i64) -> bool {
        let extra = self.duration_ms - duration_ms;
        (-SLACK_MS..=CLIP_EXTRA_MS).contains(&extra)
            && self.titles.contains(&song_key(title))
            && names.iter().any(|n| self.artists.contains(&main_artist_key(n)))
    }
}

/// The one track with a file the clip shows (none when two would do).
fn clip_song_file(conn: &Connection, clip: &Clip, exclude: Option<&str>) -> Result<Option<String>> {
    let mut stmt = conn.prepare_cached(
        "SELECT t.id, t.title, t.artist_credit, t.duration_ms FROM track t
          WHERE t.duration_ms BETWEEN ?1 AND ?2 AND t.deleted_at IS NULL AND t.has_local = 1
            AND t.id IS NOT ?3
          ORDER BY t.seq",
    )?;
    let rows = stmt.query_map(
        params![clip.duration_ms - CLIP_EXTRA_MS, clip.duration_ms + SLACK_MS, exclude],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?)),
    )?;
    let mut found = Vec::new();
    for row in rows {
        let (id, title, credit, duration) = row?;
        if clip.shows(&title, &credit_names(&credit), duration) {
            found.push(id);
        }
    }
    Ok(match &found[..] {
        [one] => Some(one.clone()),
        _ => None,
    })
}

/// Every credited name of a stored credit (json).
fn credit_names(credit: &str) -> Vec<String> {
    serde_json::from_str::<nikgob_core::model::ArtistCredit>(credit)
        .map(|c| c.0.into_iter().map(|p| p.name).collect())
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

/// Stream-only live tracks: `(id, title, credit, duration, album, isrc,
/// seq, place)`.
type StreamOnlyRow = (String, String, String, Option<i64>, Option<String>, Option<String>, i64, Option<Place>);

/// Stream-only live tracks that are songs: a music video (its source
/// scored [`score::STREAM_YOUTUBE_CLIP`]) is never its song by name.
fn stream_only_rows(conn: &Connection) -> Result<Vec<StreamOnlyRow>> {
    let mut stmt = conn.prepare_cached(
        "SELECT t.id, t.title, t.artist_credit, t.duration_ms, r.title, t.isrc, t.seq,
                t.release_id, t.disc_no, t.track_no
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
            place_of(r.get(7)?, r.get(8)?, r.get(9)?),
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Live music videos without a file: `(id, title, credit, duration)`.
fn clip_rows(conn: &Connection) -> Result<Vec<(String, String, String, i64)>> {
    let mut stmt = conn.prepare_cached(
        "SELECT t.id, t.title, t.artist_credit, t.duration_ms FROM track t
          WHERE t.deleted_at IS NULL AND t.duration_ms IS NOT NULL
            AND NOT EXISTS (SELECT 1 FROM source s WHERE s.track_id = t.id AND s.kind = 'local_file')
            AND EXISTS (SELECT 1 FROM source s WHERE s.track_id = t.id AND s.kind = 'youtube'
                           AND s.quality_score = ?1 AND s.trashed_at IS NULL)
          ORDER BY t.seq",
    )?;
    let rows = stmt.query_map([score::STREAM_YOUTUBE_CLIP], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
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

/// One live entry of a playlist as [`playlist_gaps`] reads it: its track,
/// title, length, whether it has a file, its place.
struct GapEntry {
    track: String,
    title: String,
    duration: Option<i64>,
    local: bool,
    place: Option<Place>,
}

/// The pairs `(stream, file)` of a playlist that are one song by the
/// order of the playlist (see the module docs).
fn playlist_gaps(conn: &Connection, playlist: &str) -> Result<Vec<(String, String)>> {
    let entries: Vec<GapEntry> = {
        let mut stmt = conn.prepare_cached(
            "SELECT t.id, t.title, t.duration_ms, t.has_local, t.release_id, t.disc_no, t.track_no
               FROM playlist_entry e JOIN track t ON t.id = e.track_id
              WHERE e.playlist_id = ?1 AND e.deleted = 0 AND t.deleted_at IS NULL
              ORDER BY e.position, e.id",
        )?;
        let rows = stmt.query_map([playlist], |r| {
            Ok(GapEntry {
                track: r.get(0)?,
                title: r.get(1)?,
                duration: r.get(2)?,
                local: r.get(3)?,
                place: place_of(r.get(4)?, r.get(5)?, r.get(6)?),
            })
        })?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let in_list: std::collections::HashSet<&str> = entries.iter().map(|e| e.track.as_str()).collect();
    let file_place = |e: &GapEntry| e.place.clone().filter(|_| e.local);
    let mut out = Vec::new();
    let mut i = 0;
    while i < entries.len() {
        let Some(from) = file_place(&entries[i]) else {
            i += 1;
            continue;
        };
        let mut j = i + 1;
        while j < entries.len() && !entries[j].local {
            j += 1;
        }
        let run = &entries[i + 1..j.min(entries.len())];
        let to = entries.get(j).and_then(file_place);
        if let Some(to) = to.filter(|t| t.0 == from.0 && t.1 == from.1 && !run.is_empty())
            && to.2 - from.2 - 1 == run.len() as i64
        {
            let mut pairs = Vec::new();
            for (k, s) in run.iter().enumerate() {
                let file: Option<(String, String, Option<i64>)> = conn
                    .query_row(
                        "SELECT id, title, duration_ms FROM track WHERE release_id = ?1 AND disc_no = ?2 AND track_no = ?3
                            AND has_local = 1 AND deleted_at IS NULL",
                        params![from.0, from.1, from.2 + 1 + k as i64],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .optional()?;
                let Some((f, title, duration)) = file.filter(|f| !in_list.contains(f.0.as_str())) else { break };
                let fits = match (s.duration, duration) {
                    (Some(a), Some(b)) => (a - b).abs() <= PLACE_SLACK_MS,
                    (None, _) => other_scripts(&s.title, &title),
                    (Some(_), None) => false,
                };
                let taken = services_of(conn, &s.track)?.into_iter().map(|sv| has_service(conn, &f, sv)).collect::<Result<Vec<_>>>()?;
                if !fits || taken.into_iter().any(|t| t) {
                    break;
                }
                pairs.push((s.track.clone(), f));
            }
            if pairs.len() == run.len() {
                out.extend(pairs);
            }
        }
        i = j.max(i + 1);
    }
    Ok(out)
}

/// [`playlist_gaps`] of `playlist` merged: each stream goes into its file,
/// its title an alias of it when another. Returns how many went.
fn fold_playlist_gaps_in(conn: &Connection, playlist: &str, device: &str, now: i64) -> Result<usize> {
    let mut merged = 0;
    for (stream, file) in playlist_gaps(conn, playlist)? {
        let title: String = conn.query_row("SELECT title FROM track WHERE id = ?1", [&stream], |r| r.get(0))?;
        if merge_track_in(conn, super::parse(&stream)?, super::parse(&file)?, device, now)? {
            crate::aliases::add_track_alias_in(conn, &file, &title, "merged")?;
            note!("library: stream track {stream} «{title}» is the file {file} at its place in playlist {playlist}, merged");
            merged += 1;
        }
    }
    Ok(merged)
}

impl Library {
    /// The streams of a playlist that are files of the library by the
    /// order of the playlist (see the module docs) go into those files.
    /// Returns how many went.
    pub fn fold_playlist_gaps(&self, playlist: PlaylistId) -> Result<usize> {
        let device = self.device_id();
        let now = self.tick();
        self.write(|tx| fold_playlist_gaps_in(tx, &playlist.to_string(), device.as_str(), now))
    }

    /// Once per library (`other_script_titles_merged.v1`): a stream-only
    /// track that is a file of the library under a title in another script
    /// (see the module docs: «ベラフの子守歌» and «Belaf's Lullaby») goes
    /// into it, and so do the streams of every playlist that are files by
    /// its order ([`Self::fold_playlist_gaps`]); the file's title stays,
    /// the stream's is its alias. Returns how many tracks went.
    pub fn merge_other_script_titles(&self) -> Result<usize> {
        let device = self.device_id();
        let now = self.tick();
        Ok(self.repair(OTHER_SCRIPTS_KEY, |tx| {
            let mut merged = 0;
            for (id, title, credit, duration, album, _, _, place) in stream_only_rows(tx)? {
                let Some(duration) = duration else { continue };
                if !track_exists(tx, &id)? {
                    continue;
                }
                let release = match place {
                    Some(p) => Some(p.0),
                    None => tx.query_row("SELECT release_id FROM track WHERE id = ?1", [&id], |r| r.get(0))?,
                };
                let Some(file) = other_script_file(tx, &id, &title, &credit, duration, release.as_deref(), album.as_deref())? else {
                    continue;
                };
                if merge_track_in(tx, super::parse(&id)?, super::parse(&file)?, device.as_str(), now)? {
                    crate::aliases::add_track_alias_in(tx, &file, &title, "merged")?;
                    note!("library: stream track {id} «{title}» is the file {file} under another title, merged");
                    merged += 1;
                }
            }
            let playlists: Vec<String> = {
                let mut stmt = tx.prepare("SELECT id FROM playlist WHERE deleted = 0 ORDER BY seq")?;
                let rows = stmt.query_map([], |r| r.get(0))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            for p in playlists {
                merged += fold_playlist_gaps_in(tx, &p, device.as_str(), now)?;
            }
            Ok((merged, merged))
        })?
        .unwrap_or(0))
    }

    /// Merges stream-only tracks that are one song by the rule of
    /// [`same_song_track`] (made before the rule existed: the song liked on
    /// both services as two tracks), and liked music videos into the file of
    /// their song ([`Clip`]). The track with a file, else the older one, stays; sources, ids in the services, likes, playlist entries,
    /// history and wishlist items move onto it (one wishlist item per track).
    /// Runs once per library (a `meta` mark); returns how many tracks went.
    pub fn merge_duplicate_stream_tracks(&self) -> Result<usize> {
        let device = self.device_id();
        let now = self.tick();
        Ok(self.repair(REPAIRED_KEY, |tx| {
            let mut merged = 0;
            for (id, title, credit, duration, album, isrc, seq, place) in stream_only_rows(tx)? {
                if !track_exists(tx, &id)? {
                    continue; // folded into an earlier one
                }
                let names = credit_names(&credit);
                let artist = names.first().cloned().unwrap_or_default();
                let place = place.map(|p| real_place(tx, p)).transpose()?;
                let song = Song::new(&title, &artist, album.as_deref(), duration, isrc.as_deref())
                    .with_names(&names)
                    .at(place);
                let mine = services_of(tx, &id)?;
                let home = song.place.as_ref().map(|p| p.0.clone());
                // A row of a library album stays at its place.
                let at_home = match &home {
                    Some(r) => has_files(tx, r)?,
                    None => false,
                };
                let mut other = None;
                for c in candidates(tx, &song, Some(&id), None)? {
                    if is_clip(tx, &c.id)? || (at_home && c.release != home) {
                        continue;
                    }
                    let theirs = services_of(tx, &c.id)?;
                    if c.twin || !theirs.iter().any(|s| mine.contains(s)) {
                        other = Some((c.id, c.local || c.seq < seq));
                        break;
                    }
                }
                let Some((cand, keep_theirs)) = other else { continue };
                let (from, into) = if keep_theirs { (&id, &cand) } else { (&cand, &id) };
                let (from_title, from_album): (String, Option<String>) = tx.query_row(
                    "SELECT t.title, r.title FROM track t LEFT JOIN release r ON r.id = t.release_id WHERE t.id = ?1",
                    [from],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
                if merge_track_in(tx, super::parse(from)?, super::parse(into)?, device.as_str(), now)? {
                    prefer_cyrillic_in(tx, into, &from_title, from_album.as_deref())?;
                    note!("library: stream track {from} is {into}, merged");
                    merged += 1;
                }
            }
            // Music videos go into the file of their song.
            for (id, title, credit, duration) in clip_rows(tx)? {
                let clip = Clip::new(&title, &credit_names(&credit), duration);
                let Some(file) = clip_song_file(tx, &clip, Some(&id))? else { continue };
                if merge_track_in(tx, super::parse(&id)?, super::parse(&file)?, device.as_str(), now)? {
                    note!("library: clip {id} is the song {file}, merged");
                    merged += 1;
                }
            }
            // One song filed twice on one release under a typo.
            for (from, into) in typo_twins(tx)? {
                if !track_exists(tx, &from)? || !track_exists(tx, &into)? {
                    continue;
                }
                if merge_track_in(tx, super::parse(&from)?, super::parse(&into)?, device.as_str(), now)? {
                    note!("library: track {from} is {into} under a typo, merged (its file a spare copy)");
                    merged += 1;
                }
            }
            Ok((merged, merged))
        })?
        .unwrap_or(0))
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

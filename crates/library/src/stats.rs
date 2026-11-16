//! Reports over the history and the files for the local API: what was
//! listened to in a period, the songs loved once and not played lately,
//! and the health of the library (sizes, formats, texts, fake lossless
//! files, masters without a copy elsewhere). Reads only.

use nikgob_core::model::{ArtistId, ReleaseId, TrackId, UnixMs};
use rusqlite::{Connection, params};

use crate::db::Library;
use crate::error::Result;
use crate::util::get_id;

/// A skip before this far into a song is an early one.
pub const EARLY_SKIP_MS: i64 = 30_000;

/// One song of a report.
#[derive(Clone, Debug, PartialEq)]
pub struct StatTrack {
    pub id: TrackId,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub plays: u32,
    pub completes: u32,
    pub skips: u32,
    /// The last start or end of it (unix ms).
    pub last_played: Option<UnixMs>,
    pub liked: bool,
}

/// An artist of a report, by the first credited artist of its songs.
#[derive(Clone, Debug, PartialEq)]
pub struct StatArtist {
    pub id: Option<ArtistId>,
    pub name: String,
    pub plays: u32,
}

/// An album of a report.
#[derive(Clone, Debug, PartialEq)]
pub struct StatAlbum {
    pub id: ReleaseId,
    pub title: String,
    pub artist: String,
    pub plays: u32,
}

/// What was listened to in `[from_ms, to_ms)`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ListeningReport {
    pub from_ms: UnixMs,
    pub to_ms: UnixMs,
    /// Songs started.
    pub plays: u32,
    /// Played to their end.
    pub completes: u32,
    pub skips: u32,
    /// Skipped before [`EARLY_SKIP_MS`].
    pub early_skips: u32,
    /// Time heard: songs played to the end at their length, skipped ones
    /// up to where they were left.
    pub listened_ms: i64,
    /// Different songs started.
    pub distinct_tracks: u32,
    pub top_tracks: Vec<StatTrack>,
    pub top_artists: Vec<StatArtist>,
    pub top_albums: Vec<StatAlbum>,
    /// Songs started in each hour of the day (local time by the offset
    /// asked for), 0–23.
    pub by_hour: [u32; 24],
    /// Songs started on each device (its id), most first.
    pub by_device: Vec<(String, u32)>,
    /// Songs started from each kind of place (`album`, `playlist`,
    /// `radio`…), most first.
    pub by_context: Vec<(String, u32)>,
}

/// Files of one kind of format.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FormatCount {
    /// `lossless`, `opus`, `mp3`, `aac`, `vorbis`, `other`.
    pub group: String,
    pub files: u32,
    pub bytes: i64,
}

/// A local file the spectrum check found to be made from a lossy one.
#[derive(Clone, Debug, PartialEq)]
pub struct FakeLossless {
    pub track: TrackId,
    pub title: String,
    pub artist: String,
    /// `transcoded` or `upsampled`.
    pub verdict: String,
    /// What it was likely made from (`lossy:320`…).
    pub origin: String,
}

/// The state of the library's files and texts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LibraryHealth {
    /// Songs of the library (not in the Trash).
    pub tracks: u32,
    /// Of them, with a file here.
    pub local_tracks: u32,
    pub albums: u32,
    pub artists: u32,
    pub playlists: u32,
    pub liked_tracks: u32,
    /// Songs in the Trash.
    pub trashed_tracks: u32,
    /// What the songs' files take (the file each plays from).
    pub local_bytes: i64,
    /// The length of the songs with a file.
    pub local_ms: i64,
    pub formats: Vec<FormatCount>,
    /// Songs with a file and no timed text known (none from LRCLIB or
    /// another source kept).
    pub without_synced_lyrics: u32,
    /// Local files the spectrum check found to be made from a lossy file.
    pub fake_lossless: u32,
    /// The first of them.
    pub fake_lossless_examples: Vec<FakeLossless>,
    /// Lossless masters kept, and what they take.
    pub masters: u32,
    pub masters_bytes: i64,
    /// Masters no other device holds a checked copy of.
    pub masters_without_replica: u32,
    /// Files the library knows that are not there.
    pub missing_files: u32,
}

/// The format group of a codec as stored (`flac`, `opus`, `mp3`…).
pub fn format_group(codec: &str) -> &'static str {
    match codec.to_ascii_lowercase().as_str() {
        "flac" | "alac" | "wav" | "aiff" | "ape" | "wavpack" | "wv" | "dsd" | "pcm" => "lossless",
        "opus" => "opus",
        "mp3" => "mp3",
        "aac" | "m4a" | "mp4" => "aac",
        "vorbis" | "ogg" => "vorbis",
        _ => "other",
    }
}

fn count(c: &Connection, sql: &str) -> Result<u32> {
    Ok(c.query_row(sql, [], |r| r.get::<_, i64>(0))?.max(0) as u32)
}

fn u(v: i64) -> u32 {
    u32::try_from(v.max(0)).unwrap_or(u32::MAX)
}

/// A song of the `track` table read with its stats: columns `t.id,
/// t.title, t.artist_display, r.title, plays, completes, skips, last,
/// liked` from `base`.
fn stat_track(r: &rusqlite::Row<'_>) -> rusqlite::Result<StatTrack> {
    Ok(StatTrack {
        id: get_id(r, 0)?,
        title: r.get(1)?,
        artist: r.get(2)?,
        album: r.get(3)?,
        plays: u(r.get(4)?),
        completes: u(r.get(5)?),
        skips: u(r.get(6)?),
        last_played: r.get(7)?,
        liked: r.get::<_, i64>(8)? != 0,
    })
}

const LIKED_SQL: &str = "EXISTS(SELECT 1 FROM likes l WHERE l.target_kind = 'track' AND l.target_id = t.id AND l.deleted = 0)";

impl Library {
    /// What was listened to in `[from_ms, to_ms)`: the totals, the first
    /// `limit` songs, artists and albums, the hours of the day (shifted by
    /// `tz_offset_s` from UTC), the devices and the places plays came from.
    pub fn listening_report(&self, from_ms: UnixMs, to_ms: UnixMs, limit: u32, tz_offset_s: i64) -> Result<ListeningReport> {
        self.read(|c| {
            let mut rep = ListeningReport { from_ms, to_ms, ..Default::default() };
            let (plays, completes, skips, early, distinct): (i64, i64, i64, i64, i64) = c.query_row(
                "SELECT coalesce(SUM(type = 'start'), 0), coalesce(SUM(type = 'complete'), 0), coalesce(SUM(type = 'skip'), 0),
                        coalesce(SUM(type = 'skip' AND coalesce(position_ms, 0) < ?3), 0),
                        COUNT(DISTINCT CASE WHEN type = 'start' THEN track_id END)
                   FROM event WHERE ts >= ?1 AND ts < ?2",
                params![from_ms, to_ms, EARLY_SKIP_MS],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )?;
            rep.plays = u(plays);
            rep.completes = u(completes);
            rep.skips = u(skips);
            rep.early_skips = u(early);
            rep.distinct_tracks = u(distinct);
            rep.listened_ms = c.query_row(
                "SELECT coalesce(SUM(CASE WHEN e.type = 'complete' THEN coalesce(t.duration_ms, e.position_ms, 0)
                                          WHEN e.type = 'skip' THEN coalesce(e.position_ms, 0) ELSE 0 END), 0)
                   FROM event e LEFT JOIN track t ON t.id = e.track_id
                  WHERE e.ts >= ?1 AND e.ts < ?2",
                params![from_ms, to_ms],
                |r| r.get(0),
            )?;
            {
                let mut stmt = c.prepare_cached(&format!(
                    "SELECT t.id, t.title, t.artist_display, r.title, s.plays, s.completes, s.skips, s.last, {LIKED_SQL}
                       FROM (SELECT track_id, SUM(type = 'start') AS plays, SUM(type = 'complete') AS completes,
                                    SUM(type = 'skip') AS skips, MAX(ts) AS last
                               FROM event WHERE ts >= ?1 AND ts < ?2 GROUP BY track_id) s
                       JOIN track t ON t.id = s.track_id
                       LEFT JOIN release r ON r.id = t.release_id
                      WHERE s.plays > 0
                      ORDER BY s.plays DESC, s.completes DESC, s.last DESC
                      LIMIT ?3"
                ))?;
                rep.top_tracks = stmt.query_map(params![from_ms, to_ms, limit], stat_track)?.collect::<rusqlite::Result<_>>()?;
            }
            {
                let mut stmt = c.prepare_cached(
                    "SELECT a.id, coalesce(a.name, x.display), x.plays FROM
                       (SELECT (SELECT ta.artist_id FROM track_artist ta WHERE ta.track_id = e.track_id ORDER BY ta.position LIMIT 1) AS artist,
                               MAX(t.artist_display) AS display, COUNT(*) AS plays
                          FROM event e JOIN track t ON t.id = e.track_id
                         WHERE e.type = 'start' AND e.ts >= ?1 AND e.ts < ?2
                         GROUP BY artist, CASE WHEN artist IS NULL THEN t.artist_display END) x
                       LEFT JOIN artist a ON a.id = x.artist
                      ORDER BY x.plays DESC LIMIT ?3",
                )?;
                rep.top_artists = stmt
                    .query_map(params![from_ms, to_ms, limit], |r| {
                        let id: Option<String> = r.get(0)?;
                        Ok(StatArtist { id: id.and_then(|s| s.parse().ok()), name: r.get(1)?, plays: u(r.get(2)?) })
                    })?
                    .collect::<rusqlite::Result<_>>()?;
            }
            {
                let mut stmt = c.prepare_cached(
                    "SELECT r.id, r.title, r.artist_display, COUNT(*) AS plays
                       FROM event e JOIN track t ON t.id = e.track_id JOIN release r ON r.id = t.release_id
                      WHERE e.type = 'start' AND e.ts >= ?1 AND e.ts < ?2
                      GROUP BY r.id ORDER BY plays DESC LIMIT ?3",
                )?;
                rep.top_albums = stmt
                    .query_map(params![from_ms, to_ms, limit], |r| {
                        Ok(StatAlbum { id: get_id(r, 0)?, title: r.get(1)?, artist: r.get(2)?, plays: u(r.get(3)?) })
                    })?
                    .collect::<rusqlite::Result<_>>()?;
            }
            {
                let mut stmt = c.prepare_cached(
                    "SELECT ((ts / 1000 + ?3) % 86400 + 86400) % 86400 / 3600 AS h, COUNT(*)
                       FROM event WHERE type = 'start' AND ts >= ?1 AND ts < ?2 GROUP BY h",
                )?;
                let rows = stmt.query_map(params![from_ms, to_ms, tz_offset_s], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
                for row in rows {
                    let (h, n) = row?;
                    if let Some(slot) = usize::try_from(h).ok().and_then(|h| rep.by_hour.get_mut(h)) {
                        *slot = u(n);
                    }
                }
            }
            for (col, out) in [("device", &mut rep.by_device), ("context", &mut rep.by_context)] {
                let mut stmt = c.prepare(&format!(
                    "SELECT {col}, COUNT(*) AS n FROM event WHERE type = 'start' AND ts >= ?1 AND ts < ?2 GROUP BY {col} ORDER BY n DESC LIMIT 20"
                ))?;
                *out = stmt.query_map(params![from_ms, to_ms], |r| Ok((r.get::<_, String>(0)?, u(r.get(1)?))))?.collect::<rusqlite::Result<_>>()?;
            }
            Ok(rep)
        })
    }

    /// Songs loved once and not played lately: liked or started at least
    /// `min_plays` times, last heard before `before_ms` (a liked song never
    /// played counts once it was liked before then), most played first.
    pub fn forgotten_gems(&self, before_ms: UnixMs, min_plays: u32, limit: u32) -> Result<Vec<StatTrack>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT t.id, t.title, t.artist_display, r.title, coalesce(s.plays, 0), coalesce(s.completes, 0), coalesce(s.skips, 0),
                        s.last, {LIKED_SQL} AS liked
                   FROM track t
                   LEFT JOIN (SELECT track_id, SUM(type = 'start') AS plays, SUM(type = 'complete') AS completes,
                                     SUM(type = 'skip') AS skips, MAX(CASE WHEN type IN ('start', 'complete') THEN ts END) AS last
                                FROM event GROUP BY track_id) s ON s.track_id = t.id
                   LEFT JOIN release r ON r.id = t.release_id
                  WHERE t.deleted_at IS NULL
                    AND (coalesce(s.plays, 0) >= ?2 OR {LIKED_SQL})
                    AND (s.last < ?1 OR (s.last IS NULL AND EXISTS(SELECT 1 FROM likes l WHERE l.target_kind = 'track'
                                                                     AND l.target_id = t.id AND l.deleted = 0 AND l.updated_at < ?1)))
                  ORDER BY coalesce(s.plays, 0) DESC, liked DESC, s.last DESC
                  LIMIT ?3"
            ))?;
            Ok(stmt.query_map(params![before_ms, min_plays, limit], stat_track)?.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// The state of the library's files and texts; `examples` fake lossless
    /// files named at most.
    pub fn library_health(&self, examples: u32) -> Result<LibraryHealth> {
        self.read(|c| {
            let mut h = LibraryHealth {
                tracks: count(c, "SELECT COUNT(*) FROM track WHERE deleted_at IS NULL")?,
                local_tracks: count(c, "SELECT COUNT(*) FROM track WHERE deleted_at IS NULL AND has_local = 1")?,
                albums: count(
                    c,
                    "SELECT COUNT(*) FROM release r WHERE EXISTS(SELECT 1 FROM track t WHERE t.release_id = r.id AND t.deleted_at IS NULL)",
                )?,
                artists: count(
                    c,
                    "SELECT COUNT(*) FROM artist a WHERE EXISTS(SELECT 1 FROM track_artist ta JOIN track t ON t.id = ta.track_id
                                                               WHERE ta.artist_id = a.id AND t.deleted_at IS NULL)",
                )?,
                playlists: count(c, "SELECT COUNT(*) FROM playlist WHERE deleted = 0")?,
                liked_tracks: count(c, "SELECT COUNT(*) FROM likes WHERE target_kind = 'track' AND deleted = 0")?,
                trashed_tracks: count(c, "SELECT COUNT(*) FROM track WHERE deleted_at IS NOT NULL")?,
                without_synced_lyrics: count(
                    c,
                    "SELECT COUNT(*) FROM track t WHERE t.deleted_at IS NULL AND t.has_local = 1
                        AND NOT EXISTS(SELECT 1 FROM lyrics l WHERE l.track_id = t.id AND l.synced IS NOT NULL)
                        AND NOT EXISTS(SELECT 1 FROM lyrics_text x WHERE x.track_id = t.id AND x.body IS NOT NULL)",
                )?,
                masters: count(c, "SELECT COUNT(*) FROM master")?,
                masters_without_replica: count(
                    c,
                    "SELECT COUNT(*) FROM master m WHERE NOT EXISTS(SELECT 1 FROM replica r WHERE r.file_hash = m.file_hash AND r.verified_at IS NOT NULL)",
                )?,
                missing_files: count(
                    c,
                    "SELECT COUNT(*) FROM source s JOIN track t ON t.id = s.track_id
                      WHERE s.kind = 'local_file' AND s.trashed_at IS NULL AND s.available = 0 AND t.deleted_at IS NULL",
                )?,
                ..Default::default()
            };
            h.masters_bytes = c.query_row("SELECT coalesce(SUM(size_bytes), 0) FROM master", [], |r| r.get(0))?;
            h.local_ms = c.query_row(
                "SELECT coalesce(SUM(duration_ms), 0) FROM track WHERE deleted_at IS NULL AND has_local = 1",
                [],
                |r| r.get(0),
            )?;
            // The file each song plays from.
            let mut stmt = c.prepare_cached(
                "SELECT s.codec, COUNT(*), coalesce(SUM(s.size_bytes), 0) FROM source s JOIN track t ON t.id = s.track_id
                  WHERE s.kind = 'local_file' AND s.is_active = 1 AND s.trashed_at IS NULL AND t.deleted_at IS NULL
                  GROUP BY s.codec",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)))?;
            for row in rows {
                let (codec, files, bytes) = row?;
                let group = format_group(&codec);
                h.local_bytes += bytes;
                match h.formats.iter_mut().find(|f| f.group == group) {
                    Some(f) => {
                        f.files += u(files);
                        f.bytes += bytes;
                    }
                    None => h.formats.push(FormatCount { group: group.to_owned(), files: u(files), bytes }),
                }
            }
            h.formats.sort_by(|a, b| b.bytes.cmp(&a.bytes));
            const FAKES: &str = "FROM source_quality q JOIN source s ON s.id = q.source_id JOIN track t ON t.id = s.track_id
                  WHERE q.verdict IN ('transcoded', 'upsampled') AND s.kind = 'local_file' AND s.trashed_at IS NULL
                    AND t.deleted_at IS NULL";
            h.fake_lossless = count(c, &format!("SELECT COUNT(DISTINCT s.id) {FAKES}"))?;
            let mut stmt = c.prepare(&format!("SELECT t.id, t.title, t.artist_display, q.verdict, q.origin {FAKES} ORDER BY t.artist_key, t.title_key LIMIT ?1"))?;
            h.fake_lossless_examples = stmt
                .query_map([examples], |r| {
                    Ok(FakeLossless { track: get_id(r, 0)?, title: r.get(1)?, artist: r.get(2)?, verdict: r.get(3)?, origin: r.get(4)? })
                })?
                .collect::<rusqlite::Result<_>>()?;
            Ok(h)
        })
    }
}

#[cfg(test)]
mod tests {
    use nikgob_core::model::{Artist, ArtistCredit, Codec, DeviceId, Event, EventType, LikeTarget, Release, Source, SourceKind, Track};

    use super::*;

    const DAY: i64 = 24 * 3600 * 1000;

    fn song(lib: &Library, a: &Artist, rel: &Release, title: &str, ms: u32, codec: Codec) -> Track {
        let mut t = Track::new(title, ArtistCredit::single(a.name.clone(), Some(a.id)));
        t.release_id = Some(rel.id);
        t.duration_ms = Some(ms);
        let t = lib.upsert_track(&t).unwrap();
        let mut s = Source::new(t.id, SourceKind::LocalFile, format!("/m/{title}.x"), codec);
        s.size_bytes = Some(1_000);
        lib.add_source(&s.with_quality(false)).unwrap();
        t
    }

    fn play(lib: &Library, t: &Track, ts: i64, end: EventType, at: u32) {
        let dev = DeviceId("pc".into());
        lib.record_event_raw(&Event::new(t.id, EventType::Start, ts, dev.clone())).unwrap();
        let mut e = Event::new(t.id, end, ts + i64::from(at), dev);
        e.position_ms = Some(at);
        lib.record_event_raw(&e).unwrap();
    }

    #[test]
    fn a_period_has_its_plays_skips_and_time() {
        let lib = Library::open_in_memory().unwrap();
        let a = lib.upsert_artist(&Artist::new("Кино")).unwrap();
        let rel = lib.upsert_release(&Release::new("Группа крови", ArtistCredit::single("Кино", Some(a.id)))).unwrap();
        let x = song(&lib, &a, &rel, "Группа крови", 200_000, Codec::Flac);
        let y = song(&lib, &a, &rel, "Закрой за мной дверь", 180_000, Codec::Mp3);
        let t0 = 100 * DAY;
        for i in 0..3 {
            play(&lib, &x, t0 + i * 3_600_000, EventType::Complete, 200_000);
        }
        play(&lib, &y, t0 + 10 * 3_600_000, EventType::Skip, 10_000);
        // Outside the period.
        play(&lib, &y, t0 - 5 * DAY, EventType::Complete, 180_000);
        let r = lib.listening_report(t0, t0 + DAY, 10, 0).unwrap();
        assert_eq!((r.plays, r.completes, r.skips, r.early_skips, r.distinct_tracks), (4, 3, 1, 1, 2));
        assert_eq!(r.listened_ms, 3 * 200_000 + 10_000);
        assert_eq!(r.top_tracks[0].id, x.id);
        assert_eq!(r.top_tracks[0].plays, 3);
        assert_eq!(r.top_artists[0].name, "Кино");
        assert_eq!(r.top_artists[0].plays, 4);
        assert_eq!(r.top_albums[0].id, rel.id);
        assert_eq!(r.by_hour.iter().sum::<u32>(), 4);
        assert_eq!(r.by_hour[0], 1);
        assert_eq!(r.by_device, vec![("pc".to_owned(), 4)]);
        // Shifted by the local offset (+3 h).
        let r = lib.listening_report(t0, t0 + DAY, 10, 3 * 3600).unwrap();
        assert_eq!(r.by_hour[3], 1);
        assert_eq!(r.by_hour[13], 1);
    }

    #[test]
    fn a_song_loved_long_ago_and_not_lately_is_a_gem() {
        let lib = Library::open_in_memory().unwrap();
        let a = lib.upsert_artist(&Artist::new("A")).unwrap();
        let rel = lib.upsert_release(&Release::new("R", ArtistCredit::single("A", Some(a.id)))).unwrap();
        let old = song(&lib, &a, &rel, "Old", 100_000, Codec::Opus);
        let fresh = song(&lib, &a, &rel, "Fresh", 100_000, Codec::Opus);
        let rare = song(&lib, &a, &rel, "Rare", 100_000, Codec::Opus);
        let now = 400 * DAY;
        for i in 0..6 {
            play(&lib, &old, now - 200 * DAY + i, EventType::Complete, 100_000);
            play(&lib, &fresh, now - DAY + i, EventType::Complete, 100_000);
        }
        play(&lib, &rare, now - 200 * DAY, EventType::Complete, 100_000);
        let gems = lib.forgotten_gems(now - 90 * DAY, 5, 10).unwrap();
        assert_eq!(gems.iter().map(|g| g.id).collect::<Vec<_>>(), vec![old.id]);
        assert_eq!(gems[0].plays, 6);
        // A liked song played once long ago is one too.
        lib.like(LikeTarget::Track(rare.id)).unwrap();
        let gems = lib.forgotten_gems(now - 90 * DAY, 5, 10).unwrap();
        assert!(gems.iter().any(|g| g.id == rare.id && g.liked));
    }

    #[test]
    fn the_health_counts_files_by_format() {
        let lib = Library::open_in_memory().unwrap();
        let a = lib.upsert_artist(&Artist::new("A")).unwrap();
        let rel = lib.upsert_release(&Release::new("R", ArtistCredit::single("A", Some(a.id)))).unwrap();
        song(&lib, &a, &rel, "One", 100_000, Codec::Flac);
        song(&lib, &a, &rel, "Two", 100_000, Codec::Mp3);
        song(&lib, &a, &rel, "Three", 100_000, Codec::Mp3);
        let h = lib.library_health(5).unwrap();
        assert_eq!((h.tracks, h.local_tracks, h.albums, h.artists), (3, 3, 1, 1));
        assert_eq!(h.local_bytes, 3_000);
        assert_eq!(h.local_ms, 300_000);
        assert_eq!(h.without_synced_lyrics, 3);
        assert_eq!(h.fake_lossless, 0);
        assert_eq!(h.masters_without_replica, 0);
        let mp3 = h.formats.iter().find(|f| f.group == "mp3").unwrap();
        assert_eq!((mp3.files, mp3.bytes), (2, 2_000));
        assert_eq!(h.formats.iter().find(|f| f.group == "lossless").map(|f| f.files), Some(1));
        assert_eq!(format_group("FLAC"), "lossless");
        assert_eq!(format_group("opus"), "opus");
        assert_eq!(format_group("xyz"), "other");
    }
}

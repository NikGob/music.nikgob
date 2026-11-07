//! A wished album gathered from several sources keeps the places of its
//! tracklist (`wished_album_places_fixed.v1`).
//!
//! The files of one «Хочу скачать» album may come from several sources
//! (a Soulseek folder of a bigger edition, YouTube, a torrent): each file
//! used to bring its own numbers into the album, so it listed two tracks
//! at 3, tracks without a number first and a bonus song at 11 of 8. The
//! importer now numbers such files from the album; this repair puts the
//! albums already broken this way back in order, once:
//!
//! * [`Library::wished_albums_out_of_place`]: the albums of «Хочу скачать»
//!   items with a file and a track without a number, two tracks at one
//!   place, or a track past the album's count;
//! * [`Library::place_album_tracks`]: the tracklist of the album in the
//!   service it is linked to (what the wish was for) gives each track its
//!   place, by the service's track id, then ISRC, then title and length;
//!   a track the tracklist does not have (a bonus song of another edition)
//!   follows the album's last track, in the order it had, as
//!   `edition_places_fixed.v1` places strays. A tracklist that is most
//!   likely another edition (fewer than half of its tracks found) changes
//!   nothing.
//!
//! The runtime fetches the tracklists, keeps the mark and backs the library
//! up before the first change; the files follow through the importer's
//! retag.

use std::collections::HashSet;

use nikgob_core::model::{ReleaseId, TrackId};
use nikgob_core::streaming::{EntityKind, StreamAlbum};
use rusqlite::{Connection, params};

use crate::db::Library;
use crate::error::Result;
use crate::streams::{SLACK_MS, albums_agree, linked, normal_isrc, parse, song_key};

/// `meta` key of the repair.
pub const PLACES_FROM_WISH_KEY: &str = "wished_album_places_fixed.v1";

/// One track the repair moves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaceChange {
    pub track: TrackId,
    pub title: String,
    /// `(disc, number)` before and after.
    pub from: (u32, u32),
    pub to: (u32, u32),
    /// It has a file (whose tags and name follow).
    pub has_local: bool,
}

/// A live track of the album as [`plan_places`] sees it.
#[derive(Clone, Debug)]
pub(crate) struct Row {
    pub id: String,
    pub title: String,
    pub place: (u32, u32),
    pub duration_ms: Option<i64>,
    pub isrc: Option<String>,
    /// In the library or with a file: a line the user has.
    pub kept: bool,
    pub has_local: bool,
}

/// A line of the service's tracklist.
#[derive(Clone, Debug)]
pub(crate) struct Line {
    pub place: (u32, u32),
    pub title: String,
    pub duration_ms: Option<i64>,
    pub isrc: Option<String>,
    /// The library track the service's track id is linked to.
    pub linked: Option<String>,
}

fn close(a: Option<i64>, b: Option<i64>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => (a - b).abs() <= SLACK_MS,
        _ => true,
    }
}

/// Each row's place by `lines` (see the module docs): `(row index, place)`
/// for the rows whose place changes; `None` when the tracklist is most
/// likely another edition.
pub(crate) fn plan_places(rows: &[Row], lines: &[Line]) -> Option<Vec<(usize, (u32, u32))>> {
    let mut of_row: Vec<Option<usize>> = vec![None; rows.len()];
    let mut used = vec![false; lines.len()];
    let mut take = |fits: &dyn Fn(&Row, &Line) -> bool| {
        for (i, r) in rows.iter().enumerate() {
            if of_row[i].is_some() {
                continue;
            }
            if let Some(j) = (0..lines.len()).find(|&j| !used[j] && fits(r, &lines[j])) {
                of_row[i] = Some(j);
                used[j] = true;
            }
        }
    };
    // The service's own id, then the ISRC, then the title and length, then
    // one title the other with more words (a remix's name) and the length.
    take(&|r, l| l.linked.as_deref() == Some(r.id.as_str()));
    take(&|r, l| {
        let (a, b) = (normal_isrc(r.isrc.as_deref()), normal_isrc(l.isrc.as_deref()));
        a.is_some() && a == b
    });
    take(&|r, l| {
        let k = song_key(&r.title);
        !k.is_empty() && k == song_key(&l.title) && close(r.duration_ms, l.duration_ms)
    });
    take(&|r, l| {
        let (a, b) = (song_key(&r.title), song_key(&l.title));
        !a.is_empty() && albums_agree(&a, &b) && r.duration_ms.is_some() && close(r.duration_ms, l.duration_ms)
    });
    let found = used.iter().filter(|u| **u).count();
    if lines.is_empty() || found * 2 < lines.len() {
        return None;
    }
    let mut out = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        if let Some(j) = of_row[i]
            && lines[j].place != r.place
        {
            out.push((i, lines[j].place));
        }
    }
    // The rest the user has follow the album's last line, in their order.
    let last_disc = lines.iter().map(|l| l.place.0).max().unwrap_or(1);
    let mut next = lines.iter().filter(|l| l.place.0 == last_disc).map(|l| l.place.1).max().unwrap_or(0);
    let mut rest: Vec<usize> = (0..rows.len()).filter(|&i| of_row[i].is_none() && rows[i].kept).collect();
    rest.sort_by_key(|&i| (rows[i].place.0, rows[i].place.1 == 0, rows[i].place.1, i));
    for i in rest {
        next += 1;
        if rows[i].place != (last_disc, next) {
            out.push((i, (last_disc, next)));
        }
    }
    Some(out)
}

fn rows_of(conn: &Connection, release: &str) -> Result<Vec<Row>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, title, max(disc_no, 1), track_no, duration_ms, isrc, in_library = 1 OR has_local = 1, has_local
           FROM track WHERE release_id = ?1 AND deleted_at IS NULL
          ORDER BY max(disc_no, 1), track_no = 0, track_no, seq",
    )?;
    let rows = stmt.query_map([release], |r| {
        Ok(Row {
            id: r.get(0)?,
            title: r.get(1)?,
            place: (r.get::<_, i64>(2)? as u32, r.get::<_, i64>(3)?.max(0) as u32),
            duration_ms: r.get(4)?,
            isrc: r.get(5)?,
            kept: r.get(6)?,
            has_local: r.get(7)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

impl Library {
    /// The albums of «Хочу скачать» items whose places are broken (see the
    /// module docs), oldest first.
    pub fn wished_albums_out_of_place(&self) -> Result<Vec<ReleaseId>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT r.id FROM release r
                  WHERE EXISTS (SELECT 1 FROM wishlist w WHERE w.release_id = r.id AND w.scope = 'album')
                    AND EXISTS (SELECT 1 FROM track t WHERE t.release_id = r.id AND t.deleted_at IS NULL AND t.has_local = 1)
                    AND (EXISTS (SELECT 1 FROM track t WHERE t.release_id = r.id AND t.deleted_at IS NULL
                                    AND (t.in_library = 1 OR t.has_local = 1)
                                    AND (t.track_no = 0 OR t.track_no > coalesce(r.total_tracks, t.track_no)))
                         OR EXISTS (SELECT 1 FROM track a JOIN track b ON b.release_id = a.release_id AND b.id != a.id
                                       AND max(b.disc_no, 1) = max(a.disc_no, 1) AND b.track_no = a.track_no
                                     WHERE a.release_id = r.id AND a.deleted_at IS NULL AND b.deleted_at IS NULL
                                       AND a.track_no > 0 AND (a.in_library = 1 OR a.has_local = 1)
                                       AND (b.in_library = 1 OR b.has_local = 1)))
                  ORDER BY r.seq",
            )?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            let ids: Vec<String> = rows.collect::<rusqlite::Result<_>>()?;
            ids.iter().map(|id| parse(id)).collect()
        })
    }

    /// Puts the tracks of `release` at their places on `album`, its
    /// tracklist in a service (see the module docs); the album counts the
    /// tracklist's lines. Returns what moved (nothing for another edition).
    pub fn place_album_tracks(&self, release: ReleaseId, album: &StreamAlbum) -> Result<Vec<PlaceChange>> {
        self.write(|tx| {
            let rid = release.to_string();
            let rows = rows_of(tx, &rid)?;
            let service = album.id.service;
            let mut lines = Vec::with_capacity(album.tracks.len());
            for (i, t) in album.tracks.iter().enumerate() {
                lines.push(Line {
                    place: (t.disc_no.unwrap_or(1).max(1), t.track_no.unwrap_or(i as u32 + 1)),
                    title: t.title.clone(),
                    duration_ms: t.duration_ms.and_then(|d| i64::try_from(d).ok()),
                    isrc: t.isrc.clone(),
                    linked: linked(tx, "track", service, EntityKind::Track, &t.id.id)?,
                });
            }
            // Two lines at one place: no tracklist to go by.
            let places: HashSet<(u32, u32)> = lines.iter().map(|l| l.place).collect();
            if places.len() < lines.len() {
                return Ok(Vec::new());
            }
            let Some(plan) = plan_places(&rows, &lines) else {
                note!("library: «{rid}»: the tracklist of {} {} is another edition, places stay", service.as_str(), album.id.id);
                return Ok(Vec::new());
            };
            let title: String = tx.query_row("SELECT title FROM release WHERE id = ?1", [&rid], |r| r.get(0))?;
            let mut out = Vec::new();
            for (i, to) in plan {
                let r = &rows[i];
                tx.execute("UPDATE track SET disc_no = ?2, track_no = ?3 WHERE id = ?1", params![r.id, to.0, to.1])?;
                note!("library: «{title}»: «{}» {}-{} → {}-{}", r.title, r.place.0, r.place.1, to.0, to.1);
                out.push(PlaceChange { track: parse(&r.id)?, title: r.title.clone(), from: r.place, to, has_local: r.has_local });
            }
            tx.execute("UPDATE release SET total_tracks = ?2 WHERE id = ?1", params![rid, lines.len() as i64])?;
            Ok(out)
        })
    }
}


#[cfg(test)]
mod tests {
    use nikgob_core::model::{
        ArtistCredit, Codec, Release, Source, SourceKind, Track, WishlistItem, WishlistScope, WishlistStatus,
    };
    use nikgob_core::streaming::{ArtistRef, RemoteId, Service, StreamTrack};

    use super::*;

    /// «8 способов как бросить...» of Пошлая Молли as the Deezer album it
    /// was wished from: `(deezer id, title, ms)` at places 1..8.
    const DEEZER: &[(&str, &str, u64)] = &[
        ("3949282011", "Любимая песня твоей сестры", 188_000),
        ("3949282021", "Даже моя бэйби не знает", 234_336),
        ("3949282031", "Нон стоп (Рефлекс ремикс)", 213_786),
        ("3949282041", "Ханнамонтана", 215_824),
        ("3949282051", "Супермаркет", 214_256),
        ("3949282061", "Паки пуси", 143_360),
        ("3949282071", "Молли", 228_884),
        ("3949282081", "Тмстс", 160_313),
    ];

    fn album() -> StreamAlbum {
        let artists = vec![ArtistRef::new(None, "Пошлая Молли")];
        StreamAlbum {
            id: RemoteId::new(Service::Deezer, "957193101"),
            title: "8 способов как бросить...".into(),
            artists: artists.clone(),
            year: Some(2017),
            release_date: None,
            release_type: Default::default(),
            cover_url: None,
            total_tracks: Some(8),
            tracks: DEEZER
                .iter()
                .enumerate()
                .map(|(i, (id, title, ms))| {
                    let mut t = StreamTrack::new(RemoteId::new(Service::Deezer, *id), *title);
                    t.artists = artists.clone();
                    t.track_no = Some(i as u32 + 1);
                    t.duration_ms = Some(*ms);
                    t
                })
                .collect(),
        }
    }

    /// The album as the files of several sources left it: two tracks at 3,
    /// two without a number, two bonus songs of another edition at 10 and 11.
    fn broken(lib: &Library) -> ReleaseId {
        let credit = ArtistCredit::single("Пошлая Молли", None);
        let mut r = Release::new("8 способов как бросить...", credit.clone());
        r.total_tracks = Some(8);
        let release = lib.upsert_release(&r).unwrap().id;
        lib.set_external_id(EntityKind::Album, &RemoteId::new(Service::Deezer, "957193101"), release.0).unwrap();
        let rows: [(&str, u32, Option<&str>, u32, &str); 10] = [
            ("Тмстс", 0, Some("3949282081"), 160_313, "opus"),
            ("Супермаркет", 0, Some("3949282051"), 214_256, "flac"),
            ("Любимая песня твоей сестры", 1, Some("3949282011"), 188_000, "opus"),
            ("Даже моя бэйби не знает", 3, Some("3949282021"), 234_336, "opus"),
            ("Нон стоп", 3, Some("3949282031"), 213_786, "opus"),
            ("Ханнамонтана", 5, Some("3949282041"), 215_824, "opus"),
            ("Паки пуси", 6, Some("3949282061"), 143_360, "opus"),
            ("Молли", 7, Some("3949282071"), 228_884, "flac"),
            ("Сфотографирую", 10, None, 160_479, "opus"),
            ("Lin Ansty", 11, None, 152_790, "flac"),
        ];
        for (title, no, deezer, ms, ext) in rows {
            let mut t = Track::new(title, credit.clone());
            t.release_id = Some(release);
            t.track_no = no;
            t.duration_ms = Some(ms);
            let t = lib.upsert_track(&t).unwrap();
            let codec = if ext == "flac" { Codec::Flac } else { Codec::Opus };
            lib.add_source(&Source::new(t.id, SourceKind::LocalFile, format!("M/8/{no:02} - {title}.{ext}"), codec)).unwrap();
            if let Some(d) = deezer {
                lib.set_external_id(EntityKind::Track, &RemoteId::new(Service::Deezer, d), t.id.0).unwrap();
            }
        }
        release
    }

    fn wish(lib: &Library, release: ReleaseId) {
        let mut w = WishlistItem::new(WishlistStatus::Done, 1);
        w.scope = WishlistScope::Album;
        w.release_id = Some(release);
        lib.put_wishlist_item(&w).unwrap();
    }

    fn places(lib: &Library, release: ReleaseId) -> Vec<String> {
        lib.read(|c| {
            let mut stmt = c.prepare("SELECT track_no, title FROM track WHERE release_id = ?1 ORDER BY disc_no, track_no, seq")?;
            let rows = stmt.query_map([release.to_string()], |r| Ok(format!("{} {}", r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
        .unwrap()
    }

    #[test]
    fn a_wished_album_of_several_sources_takes_the_order_of_its_tracklist() {
        let lib = Library::open_in_memory().unwrap();
        let release = broken(&lib);
        assert!(lib.wished_albums_out_of_place().unwrap().is_empty(), "no wish: not this repair's album");
        wish(&lib, release);
        assert_eq!(lib.wished_albums_out_of_place().unwrap(), [release]);

        let moved = lib.place_album_tracks(release, &album()).unwrap();
        assert_eq!(moved.len(), 6, "{moved:?}");
        assert!(moved.iter().all(|c| c.has_local));
        assert_eq!(
            places(&lib, release),
            [
                "1 Любимая песня твоей сестры",
                "2 Даже моя бэйби не знает",
                "3 Нон стоп",
                "4 Ханнамонтана",
                "5 Супермаркет",
                "6 Паки пуси",
                "7 Молли",
                "8 Тмстс",
                "9 Сфотографирую",
                "10 Lin Ansty",
            ],
            "the bonus songs follow the album, in their order"
        );
        assert_eq!(lib.get_release(release).unwrap().unwrap().total_tracks, Some(8));
        assert!(lib.place_album_tracks(release, &album()).unwrap().is_empty(), "in order: nothing moves again");
    }

    #[test]
    fn another_edition_moves_nothing() {
        let lib = Library::open_in_memory().unwrap();
        let release = broken(&lib);
        wish(&lib, release);
        let mut other = album();
        other.id = RemoteId::new(Service::Youtube, "MPREb_other");
        for (i, t) in other.tracks.iter_mut().enumerate() {
            t.id = RemoteId::new(Service::Youtube, format!("v{i}"));
            t.title = format!("Совсем другая песня {i}");
        }
        assert!(lib.place_album_tracks(release, &other).unwrap().is_empty());
        assert_eq!(places(&lib, release)[0], "0 Тмстс", "places stay");
    }

    #[test]
    fn titles_and_lengths_find_tracks_without_a_link() {
        let rows: Vec<Row> = [("Молли", (1, 3), 228_000), ("ТМСТС", (1, 0), 160_000), ("Бонус", (1, 9), 100_000)]
            .iter()
            .enumerate()
            .map(|(i, (t, p, ms))| Row {
                id: i.to_string(),
                title: (*t).into(),
                place: *p,
                duration_ms: Some(*ms),
                isrc: None,
                kept: true,
                has_local: true,
            })
            .collect();
        let lines: Vec<Line> = [("Тмстс", 160_500), ("Молли", 228_400)]
            .iter()
            .enumerate()
            .map(|(i, (t, ms))| Line { place: (1, i as u32 + 1), title: (*t).into(), duration_ms: Some(*ms), isrc: None, linked: None })
            .collect();
        assert_eq!(plan_places(&rows, &lines).unwrap(), [(0, (1, 2)), (1, (1, 1)), (2, (1, 3))]);
    }
}

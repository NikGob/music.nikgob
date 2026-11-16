use std::sync::atomic::AtomicI64;

use nikgob_core::model::{
    Analysis, AnalysisSource, Artist, ArtistCredit, Codec, EventContext, EventMeta, EventType, Release, Source, SourceKind, Track,
};
use nikgob_core::settings::ApiActor;

use super::*;

fn lib() -> Library {
    Library::open_in_memory().unwrap()
}

fn id(lib: &Library) -> String {
    lib.device_id().as_str().to_owned()
}

/// Takes everything new of `from` into `to`, page by page (small pages),
/// as the loop does. The rows taken.
fn pull(to: &Library, from: &Library) -> u32 {
    let (to_id, from_id) = (id(to), id(from));
    let mut taken = 0;
    for t in TABLES {
        let mut since = to.sync_cursors(&from_id).unwrap().tables.get(t.name).copied().unwrap_or(0);
        loop {
            let page = from.sync_changes(t.name, since, 3, Some(&to_id)).unwrap();
            taken += to.sync_apply(&page, &from_id, true).unwrap().taken;
            since = page.next;
            if !page.more {
                break;
            }
        }
    }
    taken
}

struct Fixture {
    artist: Artist,
    release: Release,
    track: Track,
    stream: Track,
}

fn fill(lib: &Library) -> Fixture {
    let artist = lib.upsert_artist(&Artist::new("Кино")).unwrap();
    let credit = ArtistCredit::single(artist.name.clone(), Some(artist.id));
    let release = lib.upsert_release(&Release::new("Группа крови", credit.clone())).unwrap();
    let mut t = Track::new("Звезда по имени Солнце", credit.clone());
    t.release_id = Some(release.id);
    t.track_no = 2;
    let track = lib.upsert_track(&t).unwrap();
    lib.add_source(&Source::new(track.id, SourceKind::LocalFile, "C:/Music/Кино/02.opus", Codec::Opus).with_quality(false)).unwrap();
    let stream = lib.upsert_track(&Track::new("Кукушка", credit)).unwrap();
    lib.add_source(&Source::new(stream.id, SourceKind::Youtube, "yt-kukushka", Codec::Opus).with_quality(false)).unwrap();
    lib.like(track.id).unwrap();
    let p = lib.create_playlist("Вечер").unwrap();
    lib.playlist_add(p.id, track.id).unwrap();
    lib.playlist_add(p.id, stream.id).unwrap();
    lib.set_track_fx(track.id, Some("slow")).unwrap();
    lib.set_lyrics_pick(track.id, Some("lrclib")).unwrap();
    lib.put_analysis(&Analysis {
        track_id: track.id,
        bliss_vector: (0..23).map(|i| i as f32 * 0.1).collect(),
        analyzed_with: "bliss-test".into(),
        source: AnalysisSource::File,
    })
    .unwrap();
    lib.record_event(track.id, EventType::Start, Some(SourceKind::LocalFile), Some(0), EventContext::Album).unwrap();
    Fixture { artist, release, track, stream }
}

/// The schema has every column the list syncs; the stamps, flags and
/// references the list names are its columns.
#[test]
fn the_list_matches_the_schema() {
    let lib = lib();
    lib.read(|c| {
        for t in TABLES {
            let cols: Vec<String> = t.keys.iter().chain(t.cols.iter()).map(|c| (*c).to_owned()).collect();
            c.prepare(&format!("SELECT {} FROM {} LIMIT 0", cols.join(", "), t.name)).unwrap_or_else(|e| panic!("{}: {e}", t.name));
            for col in t.stamp.iter().chain(t.prefer.iter()) {
                assert!(t.cols.contains(col), "{} lacks {col}", t.name);
            }
            for (col, target) in t.refs {
                assert!(place(t, col).is_some(), "{} lacks {col}", t.name);
                if let Target::ByKind(kind) = target {
                    assert!(place(t, kind).is_some(), "{} lacks {kind}", t.name);
                }
            }
            if t.identity != Identity::None {
                assert_eq!(t.keys, &["id"], "{}", t.name);
            }
        }
        Ok(())
    })
    .unwrap();
    let names: std::collections::HashSet<&str> = TABLES.iter().map(|t| t.name).collect();
    assert_eq!(names.len(), TABLES.len(), "each table once");
}

#[test]
fn the_clock_never_goes_back_and_has_room_within_a_millisecond() {
    let clock = AtomicI64::new(0);
    assert_eq!(hlc_next(&clock, 1_000), version_at(1_000));
    assert_eq!(hlc_next(&clock, 1_000), version_at(1_000) + 1, "the same millisecond: one more");
    assert_eq!(hlc_next(&clock, 900), version_at(1_000) + 2, "the wall clock went back: still on");
    assert_eq!(ms_of(hlc_next(&clock, 1_000)), 1_000);
    // Thousands of changes in one millisecond stay in it.
    for _ in 0..10_000 {
        hlc_next(&clock, 1_000);
    }
    assert_eq!(ms_of(hlc_next(&clock, 1_000)), 1_000);
    assert_eq!(hlc_next(&clock, 1_001), version_at(1_001), "the next millisecond starts over");
    // Another device's row two seconds ahead: the next edit here is later.
    hlc_observe(&clock, version_at(3_000) + 5);
    assert_eq!(hlc_next(&clock, 1_020), version_at(3_000) + 6);
    // An old version changes nothing.
    hlc_observe(&clock, 10);
    assert_eq!(hlc_next(&clock, 1_030), version_at(3_000) + 7);
}

#[test]
fn edits_are_stamped_by_the_clock() {
    let lib = lib();
    let a = lib.tick();
    let b = lib.tick();
    assert!(b > a);
    lib.observe_clock(version_at(now_ms() + 60_000));
    assert!(lib.tick() > now_ms() + 59_000, "past what another device wrote");
    // The triggers read the same clock: a change after it is later still.
    let x = lib.upsert_artist(&Artist::new("X")).unwrap();
    let v: i64 = lib
        .read(|c| Ok(c.query_row("SELECT hlc FROM change_log WHERE tbl = 'artist' AND key = json_array(?1)", [x.id.to_string()], |r| r.get(0))?))
        .unwrap();
    assert!(v > version_at(now_ms() + 59_000));
    let kept: i64 = lib.read(|c| Ok(c.query_row("SELECT hlc FROM sync_clock", [], |r| r.get(0))?)).unwrap();
    assert!(kept >= v, "the clock is kept with the database");
}

/// Every entry of the change log has a number of its own, also the two a
/// key change makes, so a page boundary between them loses neither.
#[test]
fn a_key_change_notes_two_entries_with_numbers_of_their_own() {
    let pc = lib();
    fill(&pc);
    pc.adopt_node_id("node-abc").unwrap();
    let (all, distinct): (i64, i64) =
        pc.read(|c| Ok(c.query_row("SELECT count(*), count(DISTINCT seq) FROM change_log", [], |r| Ok((r.get(0)?, r.get(1)?)))?)).unwrap();
    assert_eq!(all, distinct);
    let mut since = 0;
    let mut rows = Vec::new();
    loop {
        let page = pc.sync_changes("node_file", since, 1, None).unwrap();
        rows.extend(page.rows.iter().map(|r| (r.k.clone(), r.v.2)));
        since = page.next;
        if !page.more {
            break;
        }
    }
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(rows[0].1 && !rows[1].1, "the old key gone, then the new one: {rows:?}");
}

#[test]
fn base64_round_trips() {
    for n in 0..20usize {
        let data: Vec<u8> = (0..n).map(|i| (i * 37 % 256) as u8).collect();
        let s = b64_encode(&data);
        assert_eq!(s.len() % 4, 0);
        assert_eq!(b64_decode(&s).unwrap(), data, "{n}");
    }
    assert_eq!(b64_encode(b"Man"), "TWFu");
    assert_eq!(b64_encode(b"Ma"), "TWE=");
    assert_eq!(b64_decode("TQ==").unwrap(), b"M");
    assert!(b64_decode("TQ=").is_none());
    assert!(b64_decode("T=Q=").is_none());
}

/// A change is noted once per row, with a number past every other; a
/// deleted row stays as gone.
#[test]
fn changes_come_in_pages_and_deletes_stay_as_tombstones() {
    let lib = lib();
    let artists: Vec<Artist> = (0..7).map(|i| lib.upsert_artist(&Artist::new(format!("A{i}"))).unwrap()).collect();
    let heads = lib.sync_heads().unwrap();
    assert!(!heads.epoch.is_empty());
    let head = heads.tables["artist"];
    let mut since = 0;
    let mut seen = Vec::new();
    let mut pages = 0;
    loop {
        let page = lib.sync_changes("artist", since, 3, None).unwrap();
        pages += 1;
        assert!(page.rows.len() <= 3);
        assert!(page.next > since || page.rows.is_empty());
        for r in &page.rows {
            let k: Vec<String> = serde_json::from_str(&r.k).unwrap();
            seen.push(k[0].clone());
            assert!(!r.v.2);
            assert_eq!(page.cols.len(), r.d.as_ref().unwrap().len());
        }
        since = page.next;
        if !page.more {
            break;
        }
    }
    assert!(pages >= 3);
    assert_eq!(since, head);
    let ids: Vec<String> = artists.iter().map(|a| a.id.to_string()).collect();
    assert_eq!(seen, ids, "oldest first, each once");
    // Nothing new: an empty page.
    let page = lib.sync_changes("artist", head, 3, None).unwrap();
    assert!(page.rows.is_empty() && !page.more);
    // A delete: the row comes again, gone, after the others.
    lib.write(|tx| {
        tx.execute("DELETE FROM artist WHERE id = ?1", [artists[2].id.to_string()])?;
        Ok(())
    })
    .unwrap();
    let page = lib.sync_changes("artist", head, 10, None).unwrap();
    assert_eq!(page.rows.len(), 1);
    assert!(page.rows[0].v.2 && page.rows[0].d.is_none());
    // A change of a column that does not travel is not noted.
    let before = lib.sync_heads().unwrap().tables["track"];
    let credit = ArtistCredit::single("A0".to_owned(), Some(artists[0].id));
    let t = lib.upsert_track(&Track::new("T", credit)).unwrap();
    let after_insert = lib.sync_heads().unwrap().tables["track"];
    assert!(after_insert > before);
    lib.write(|tx| {
        tx.execute("UPDATE track SET has_local = 1 WHERE id = ?1", [t.id.to_string()])?;
        Ok(())
    })
    .unwrap();
    assert_eq!(lib.sync_heads().unwrap().tables["track"], after_insert);
}

/// A new device starts empty: everything comes, under the same ids, but
/// not the paths of the files (only which device has them).
#[test]
fn a_first_sync_fills_an_empty_library_with_the_same_ids() {
    let pc = lib();
    let pi = lib();
    let f = fill(&pc);
    assert!(pull(&pi, &pc) > 10);
    let t = pi.get_track(f.track.id).unwrap().expect("the track");
    assert_eq!(t.release_id, Some(f.release.id));
    assert_eq!(pi.get_release(f.release.id).unwrap().unwrap().title, "Группа крови");
    assert_eq!(pi.get_artist(f.artist.id).unwrap().unwrap().name, "Кино");
    assert!(pi.is_liked(f.track.id).unwrap());
    assert_eq!(pi.sync_conflicts(&id(&pc)).unwrap(), SyncConflicts::default());
    let lists = pi.list_playlists().unwrap();
    let mine = lists.iter().find(|p| p.name == "Вечер").expect("the playlist");
    assert_eq!(mine.track_count, 2);
    assert_eq!(pi.track_fx().unwrap(), vec![(f.track.id, "slow".to_owned())]);
    assert_eq!(pi.lyrics_pick(f.track.id).unwrap().as_deref(), Some("lrclib"));
    assert_eq!(pi.analysis_vectors("bliss-test").unwrap().len(), 1);
    assert_eq!(pi.listening_events().unwrap().len(), 1);
    // The stream's source came; the file's did not, its presence did.
    let sources = pi.sources_of_track(f.stream.id).unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].kind, SourceKind::Youtube);
    assert!(pi.sources_of_track(f.track.id).unwrap().is_empty());
    let files: Vec<(String, String)> = pi
        .read(|c| {
            let mut s = c.prepare("SELECT node_id, track_id FROM node_file")?;
            let rows = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
        .unwrap();
    assert_eq!(files, vec![(id(&pc), f.track.id.to_string())]);
    // The search index follows the rows that came.
    let indexed: i64 = pi.read(|c| Ok(c.query_row("SELECT count(*) FROM track_fts", [], |r| r.get(0))?)).unwrap();
    assert_eq!(indexed, 2);
}

/// Merging twice, or back, changes nothing more; what came from a device
/// is not offered back to it.
#[test]
fn merging_is_idempotent_and_nothing_echoes_back() {
    let pc = lib();
    let pi = lib();
    fill(&pc);
    let first = pull(&pi, &pc);
    assert!(first > 0);
    // The same pages again, from the start.
    pi.sync_start_over(&id(&pc), "same", &[]).unwrap();
    assert_eq!(pull(&pi, &pc), 0, "already here");
    // Back to the PC: nothing (all of it came from there).
    assert_eq!(pull(&pc, &pi), 0);
    for t in TABLES {
        let page = pi.sync_changes(t.name, 0, 2_000, Some(&id(&pc))).unwrap();
        assert!(page.rows.is_empty(), "{} offered back", t.name);
    }
}

/// Edits on both sides in any order end the same on both: the later wins,
/// a delete on a tie.
#[test]
fn concurrent_edits_converge() {
    let pc = lib();
    let pi = lib();
    let f = fill(&pc);
    pull(&pi, &pc);
    // The PC unlikes, the Pi (later) likes again; the Pi clears the speed,
    // the PC (later) sets another one.
    pc.unlike(f.track.id).unwrap();
    pi.set_track_fx(f.track.id, None).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    pi.like(f.stream.id).unwrap();
    pi.unlike(f.track.id).unwrap();
    pi.like(f.track.id).unwrap();
    pc.set_track_fx(f.track.id, Some("fast")).unwrap();
    pull(&pi, &pc);
    pull(&pc, &pi);
    pull(&pi, &pc);
    for lib in [&pc, &pi] {
        assert!(lib.is_liked(f.track.id).unwrap(), "the Pi's like was later");
        assert!(lib.is_liked(f.stream.id).unwrap());
        assert_eq!(lib.track_fx().unwrap(), vec![(f.track.id, "fast".to_owned())], "the PC's speed was later");
    }
    // A speed cleared syncs as cleared.
    pi.set_track_fx(f.track.id, None).unwrap();
    pull(&pc, &pi);
    assert!(pc.track_fx().unwrap().is_empty());
    // Applied in the other order, the same page gives the same result.
    let a = lib();
    let b = lib();
    let ta = a.upsert_artist(&Artist::new("X")).unwrap();
    pull(&b, &a);
    let show = |l: &Library, how: &str| {
        l.write(|tx| {
            tx.execute("INSERT OR REPLACE INTO lyrics_show(track_id, sync) VALUES (?1, ?2)", params![ta.id.to_string(), how])?;
            Ok(())
        })
        .unwrap();
    };
    show(&a, "word");
    std::thread::sleep(std::time::Duration::from_millis(5));
    show(&b, "line");
    let pa = a.sync_changes("lyrics_show", 0, 10, None).unwrap();
    let pb = b.sync_changes("lyrics_show", 0, 10, None).unwrap();
    let c1 = lib();
    let c2 = lib();
    c1.sync_apply(&pa, &id(&a), false).unwrap();
    c1.sync_apply(&pb, &id(&b), false).unwrap();
    c2.sync_apply(&pb, &id(&b), false).unwrap();
    c2.sync_apply(&pa, &id(&a), false).unwrap();
    let shown = |l: &Library| -> String {
        l.read(|c| Ok(c.query_row("SELECT sync FROM lyrics_show", [], |r| r.get(0))?)).unwrap()
    };
    assert_eq!(shown(&c1), shown(&c2));
    assert_eq!(shown(&c1), "line", "the later one");
}

/// A track deleted on one device goes on the other only while nothing
/// there uses it (a file of its own keeps it).
#[test]
fn a_catalog_delete_spares_what_is_still_used() {
    let pc = lib();
    let pi = lib();
    let f = fill(&pc);
    pull(&pi, &pc);
    let pc_id = id(&pc);
    // The Pi has its own file of the stream track.
    pi.add_source(&Source::new(f.stream.id, SourceKind::LocalFile, "/mnt/ssd/Кукушка.opus", Codec::Opus).with_quality(false)).unwrap();
    pc.write(|tx| {
        for tid in [f.track.id.to_string(), f.stream.id.to_string()] {
            tx.execute("DELETE FROM source WHERE track_id = ?1", [&tid])?;
            for table in ["track_artist", "lyrics_pick", "analysis", "track_fx"] {
                tx.execute(&format!("DELETE FROM {table} WHERE track_id = ?1"), [&tid])?;
            }
            tx.execute("UPDATE playlist_entry SET deleted = 1 WHERE track_id = ?1", [&tid])?;
            tx.execute("DELETE FROM track WHERE id = ?1", [&tid])?;
        }
        Ok(())
    })
    .unwrap();
    pull(&pi, &pc);
    assert!(pi.get_track(f.track.id).unwrap().is_none(), "nothing kept it");
    assert!(pi.get_track(f.stream.id).unwrap().is_some(), "its own file keeps it");
    assert_eq!(pi.sync_cursors(&pc_id).unwrap().tables.get("track").copied(), pc.sync_heads().unwrap().tables.get("track").copied());
    // The delete held back is kept, and goes once the file does.
    assert_eq!(pi.sync_conflicts(&pc_id).unwrap(), SyncConflicts { held: 1, ..SyncConflicts::default() });
    pi.write(|tx| {
        tx.execute("DELETE FROM source WHERE track_id = ?1", [f.stream.id.to_string()])?;
        Ok(())
    })
    .unwrap();
    assert_eq!(pi.sync_retry().unwrap(), 0, "not due yet");
    assert_eq!(pi.sync_retry_at(now_ms() + 2 * RETRY_MAX_MS).unwrap(), 1);
    assert!(pi.get_track(f.stream.id).unwrap().is_none());
    assert_eq!(pi.sync_conflicts(&pc_id).unwrap(), SyncConflicts::default());
}

/// An artist deleted with its last link: the delete waits while the link
/// is here, and goes when the link's own delete comes later in the pull.
#[test]
fn a_held_delete_goes_when_what_held_it_goes() {
    let pc = lib();
    let pi = lib();
    let f = fill(&pc);
    let guest = pc.upsert_artist(&Artist::new("Гость")).unwrap();
    pc.write(|tx| {
        tx.execute("INSERT INTO track_artist(track_id, artist_id, position) VALUES (?1, ?2, 1)", params![f.track.id.to_string(), guest.id.to_string()])?;
        Ok(())
    })
    .unwrap();
    pull(&pi, &pc);
    assert!(pi.get_artist(guest.id).unwrap().is_some());
    pc.write(|tx| {
        tx.execute("DELETE FROM track_artist WHERE artist_id = ?1", [guest.id.to_string()])?;
        tx.execute("DELETE FROM artist WHERE id = ?1", [guest.id.to_string()])?;
        Ok(())
    })
    .unwrap();
    pull(&pi, &pc);
    assert!(pi.get_artist(guest.id).unwrap().is_none(), "tried again after the link's page");
    assert_eq!(pi.sync_conflicts(&id(&pc)).unwrap(), SyncConflicts::default());
}

/// A row that cannot be written yet (its track has not come) is kept, the
/// cursor passes it, and it goes through after the page that brings what
/// it needs; the stream it is then plays.
#[test]
fn a_row_that_fails_is_kept_and_tried_again() {
    let pc = lib();
    let pi = lib();
    let f = fill(&pc);
    let pc_id = id(&pc);
    let page = pc.sync_changes("source", 0, 100, Some(&id(&pi))).unwrap();
    let done = pi.sync_apply(&page, &pc_id, true).unwrap();
    assert_eq!((done.taken, done.waiting), (0, 1));
    assert_eq!(pi.sync_conflicts(&pc_id).unwrap(), SyncConflicts { waiting: 1, ..SyncConflicts::default() });
    assert_eq!(pi.sync_cursors(&pc_id).unwrap().tables.get("source").copied(), Some(page.next), "kept, so passed");
    pull(&pi, &pc);
    assert_eq!(pi.sync_conflicts(&pc_id).unwrap(), SyncConflicts::default());
    let sources = pi.sources_of_track(f.stream.id).unwrap();
    assert_eq!(sources.len(), 1);
    let active: i64 = pi.read(|c| Ok(c.query_row("SELECT count(*) FROM source WHERE is_active = 1", [], |r| r.get(0))?)).unwrap();
    assert_eq!(active, 1, "the stream is the track's playing source");
}

/// Two libraries built apart with the same artist and album under other
/// ids: nothing is merged on a guess. The artist and the album are kept as
/// identity conflicts for the owner (never tried again on their own), what
/// points at them waits, nothing here changes; a newer version that no
/// longer clashes goes through.
#[test]
fn the_same_artist_or_album_under_another_id_waits_for_the_owner() {
    let pc = lib();
    let laptop = lib();
    let f = fill(&pc);
    pc.like(f.artist.id).unwrap();
    pc.like(f.release.id).unwrap();
    let own = laptop.upsert_artist(&Artist::new("КИНО")).unwrap();
    let credit = ArtistCredit::single("Кино".to_owned(), Some(own.id));
    let album = laptop.upsert_release(&Release::new("Группа Крови", credit)).unwrap();
    pull(&laptop, &pc);
    let kept = laptop.sync_conflicts(&id(&pc)).unwrap();
    assert_eq!(kept.identity, 2, "{kept:?}");
    assert!(kept.waiting >= 4, "the album's track, the links, the likes: {kept:?}");
    assert!(laptop.get_artist(f.artist.id).unwrap().is_none());
    assert!(laptop.get_release(f.release.id).unwrap().is_none());
    assert!(laptop.get_track(f.track.id).unwrap().is_none(), "its album is not here");
    assert_eq!(laptop.get_artist(own.id).unwrap().unwrap().name, "КИНО", "nothing here changed");
    assert!(laptop.get_release(album.id).unwrap().is_some());
    assert!(!laptop.is_liked(own.id).unwrap(), "no like moved onto another id");
    assert!(laptop.get_track(f.stream.id).unwrap().is_some(), "what does not point at them came");
    // Trying again does not touch the identity conflicts.
    laptop.sync_retry_at(now_ms() + 2 * RETRY_MAX_MS).unwrap();
    assert_eq!(laptop.sync_conflicts(&id(&pc)).unwrap().identity, 2);
    // A newer version of the artist that does not clash goes through, and
    // what waited for it follows.
    pc.write(|tx| {
        tx.execute("UPDATE artist SET name = 'Kino (band)' WHERE id = ?1", [f.artist.id.to_string()])?;
        Ok(())
    })
    .unwrap();
    pull(&laptop, &pc);
    assert_eq!(laptop.sync_conflicts(&id(&pc)).unwrap().identity, 1, "the album still waits");
    assert!(laptop.get_artist(f.artist.id).unwrap().is_some());
    laptop.sync_retry_at(now_ms() + 2 * RETRY_MAX_MS).unwrap();
    assert!(laptop.is_liked(f.artist.id).unwrap(), "the like that waited for it");
    assert!(!laptop.is_liked(f.release.id).unwrap(), "the album's still waits");
}

/// Rows from before sync have no version: two that differ end the same on
/// both devices (the larger device id), two that are the same stay; a row
/// with its own last edit time is ordered by it.
#[test]
fn rows_from_before_sync_end_the_same_everywhere() {
    let pc = lib();
    let pi = lib();
    let f = fill(&pc);
    pull(&pi, &pc);
    pc.set_lyrics_pick(f.track.id, Some("lrclib")).unwrap();
    pi.set_lyrics_pick(f.track.id, Some("genius")).unwrap();
    pc.set_track_fx(f.track.id, Some("slow")).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    pi.set_track_fx(f.track.id, Some("fast")).unwrap();
    for l in [&pc, &pi] {
        l.write(|tx| {
            tx.execute("DELETE FROM change_log WHERE tbl IN ('lyrics_pick', 'track_fx', 'analysis')", [])?;
            backfill(tx)
        })
        .unwrap();
    }
    let v: i64 = pc.read(|c| Ok(c.query_row("SELECT hlc FROM change_log WHERE tbl = 'lyrics_pick'", [], |r| r.get(0))?)).unwrap();
    assert_eq!(v, 0);
    pull(&pi, &pc);
    pull(&pc, &pi);
    pull(&pi, &pc);
    let winner = if id(&pc) > id(&pi) { "lrclib" } else { "genius" };
    assert_eq!(pc.lyrics_pick(f.track.id).unwrap().as_deref(), Some(winner));
    assert_eq!(pi.lyrics_pick(f.track.id).unwrap().as_deref(), Some(winner));
    for l in [&pc, &pi] {
        assert_eq!(l.track_fx().unwrap(), vec![(f.track.id, "fast".to_owned())], "the later edit");
    }
    // The same analysis on both: left alone (not taken, not noted again).
    let came: Option<String> = pi
        .read(|c| Ok(c.query_row("SELECT came_from FROM change_log WHERE tbl = 'analysis'", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(came, None);
    // A later edit of either goes over.
    pc.set_lyrics_pick(f.track.id, Some("musixmatch")).unwrap();
    pull(&pi, &pc);
    assert_eq!(pi.lyrics_pick(f.track.id).unwrap().as_deref(), Some("musixmatch"));
}

/// A `lyrics_align` row as it travels: version `v`, saved or not, gone or
/// live.
fn align_row(v: i64, node: &str, saved: bool, gone: bool, body: &str) -> SyncRow {
    let d = (!gone).then(|| {
        vec![
            Value::from("h"),
            Value::from(0.9),
            Value::from("fits"),
            Value::from(0),
            Value::from(1.0),
            Value::from(body),
            Value::from("test"),
            Value::from(1),
            Value::from(i64::from(saved)),
            Value::from(1),
        ]
    });
    SyncRow { k: r#"["t1","lrclib"]"#.to_owned(), v: (v, node.to_owned(), gone), d, f: Some(saved) }
}

/// The body left after the rows come in this order (none: no row).
fn align_after(rows: &[&SyncRow]) -> Option<String> {
    let l = lib();
    let t = sync_table("lyrics_align").unwrap();
    let cols: Vec<String> = t.cols.iter().map(|c| (*c).to_owned()).collect();
    for r in rows {
        let page = SyncPage { table: t.name.to_owned(), cols: cols.clone(), rows: vec![(*r).clone()], next: 0, more: false };
        l.sync_apply(&page, &r.v.1, false).unwrap();
    }
    l.read(|c| Ok(c.query_row("SELECT body FROM lyrics_align", [], |r| r.get(0)).optional()?)).unwrap()
}

fn orders(n: usize) -> Vec<Vec<usize>> {
    if n == 0 {
        return vec![Vec::new()];
    }
    let mut out = Vec::new();
    for rest in orders(n - 1) {
        for at in 0..=rest.len() {
            let mut o = rest.clone();
            o.insert(at, n - 1);
            out.push(o);
        }
    }
    out
}

/// Saved over automatic as one order with deletes in it: a delete keeps
/// the flag of what it deleted, so an automatic row or the delete of one
/// never beats a saved row, a later delete of the saved row does — in
/// every order the rows come.
#[test]
fn a_saved_alignment_ranks_over_automatic_rows_and_their_deletes_in_any_order() {
    let saved = align_row(1, "a", true, false, "mine");
    let auto = align_row(2, "b", false, false, "auto");
    let auto_gone = align_row(3, "b", false, true, "");
    let saved_gone = align_row(4, "a", true, true, "");
    let all = [&saved, &auto, &auto_gone, &saved_gone];
    for o in orders(4) {
        let rows: Vec<&SyncRow> = o.iter().map(|i| all[*i]).collect();
        assert_eq!(align_after(&rows), None, "the saved row's delete is last: {o:?}");
    }
    let three = [&saved, &auto, &auto_gone];
    for o in orders(3) {
        let rows: Vec<&SyncRow> = o.iter().map(|i| three[*i]).collect();
        assert_eq!(align_after(&rows).as_deref(), Some("mine"), "{o:?}");
    }
    for o in orders(2) {
        let pair = [&auto, &auto_gone];
        let rows: Vec<&SyncRow> = o.iter().map(|i| pair[*i]).collect();
        assert_eq!(align_after(&rows), None, "the later delete: {o:?}");
    }
    // A delete made here carries the flag of the row it deleted.
    let l = lib();
    l.write(|tx| {
        tx.execute(
            "INSERT INTO lyrics_align(track_id, source, text_hash, fit, timing, body, made_by, made_at, saved, fits)
             VALUES ('t1', 'lrclib', 'h', 0.9, 'fits', 'mine', 'test', 1, 1, 1)",
            [],
        )?;
        tx.execute("DELETE FROM lyrics_align", [])?;
        Ok(())
    })
    .unwrap();
    let page = l.sync_changes("lyrics_align", 0, 10, None).unwrap();
    assert_eq!(page.rows.len(), 1);
    assert!(page.rows[0].v.2 && page.rows[0].f == Some(true));
}

/// Two devices, real edits: a word timing the owner saved beats an
/// automatic one made later, on both; between two saved ones the later
/// wins.
#[test]
fn a_saved_alignment_beats_an_automatic_one() {
    let pc = lib();
    let pi = lib();
    let f = fill(&pc);
    pull(&pi, &pc);
    let align = |l: &Library, body: &str, saved: bool| {
        l.write(|tx| {
            tx.execute(
                "INSERT OR REPLACE INTO lyrics_align(track_id, source, text_hash, fit, timing, body, made_by, made_at, saved, fits)
                 VALUES (?1, 'lrclib', 'h', 0.9, 'fits', ?2, 'test', 1, ?3, 1)",
                params![f.track.id.to_string(), body, saved],
            )?;
            Ok(())
        })
        .unwrap();
    };
    let body = |l: &Library| -> String { l.read(|c| Ok(c.query_row("SELECT body FROM lyrics_align", [], |r| r.get(0))?)).unwrap() };
    align(&pc, "mine", true);
    std::thread::sleep(std::time::Duration::from_millis(5));
    align(&pi, "auto", false);
    pull(&pi, &pc);
    pull(&pc, &pi);
    assert_eq!(body(&pc), "mine");
    assert_eq!(body(&pi), "mine");
    std::thread::sleep(std::time::Duration::from_millis(5));
    align(&pi, "mine again", true);
    pull(&pc, &pi);
    pull(&pi, &pc);
    assert_eq!(body(&pc), "mine again");
    assert_eq!(body(&pi), "mine again");
}

/// A page from a clock far ahead is refused whole, and moves nothing here.
#[test]
fn a_page_from_a_clock_far_ahead_is_refused() {
    let pc = lib();
    let pi = lib();
    pc.upsert_artist(&Artist::new("Сплин")).unwrap();
    let mut page = pc.sync_changes("artist", 0, 10, None).unwrap();
    page.rows[0].v.0 = version_at(now_ms() + 3_600_000);
    assert!(page_ahead(&page, now_ms()));
    assert!(pi.sync_apply(&page, &id(&pc), true).is_err());
    assert!(pi.sync_cursors(&id(&pc)).unwrap().tables.is_empty());
    assert!(pi.tick() < now_ms() + 60_000);
    page.rows[0].v.0 = version_at(now_ms() + 60_000);
    assert!(!page_ahead(&page, now_ms()), "a minute is within reason");
    // Taken, the clock is past it before anything else can write.
    pi.sync_apply(&page, &id(&pc), true).unwrap();
    pi.upsert_artist(&Artist::new("Би-2")).unwrap();
    let newest: i64 = pi.read(|c| Ok(c.query_row("SELECT max(hlc) FROM change_log WHERE came_from IS NULL", [], |r| r.get(0))?)).unwrap();
    assert!(newest > page.rows[0].v.0);
}

/// The epoch is kept beside the database too: a database put back from a
/// copy (its epoch not the one beside it), or one that synced and lost the
/// copy, takes a new one, so the other devices take it from the start.
#[test]
fn a_database_put_back_takes_a_new_epoch() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.sqlite");
    let first = Library::open(&path).unwrap().sync_heads().unwrap().epoch;
    let file = epoch_file_of(&path);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), first);
    assert_eq!(Library::open(&path).unwrap().sync_heads().unwrap().epoch, first, "the same database");
    std::fs::write(&file, "a-later-one").unwrap();
    let second = Library::open(&path).unwrap().sync_heads().unwrap().epoch;
    assert_ne!(second, first);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), second);
    // Never synced: a missing copy is written again, the epoch stays.
    std::fs::remove_file(&file).unwrap();
    assert_eq!(Library::open(&path).unwrap().sync_heads().unwrap().epoch, second);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), second);
    // Synced: a missing copy means a new epoch.
    Library::open(&path).unwrap().sync_start_over("pi", "e", &[]).unwrap();
    std::fs::remove_file(&file).unwrap();
    let third = Library::open(&path).unwrap().sync_heads().unwrap().epoch;
    assert_ne!(third, second);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), third);
}

/// A copy of the epoch that cannot be written: syncing is off on this
/// database (with the reason), and the epoch does not change at each start.
#[test]
fn an_epoch_that_cannot_be_kept_turns_syncing_off() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.sqlite");
    // A folder where the copy goes: it can be neither read nor written.
    std::fs::create_dir_all(epoch_file_of(&path)).unwrap();
    let epoch = |l: &Library| l.read(|c| meta_get(c, "sync.epoch")).unwrap();
    let lib = Library::open(&path).unwrap();
    assert!(lib.sync_blocked().is_some());
    assert!(lib.sync_heads().is_err());
    let first = epoch(&lib);
    assert!(first.is_some());
    drop(lib);
    let lib = Library::open(&path).unwrap();
    assert!(lib.sync_blocked().is_some());
    assert_eq!(epoch(&lib), first, "not a new one at every start");
}

/// A library that already has things keeps them: the other's are added.
#[test]
fn a_first_sync_onto_a_full_library_merges() {
    let pc = lib();
    let laptop = lib();
    let f = fill(&pc);
    let own = laptop.upsert_artist(&Artist::new("Аквариум")).unwrap();
    let credit = ArtistCredit::single(own.name.clone(), Some(own.id));
    let song = laptop.upsert_track(&Track::new("Город золотой", credit)).unwrap();
    laptop.like(song.id).unwrap();
    pull(&laptop, &pc);
    assert!(laptop.is_liked(song.id).unwrap());
    assert!(laptop.is_liked(f.track.id).unwrap());
    assert!(laptop.get_artist(own.id).unwrap().is_some());
    assert!(laptop.get_artist(f.artist.id).unwrap().is_some());
    // And the other way.
    pull(&pc, &laptop);
    assert!(pc.is_liked(song.id).unwrap());
    assert_eq!(pc.get_track(song.id).unwrap().unwrap().title, "Город золотой");
}

/// Two libraries that synced know the same ids for the same things.
#[test]
fn ids_stay_the_same_across_devices() {
    let pc = lib();
    let pi = lib();
    fill(&pc);
    pull(&pi, &pc);
    let ids = |l: &Library, table: &str| -> Vec<String> {
        l.read(|c| {
            let mut s = c.prepare(&format!("SELECT id FROM {table} ORDER BY id"))?;
            let rows = s.query_map([], |r| r.get(0))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
        .unwrap()
    };
    for table in ["artist", "release", "track", "playlist", "playlist_entry", "event"] {
        assert_eq!(ids(&pc, table), ids(&pi, table), "{table}");
    }
}

/// A guest's plays stay where they were played; the voice assistant's
/// come into the history as they are, with who put them on and where (the
/// taste weighs them by that), the owner's own plays untouched.
#[test]
fn the_assistant_comes_with_its_actor_and_guests_stay_out() {
    let pc = lib();
    let pi = lib();
    let f = fill(&pc);
    pull(&pi, &pc);
    let before = pc.listening_events().unwrap();
    let as_ = |actor| EventMeta { actor: Some(actor), ..EventMeta::default() };
    for (track, actor) in [(f.track.id, ApiActor::Assistant), (f.stream.id, ApiActor::Assistant), (f.stream.id, ApiActor::Guest)] {
        pi.record_event_with(track, EventType::Start, None, Some(0), EventContext::Other, as_(actor)).unwrap();
        pi.record_event_with(track, EventType::Complete, None, Some(180_000), EventContext::Other, as_(actor)).unwrap();
    }
    pull(&pc, &pi);
    let after = pc.listening_events().unwrap();
    assert!(before.iter().all(|e| after.contains(e)), "the owner's history as it was");
    let heard: Vec<_> = after.iter().filter(|e| !before.contains(e)).collect();
    assert_eq!(heard.len(), 4, "the assistant's plays came");
    assert!(heard.iter().all(|e| e.actor == Some(ApiActor::Assistant)));
    assert!(heard.iter().all(|e| e.device.as_str() == id(&pi)), "the device they played on");
    assert!(!pc.own_device_ids().unwrap().contains(&id(&pi)), "the Pi is not the PC");
    let guests: i64 = pc.read(|c| Ok(c.query_row("SELECT count(*) FROM event WHERE actor = 'guest'", [], |r| r.get(0))?)).unwrap();
    assert_eq!(guests, 0, "a guest's plays never leave the device");
    // On the Pi itself the guest's plays are kept, out of the history.
    let on_pi = pi.listening_events().unwrap();
    assert_eq!(on_pi.len(), before.len() + 4);
    assert!(on_pi.iter().all(|e| e.actor != Some(ApiActor::Guest)));
}

/// The device id becomes the node id: the files are noted under it, the
/// old one is kept.
#[test]
fn the_device_takes_its_node_id() {
    let pc = lib();
    let f = fill(&pc);
    let old = id(&pc);
    assert!(pc.adopt_node_id("node-abc").unwrap());
    assert!(!pc.adopt_node_id("node-abc").unwrap());
    assert_eq!(id(&pc), "node-abc");
    let (files, before): (Vec<String>, Option<String>) = pc
        .read(|c| {
            let mut s = c.prepare("SELECT node_id FROM node_file")?;
            let rows = s.query_map([], |r| r.get(0))?;
            let files = rows.collect::<rusqlite::Result<_>>()?;
            Ok((files, meta_get(c, "device_ids_before")?))
        })
        .unwrap();
    assert_eq!(files, vec!["node-abc".to_owned()]);
    assert_eq!(before.as_deref(), Some(old.as_str()));
    let own = pc.own_device_ids().unwrap();
    assert!(own.contains("node-abc") && own.contains(old.as_str()), "the plays of both are its own");
    // New edits carry it.
    pc.unlike(f.track.id).unwrap();
    let by: String = pc.read(|c| Ok(c.query_row("SELECT updated_by FROM likes", [], |r| r.get(0))?)).unwrap();
    assert_eq!(by, "node-abc");
    // A new file is noted under it too.
    pc.add_source(&Source::new(f.stream.id, SourceKind::LocalFile, "C:/Music/k.opus", Codec::Opus).with_quality(false)).unwrap();
    let n: i64 = pc.read(|c| Ok(c.query_row("SELECT count(*) FROM node_file WHERE node_id = 'node-abc'", [], |r| r.get(0))?)).unwrap();
    assert_eq!(n, 2);
}

/// Turning a device off and on keeps where it was.
#[test]
fn syncing_can_be_turned_off_per_device() {
    let lib = lib();
    assert!(lib.sync_off_nodes().unwrap().is_empty());
    lib.set_sync_off("pi", true).unwrap();
    assert_eq!(lib.sync_off_nodes().unwrap(), vec!["pi".to_owned()]);
    assert!(lib.sync_cursors("pi").unwrap().off);
    lib.set_sync_off("pi", false).unwrap();
    assert!(!lib.sync_cursors("pi").unwrap().off);
    lib.sync_mark_ok("pi", 77).unwrap();
    assert_eq!(lib.sync_cursors("pi").unwrap().last_ok, Some(77));
}

/// A page of a version with a column this one lacks (or without one it
/// has) still merges what both know.
#[test]
fn a_page_of_another_version_merges_what_both_know() {
    let pc = lib();
    let pi = lib();
    let a = pc.upsert_artist(&Artist::new("Сплин")).unwrap();
    let mut page = pc.sync_changes("artist", 0, 10, None).unwrap();
    page.cols.push("future".into());
    for r in &mut page.rows {
        if let Some(d) = &mut r.d {
            d.push(Value::from(1));
        }
    }
    let images = page.cols.iter().position(|c| c == "images").unwrap();
    page.cols.remove(images);
    for r in &mut page.rows {
        if let Some(d) = &mut r.d {
            d.remove(images);
        }
    }
    let done = pi.sync_apply(&page, &id(&pc), false).unwrap();
    assert_eq!(done.taken, 1);
    assert_eq!(pi.get_artist(a.id).unwrap().unwrap().name, "Сплин");
}

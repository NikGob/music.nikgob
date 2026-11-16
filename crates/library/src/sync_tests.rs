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

/// The schema has every column the list syncs, and every user-data table
/// its versions.
#[test]
fn the_list_matches_the_schema() {
    let lib = lib();
    lib.read(|c| {
        for t in TABLES {
            let cols: Vec<String> = t.keys.iter().chain(t.cols.iter()).map(|c| (*c).to_owned()).collect();
            c.prepare(&format!("SELECT {} FROM {} LIMIT 0", cols.join(", "), t.name)).unwrap_or_else(|e| panic!("{}: {e}", t.name));
            if t.own {
                for v in ["updated_at", "updated_by", "deleted"] {
                    assert!(t.cols.contains(&v), "{} lacks {v}", t.name);
                }
            }
        }
        Ok(())
    })
    .unwrap();
    let names: std::collections::HashSet<&str> = TABLES.iter().map(|t| t.name).collect();
    assert_eq!(names.len(), TABLES.len(), "each table once");
}

#[test]
fn the_clock_never_goes_back_and_follows_others_within_reason() {
    let last = AtomicI64::new(0);
    assert_eq!(hlc_next(&last, 1_000), 1_000);
    assert_eq!(hlc_next(&last, 1_000), 1_001, "the same millisecond: one more");
    assert_eq!(hlc_next(&last, 900), 1_002, "the wall clock went back: still on");
    // Another device's row two seconds ahead: the next edit here is later.
    hlc_observe(&last, 3_000, 1_010);
    assert_eq!(hlc_next(&last, 1_020), 3_001);
    // A clock a day ahead is followed only so far.
    let day = 86_400_000;
    hlc_observe(&last, 5_000 + day, 5_000);
    let next = hlc_next(&last, 5_000);
    assert_eq!(next, 5_000 + MAX_AHEAD_MS + 1);
    // An old time changes nothing.
    hlc_observe(&last, 10, 6_000);
    assert!(hlc_next(&last, 6_000) > next);
}

#[test]
fn edits_are_stamped_by_the_clock() {
    let lib = lib();
    let a = lib.tick();
    let b = lib.tick();
    assert!(b > a);
    lib.observe_clock(now_ms() + 60_000);
    assert!(lib.tick() > now_ms() + 59_000, "past what another device wrote");
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
/// come, but never into the owner's history: a session of it on the Pi
/// leaves what the PC's mixes are made of as it was.
#[test]
fn the_assistant_and_guests_do_not_move_the_owners_history() {
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
    assert_eq!(pc.listening_events().unwrap(), before, "the owner's history as it was");
    let heard = pc.assistant_events().unwrap();
    assert_eq!(heard.len(), 4, "the assistant's plays came");
    assert!(heard.iter().all(|e| e.actor == Some(ApiActor::Assistant)));
    assert!(heard.iter().filter(|e| e.event_type == EventType::Complete).all(|e| e.attention == Some(1.0)), "background");
    assert!(heard.iter().all(|e| e.device.as_str() == id(&pi)), "the device they played on");
    let guests: i64 = pc.read(|c| Ok(c.query_row("SELECT count(*) FROM event WHERE actor = 'guest'", [], |r| r.get(0))?)).unwrap();
    assert_eq!(guests, 0, "a guest's plays never leave the device");
    // On the Pi itself the guest's plays are kept, out of the history too.
    assert_eq!(pi.listening_events().unwrap().len(), before.len());
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

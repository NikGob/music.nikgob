
    use nikgob_core::model::{Artist, ArtistCredit, DeviceId, Master, Release, Replica, Track};

    use super::*;
    use crate::presence::Presence;

    #[test]
    fn a_failed_file_waits_longer_each_time() {
        assert_eq!(retry_after(1), RETRY_MIN);
        assert_eq!(retry_after(2), RETRY_MIN * 2);
        assert_eq!(retry_after(30), RETRY_MAX);
    }

    /// The profile is the ring holder's word (another device's is not);
    /// the pause is this device's own or one a device asks.
    #[test]
    fn the_holder_says_what_is_kept_and_any_owner_may_pause() {
        let a = Assigned { node: "pi".into(), profile: MeshProfile::Full, ..Assigned::default() };
        let other = Presence { assign: vec![Assigned { profile: MeshProfile::SoundOnly, ..a.clone() }], ..Presence::default() };
        let holder = Presence { holder: true, assign: vec![a.clone()], ..Presence::default() };
        assert_eq!(assigned_by(&[other.clone(), holder.clone()], "pi"), Some(a));
        assert_eq!(assigned_by(&[other.clone()], "pi"), None, "only the holder's word counts");
        assert_eq!(assigned_by(&[holder.clone()], "tv"), None);
        let none = HashSet::new();
        assert!(!paused_by(&none, &[holder.clone()], "pi"));
        assert!(paused_by(&["pi".to_owned()].into(), &[], "pi"));
        let asks = Presence { pause: vec!["pi".into()], ..Presence::default() };
        assert!(paused_by(&none, &[asks], "pi"));
    }

    #[test]
    fn masters_are_taken_from_any_device_listing_them_once() {
        let item = |h: &str| MasterItem { hash: h.into(), codec: "flac".into(), size: 10, tracks: Vec::new() };
        let now = Instant::now();
        let lists: HashMap<String, (Instant, Vec<MasterItem>)> =
            [("pc".to_owned(), (now, vec![item("a"), item("b")])), ("lap".to_owned(), (now, vec![item("b"), item("c")]))].into();
        let held: HashSet<String> = ["c".to_owned()].into();
        let plan = plan_masters(&lists, &held);
        assert_eq!(plan.iter().map(|(m, _)| m.hash.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(plan[1].1, ["lap", "pc"], "both devices have it");
    }

    /// A device's copy counts the masters only once every device it pulls
    /// from listed them: a list that failed to come leaves the number as it
    /// was, not a plan with no masters in it.
    #[test]
    fn a_copy_waits_for_the_masters_of_every_device() {
        let item = |h: &str, size: u64| MasterItem { hash: h.into(), codec: "flac".into(), size, tracks: Vec::new() };
        let now = Instant::now();
        let peers = ["pc".to_owned(), "lap".to_owned()];
        let held: HashSet<String> = ["a".to_owned()].into();
        let lists: HashMap<String, (Instant, Vec<MasterItem>)> =
            [("pc".to_owned(), (now, vec![item("a", 10)]))].into();
        assert_eq!(masters_plan(&lists, &peers, &held), None, "lap's list is not in");
        let all: HashMap<String, (Instant, Vec<MasterItem>)> = [
            ("pc".to_owned(), (now, vec![item("a", 10)])),
            ("lap".to_owned(), (now, vec![item("b", 20), item("a", 10)])),
        ]
        .into();
        assert_eq!(masters_plan(&all, &peers, &held), Some((10, 30)), "a file shared by two once");
    }

    #[test]
    fn a_device_counts_once_for_a_copy() {
        let h = |s: &str| Held { hash: s.into(), verified_ms: 1 };
        let answers = vec![("pi".to_owned(), vec![h("a"), h("a"), h("x")]), ("lap".to_owned(), vec![h("a"), h("b")])];
        let c = confirmed(&answers, &["a".to_owned(), "b".to_owned()]);
        assert_eq!(c.get("a"), Some(&2));
        assert_eq!(c.get("b"), Some(&1));
        assert_eq!(c.get("x"), None, "not asked");
    }

    fn chunks_body(chunks: Vec<Vec<u8>>, stop_at: usize, ctl: Arc<Ctl>) -> http::Response<Body> {
        let stream = futures_util::stream::iter(chunks.into_iter().enumerate().map(move |(i, c)| {
            if i == stop_at {
                ctl.stop.store(true, Ordering::Relaxed);
            }
            Ok::<_, std::io::Error>(axum::body::Bytes::from(c))
        }));
        http::Response::new(Body::from_stream(stream))
    }

    /// A pause stops a file between two pieces, its part kept; the pull
    /// goes on from there and the whole file checks; a damaged one is
    /// dropped with its note.
    #[tokio::test]
    async fn a_paused_file_goes_on_where_it_stopped_and_is_checked() {
        let dir = tempfile::tempdir().unwrap();
        let bytes: Vec<u8> = (0..=255u8).cycle().take(30_000).collect();
        let hash = blake3::hash(&bytes).to_hex().to_string();
        let part = dir.path().join("t.part");
        let meta = dir.path().join("t.meta");
        std::fs::write(&meta, &hash).unwrap();
        let ctl = Arc::new(Ctl { keep_free: 0, ..Ctl::new(RATE) });
        let mut room = Reserve::new(ctl.clone(), 30_000);
        let chunks: Vec<Vec<u8>> = bytes.chunks(10_000).map(<[u8]>::to_vec).collect();
        let first = receive_paced(chunks_body(chunks.clone(), 1, ctl.clone()), &part, 0, 30_000, 0, &ctl, &mut room).await;
        assert_eq!(first, Err(Failed::Stopped));
        let kept = std::fs::metadata(&part).unwrap().len();
        assert_eq!(kept, 20_000, "the pieces before the pause stay");
        assert_eq!(ctl.reserved.load(Ordering::Relaxed), 10_000, "what is written is no longer kept for it");
        ctl.stop.store(false, Ordering::Relaxed);
        let rest = http::Response::new(Body::from(bytes[kept as usize..].to_vec()));
        assert_eq!(receive_paced(rest, &part, kept, 30_000, 0, &ctl, &mut room).await, Ok(30_000));
        assert_eq!(ctl.received.load(Ordering::Relaxed), 30_000);
        assert!(verify_part(&part, &meta, &hash).is_ok());
        drop(room);
        assert_eq!(ctl.reserved.load(Ordering::Relaxed), 0);
        let mut room = Reserve::new(ctl.clone(), 0);

        std::fs::write(&part, &bytes[..100]).unwrap();
        assert_eq!(verify_part(&part, &meta, &hash).unwrap_err(), "файл пришёл повреждённым");
        assert!(!part.exists() && !meta.exists());

        // A new profile or new pins: a pull of the old plan stops.
        ctl.era.store(1, Ordering::Relaxed);
        let old = http::Response::new(Body::from(bytes.clone()));
        assert_eq!(receive_paced(old, &dir.path().join("o.part"), 0, 30_000, 0, &ctl, &mut room).await, Err(Failed::Stopped));
        // Never more than the planned file's bytes.
        let big = http::Response::new(Body::from(bytes.clone()));
        assert!(matches!(receive_paced(big, &dir.path().join("b.part"), 0, 100, 1, &ctl, &mut room).await, Err(Failed::Bad(_))));
        // No write takes the disk past its reserve.
        let full = Arc::new(Ctl { keep_free: u64::MAX, ..Ctl::new(RATE) });
        let mut none = Reserve::new(full.clone(), 0);
        let f = dir.path().join("f.part");
        let r = receive_paced(http::Response::new(Body::from(bytes.clone())), &f, 0, 30_000, 0, &full, &mut none).await;
        assert_eq!(r, Err(Failed::NoRoom));
        assert_eq!(std::fs::metadata(&f).unwrap().len(), 0, "not a byte past the reserve");
    }

    /// A pull keeps for itself what it may still write; the others' share
    /// is what the rest keep; what it did not write goes back at its end.
    #[test]
    fn a_pull_keeps_its_bytes_while_it_runs() {
        let c = Arc::new(Ctl::new(RATE));
        let mut a = Reserve::new(c.clone(), 100);
        let b = Reserve::new(c.clone(), 50);
        assert_eq!((a.others(), b.others()), (50, 100));
        a.wrote(30);
        assert_eq!(c.reserved.load(Ordering::Relaxed), 120);
        a.wrote(500);
        assert_eq!(c.reserved.load(Ordering::Relaxed), 50, "a file larger than kept takes nothing of the others'");
        drop(b);
        drop(a);
        assert_eq!(c.reserved.load(Ordering::Relaxed), 0);
    }

    /// A part is found by listing its folder, only a plain file whose note
    /// is of the very file; a new one is never made over anything, an old
    /// one of another file goes, and it is written from where it stopped.
    #[test]
    fn parts_are_found_by_listing_and_never_made_over_anything() {
        let dir = tempfile::tempdir().unwrap();
        let (h1, h2) = ("1".repeat(64), "2".repeat(64));
        let p = Part::new(dir.path(), "t-a").unwrap();
        assert!(p.file.file_name().unwrap().to_string_lossy().len() > "t-a..part".len() + 30, "a random name");
        write_note(&p.note, &h1, "flac").unwrap();
        assert!(write_note(&p.note, &h1, "flac").is_err(), "a note is never written over");
        {
            let mut f = open_part(&p.file, 0).unwrap();
            std::io::Write::write_all(&mut f, b"12345").unwrap();
        }
        assert!(open_part(&Part::new(dir.path(), "t-a").unwrap().file, 3).is_err(), "only a new part starts at 0");
        let (found, ext, len) = find_part(dir.path(), "t-a", &h1).unwrap();
        assert_eq!((found.file.clone(), ext.as_str(), len), (p.file.clone(), "flac", 5));
        {
            let mut f = open_part(&p.file, 3).unwrap();
            std::io::Write::write_all(&mut f, b"xy").unwrap();
        }
        assert_eq!(std::fs::read(&p.file).unwrap(), b"123xy", "from where it stopped, what was after it dropped");
        assert!(find_part(dir.path(), "t-b", &h1).is_none(), "another job's");
        assert!(find_part(dir.path(), "t-a", &h2).is_none(), "of another file");
        assert!(!p.file.exists() && !p.note.exists(), "it went");
        assert!(read_note(&dir.path().join("t-a.0.meta")).is_none());
        let orphan = Part::new(dir.path(), "t-a").unwrap();
        write_note(&orphan.note, &h1, "flac").unwrap();
        assert!(find_part(dir.path(), "t-a", &h1).is_none());
        assert!(!orphan.note.exists(), "a note whose part never came goes");
    }

    /// A part's name taken by a link is never written or read through.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_part_never_goes_through_a_link() {
        let dir = tempfile::tempdir().unwrap();
        let song = dir.path().join("song.flac");
        std::fs::write(&song, b"music").unwrap();
        let p = Part::named(dir.path(), "t-a", &"0".repeat(32));
        std::os::unix::fs::symlink(&song, &p.file).unwrap();
        std::os::unix::fs::symlink(&song, &p.note).unwrap();
        let ctl = Arc::new(Ctl { keep_free: 0, ..Ctl::new(RATE) });
        let mut room = Reserve::new(ctl.clone(), 0);
        let r = receive_paced(http::Response::new(Body::from(vec![1u8; 10])), &p.file, 0, 100, 0, &ctl, &mut room).await;
        assert!(matches!(r, Err(Failed::Bad(_))));
        assert!(open_part(&p.file, 0).is_err() && blake3_plain(&p.file).is_err() && read_note(&p.note).is_none());
        assert!(write_note(&p.note, &"1".repeat(64), "flac").is_err());
        assert!(find_part(dir.path(), "t-a", &"1".repeat(64)).is_none());
        assert_eq!(std::fs::read(&song).unwrap(), b"music", "the file it points to is untouched");
    }

    /// A disk whose free space is not known gets nothing; the reserve and
    /// the running pulls' bytes are kept.
    #[test]
    fn a_disk_keeps_its_reserve() {
        assert!(!has_room(&[Path::new("")], 0, 0), "free space not known");
        let dir = tempfile::tempdir().unwrap();
        assert!(!has_room(&[dir.path()], u64::MAX / 2, 0), "never past the reserve");
        assert!(!has_room(&[dir.path()], 0, u64::MAX / 2), "the running pulls' bytes count");
    }

    /// The pauses asked here are kept and come back after a restart.
    #[test]
    fn pauses_asked_here_outlive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(PAUSED);
        let r = ReplicaShared::load(file.clone());
        r.set_paused("pi", true);
        r.set_paused("tv", true);
        r.set_paused("tv", false);
        assert_eq!(ReplicaShared::load(file).pause_list(), ["pi"]);
    }

    /// The pulls together keep to the rate.
    #[tokio::test(start_paused = true)]
    async fn pulls_are_paced() {
        let pace = Pace::new(1_000_000.0);
        let start = Instant::now();
        for _ in 0..10 {
            pace.take(500_000).await;
        }
        assert!(start.elapsed() >= Duration::from_secs(5) - BURST, "five megabytes at a megabyte a second");
    }

    fn song(lib: &Library) -> Track {
        let artist = lib.upsert_artist(&Artist::new("Кино")).unwrap();
        let credit = ArtistCredit::single(artist.name.clone(), Some(artist.id));
        let mut r = Release::new("Группа крови", credit.clone());
        r.year = Some(1988);
        let release = lib.upsert_release(&r).unwrap();
        let mut t = Track::new("Группа крови", credit);
        t.release_id = Some(release.id);
        t.track_no = 1;
        lib.upsert_track(&t).unwrap()
    }

    fn want(t: TrackId, hash: &str, size: u64) -> Want {
        Want {
            track_id: t,
            file_hash: hash.into(),
            size,
            offers: vec![Offer {
                track_id: t,
                node_id: "pc".into(),
                file_hash: hash.into(),
                size,
                codec: "opus".into(),
                quality_score: 70,
                bitrate: Some(160),
                sample_rate: Some(48_000),
                bit_depth: None,
            }],
        }
    }

    /// A pulled song lands where the library lays songs out and plays as
    /// this device's own; a name another file has gets a number (never
    /// over it); the very file there already is taken as it is.
    #[test]
    fn a_pulled_song_is_laid_out_registered_and_never_overwrites() {
        let lib = Library::open_in_memory().unwrap();
        let root = tempfile::tempdir().unwrap();
        let library = root.path().join("Music");
        std::fs::create_dir_all(&library).unwrap();
        let t = song(&lib);
        let bytes = b"OggS opus".to_vec();
        let hash = blake3::hash(&bytes).to_hex().to_string();
        let target = library.join("Кино").join("1988 - Группа крови").join("01 - Группа крови.opus");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, b"another file").unwrap();
        let part = root.path().join("t.part");
        std::fs::write(&part, &bytes).unwrap();

        let placed = place_song(&lib, &library, "me", &want(t.id, &hash, bytes.len() as u64), &part, &hash, "opus", bytes.len() as u64, Room::default()).unwrap();
        assert_eq!(placed, library.join("Кино").join("1988 - Группа крови").join("01 - Группа крови (2).opus"));
        assert_eq!(std::fs::read(&target).unwrap(), b"another file", "never over another file");
        assert_eq!(std::fs::read(&placed).unwrap(), bytes);
        assert!(!part.exists());
        let s = lib.active_source(t.id).unwrap().unwrap();
        assert_eq!(s.locator, placed.to_string_lossy());
        assert_eq!((s.file_hash.as_deref(), s.quality_score, s.codec), (Some(hash.as_str()), 70, Codec::Opus));
        let dirs = Dirs { library: library.clone(), masters: library.join(".musiclib").join("masters") };
        // Its hash noted when it was put there: held without reading it.
        let (held, pending) = held_on_disk(&lib, &dirs, &mut Rehash { files: 0, bytes: 0 }).unwrap();
        assert_eq!(held[&t.id].iter().map(|f| f.file_hash.as_deref()).collect::<Vec<_>>(), [Some(hash.as_str())]);
        assert!(pending.is_empty());
        assert!(lib.replica_verified(&hash, "me").unwrap().is_some());

        // A disk that would go below its reserve: the part stays.
        std::fs::write(&part, &bytes).unwrap();
        let tight = Room { keep: u64::MAX / 2, others: 0 };
        let elsewhere = library.join("Кино").join("1988 - Группа крови").join("02 - Другая.opus");
        assert_eq!(put_in_place(&part, &elsewhere, &hash, bytes.len() as u64, &library, tight), Err(Failed::NoRoom));
        assert!(part.exists() && !elsewhere.exists());

        // The same file at the name already: taken as it is.
        std::fs::write(&target, &bytes).unwrap();
        std::fs::write(&part, &bytes).unwrap();
        let again = put_in_place(&part, &target, &hash, bytes.len() as u64, &library, Room::default()).unwrap();
        assert_eq!(again, (target.clone(), false), "not moved: the file was there");
        assert!(!part.exists());
    }

    /// A master of one track goes by name into the masters folder, a disc
    /// image's flat under its hash; both kept as this device's.
    #[test]
    fn a_pulled_master_goes_into_the_masters_folder() {
        let lib = Library::open_in_memory().unwrap();
        let root = tempfile::tempdir().unwrap();
        let masters = root.path().join("Music").join(".musiclib").join("masters");
        let t = song(&lib);
        let bytes = b"fLaC master".to_vec();
        let hash = blake3::hash(&bytes).to_hex().to_string();
        let part = root.path().join("m.part");
        std::fs::write(&part, &bytes).unwrap();
        let item = MasterItem { hash: hash.clone(), codec: "flac".into(), size: bytes.len() as u64, tracks: vec![t.id] };
        let placed = place_master(&lib, &masters, "me", &item, &part, "flac", Room::default()).unwrap();
        assert!(placed.starts_with(&masters) && placed.parent() != Some(masters.as_path()), "by name: {}", placed.display());
        let by_name = placed.to_string_lossy().into_owned();
        assert_eq!(lib.get_master(t.id).unwrap().unwrap().file_path.as_deref(), Some(by_name.as_str()));

        let image = b"fLaC image".to_vec();
        let ihash = blake3::hash(&image).to_hex().to_string();
        std::fs::write(&part, &image).unwrap();
        let disc = MasterItem { hash: ihash.clone(), codec: "flac".into(), size: image.len() as u64, tracks: vec![TrackId::new(), TrackId::new()] };
        let flat = place_master(&lib, &masters, "me", &disc, &part, "flac", Room::default()).unwrap();
        assert_eq!(flat, masters.join(format!("{ihash}.flac")));
        let (here, pending) = masters_on_disk(&lib, &masters, &mut Rehash { files: 0, bytes: 0 }).unwrap();
        assert!(pending.is_empty(), "their hashes noted when they were put there");
        assert!(here.contains(&ihash), "its copy, though its tracks are not here yet");
        assert!(here.contains(&hash));
    }

    /// What this device holds is what is on its disk, the very file: a
    /// source or a master row whose file is gone, outside its folders or
    /// another file now is not held; a flat master file is; a file not
    /// read yet waits (its bytes charged) and is read in a later plan.
    #[test]
    fn only_the_very_files_on_the_disk_are_held() {
        let lib = Library::open_in_memory().unwrap();
        let root = tempfile::tempdir().unwrap();
        let dirs = Dirs { library: root.path().join("Music"), masters: root.path().join("Music").join(".musiclib").join("masters") };
        std::fs::create_dir_all(&dirs.masters).unwrap();
        let t = song(&lib);
        fn sum(b: &[u8]) -> String {
            blake3::hash(b).to_hex().to_string()
        }
        let here = dirs.library.join("a.opus");
        std::fs::write(&here, b"a").unwrap();
        let changed = dirs.library.join("b.opus");
        std::fs::write(&changed, b"now another file").unwrap();
        let outside = root.path().join("c.opus");
        std::fs::write(&outside, b"c").unwrap();
        let rows = [(here.clone(), sum(b"a")), (dirs.library.join("gone.opus"), sum(b"g")), (changed, sum(b"b")), (outside, sum(b"c"))];
        for (loc, h) in rows {
            let mut s = Source::new(t.id, SourceKind::LocalFile, loc.to_string_lossy().into_owned(), Codec::Opus);
            s.file_hash = Some(h);
            lib.add_source(&s).unwrap();
        }
        let (held, pending) = held_on_disk(&lib, &dirs, &mut Rehash { files: 0, bytes: 0 }).unwrap();
        assert!(held.is_empty(), "nothing read yet");
        assert_eq!(pending.get(&t.id), Some(&(1 + 16)), "the files inside, their bytes on the disk");
        let (held, pending) = held_on_disk(&lib, &dirs, &mut Rehash::new()).unwrap();
        assert!(pending.is_empty());
        assert_eq!(held[&t.id].iter().map(|f| (f.file_hash.clone(), f.size)).collect::<Vec<_>>(), [(Some(sum(b"a")), 1)]);

        let flat = sum(b"x");
        std::fs::write(dirs.masters.join(format!("{flat}.flac")), b"x").unwrap();
        let wrong = sum(b"y");
        std::fs::write(dirs.masters.join(format!("{wrong}.flac")), b"not y").unwrap();
        let gone = dirs.masters.join("Кино").join("01.flac");
        lib.put_master(&Master {
            track_id: t.id,
            codec: Codec::Flac,
            file_hash: "e".repeat(64),
            size_bytes: 1,
            origin: None,
            file_path: Some(gone.to_string_lossy().into_owned()),
            replicas: Vec::new(),
        })
        .unwrap();
        let (m, waiting) = masters_on_disk(&lib, &dirs.masters, &mut Rehash { files: 0, bytes: 0 }).unwrap();
        assert!(m.is_empty() && waiting.contains(&flat) && waiting.contains(&wrong));
        let (m, waiting) = masters_on_disk(&lib, &dirs.masters, &mut Rehash::new()).unwrap();
        assert!(waiting.is_empty());
        assert!(m.contains(&flat));
        assert!(!m.contains(&wrong), "a file that is not its master");
        assert!(!m.contains(&"e".repeat(64)), "its file is gone: pulled again");
    }

    /// A plan reads again at most so many files, the first whatever its
    /// size.
    #[test]
    fn a_plan_reads_a_bounded_number_of_files_again() {
        let mut r = Rehash::new();
        assert!(r.take(REHASH_BYTES * 2), "the first one whatever its size");
        assert!(!r.take(1), "then its bytes ran out");
        let mut r = Rehash::new();
        for _ in 0..REHASH_FILES {
            assert!(r.take(1));
        }
        assert!(!r.take(1));
    }

    fn offer(track: TrackId, node: &str, hash: &str, size: u64, quality: i32) -> Offer {
        Offer {
            track_id: track,
            node_id: node.into(),
            file_hash: hash.into(),
            size,
            codec: "opus".into(),
            quality_score: quality,
            bitrate: None,
            sample_rate: None,
            bit_depth: None,
        }
    }

    /// A device's plan is the best file of every track; of those, the ones
    /// it holds are its very hash, or a file of a better quality (as the
    /// plan itself has it).
    #[test]
    fn a_devices_plan_is_the_best_file_of_every_track_and_what_it_holds() {
        let (a, b) = (TrackId::new(), TrackId::new());
        let offers = vec![
            offer(a, "pc", "ha", 1_000, 70),
            offer(a, "tv", "ha-low", 500, 40),
            offer(b, "pc", "hb", 2_000, 50),
        ];
        let none: &[Offer] = &[];
        assert_eq!(planned_bytes(&Keep::All, offers.clone(), none), (0, 3_000), "the whole plan");
        let held = [offer(a, "pi", "ha", 1_000, 70), offer(b, "pi", "hb-old", 700, 30)];
        assert_eq!(planned_bytes(&Keep::All, offers.clone(), &held), (1_000, 3_000), "the very file of one track");
        let held = [offer(a, "pi", "hq", 900, 80), offer(b, "pi", "hb", 2_000, 50)];
        assert_eq!(planned_bytes(&Keep::All, offers, &held), (3_000, 3_000), "a better file counts as the track");
    }

    /// The masters count once each, and a device's only when its copy is
    /// noted.
    #[test]
    fn the_masters_count_once_and_only_the_ones_a_device_verified() {
        let masters = [("ha".to_owned(), 1_000u64), ("hb".to_owned(), 2_000)];
        let verified: HashSet<String> = ["hb".to_owned()].into();
        assert_eq!(masters_bytes(&masters, &HashSet::new()), (3_000, 0));
        assert_eq!(masters_bytes(&masters, &verified), (3_000, 2_000));
    }

    /// What a device of «Полная копия» is to hold comes from the plan: the
    /// best file of every track from the devices it pulls from and every
    /// master here, of which its verified copies count; nothing held: the
    /// goal alone. The disk's use never enters it.
    #[test]
    fn a_devices_copies_are_counted_from_the_plan() {
        let lib = Library::open_in_memory().unwrap();
        lib.set_device_id(DeviceId("pc".into())).unwrap();
        let t = song(&lib);
        let mut s = Source::new(t.id, SourceKind::LocalFile, "C:/Music/a.opus", Codec::Opus);
        s.file_hash = Some("ha".into());
        s.size_bytes = Some(1_000);
        lib.add_source(&s).unwrap();
        let master = Master {
            track_id: t.id,
            codec: Codec::Flac,
            file_hash: "hm".into(),
            size_bytes: 5_000,
            origin: None,
            file_path: None,
            replicas: Vec::new(),
        };
        lib.put_master(&master).unwrap();
        let peers = ["pc".to_owned()];
        assert_eq!(copy_stats(&lib, "pi", MeshProfile::Full, &peers).unwrap(), Some(MeshCopy { done: 0, goal: 6_000 }), "the plan, nothing held");
        lib.put_replica(&Replica { file_hash: "hm".into(), node_id: DeviceId("pi".into()), verified_at: None }).unwrap();
        assert_eq!(
            copy_stats(&lib, "pi", MeshProfile::Full, &peers).unwrap(),
            Some(MeshCopy { done: 0, goal: 6_000 }),
            "a copy never verified is not held"
        );
        lib.put_replica(&Replica { file_hash: "hm".into(), node_id: DeviceId("pi".into()), verified_at: Some(1) }).unwrap();
        assert_eq!(copy_stats(&lib, "pi", MeshProfile::Full, &peers).unwrap(), Some(MeshCopy { done: 5_000, goal: 6_000 }), "its master");
        assert_eq!(copy_stats(&lib, "pi", MeshProfile::Library, &peers).unwrap(), Some(MeshCopy { done: 0, goal: 1_000 }), "no masters");
        assert_eq!(copy_stats(&lib, "pi", MeshProfile::SoundOnly, &peers).unwrap(), None, "no plan: the disk's use");
        assert_eq!(copy_stats(&lib, "pi", MeshProfile::Favorites { cap_bytes: 8 << 30 }, &peers).unwrap(), None);
    }

    /// The device's own numbers: the plan's bytes (every track's best file
    /// and every master the peers list, once) and, of them, the files its
    /// checked disk holds — its very hash, or a better quality; a file not
    /// read yet counts for neither.
    #[test]
    fn a_devices_progress_is_its_plan_against_the_files_it_holds() {
        let (a, b) = (TrackId::new(), TrackId::new());
        let offers = vec![
            offer(a, "pc", "ha", 1_000, 70),
            offer(a, "tv", "ha", 1_000, 70),
            offer(b, "pc", "hb", 2_000, 50),
        ];
        let held: HashMap<TrackId, Vec<HeldFile>> =
            [(a, vec![HeldFile { track_id: a, file_hash: Some("ha".into()), size: 1_000, quality_score: 70, locator: "a.opus".into() }])].into();
        assert_eq!(songs_progress(&Keep::All, &offers, &held), Some((1_000, 3_000)), "one track's very file");
        assert_eq!(songs_progress(&Keep::All, &offers, &HashMap::new()), Some((0, 3_000)));
        let cap = Keep::Ordered { order: vec![a], cap: 8 << 30 };
        assert_eq!(songs_progress(&cap, &offers, &held), None, "«Избранное» has its cap to show");

        let now = Instant::now();
        let item = |h: &str, size: u64| MasterItem { hash: h.into(), codec: "flac".into(), size, tracks: Vec::new() };
        let lists: HashMap<String, (Instant, Vec<MasterItem>)> =
            [("pc".to_owned(), (now, vec![item("m1", 4_000), item("m2", 5_000)])), ("tv".to_owned(), (now, vec![item("m2", 5_000)]))].into();
        let verified: HashSet<String> = ["m1".to_owned()].into();
        assert_eq!(masters_progress(&lists, &verified), (4_000, 9_000), "each master once; only the one held");
        assert_eq!(masters_progress(&lists, &HashSet::new()), (0, 9_000));
    }

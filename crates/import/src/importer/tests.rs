#[cfg(test)]
mod cover_tests {
    use super::*;

    #[test]
    fn a_youtube_music_cover_is_asked_large_and_others_as_they_are() {
        assert_eq!(
            large_web_cover("https://yt3.googleusercontent.com/abc=w544-h544-l90-rj"),
            "https://yt3.googleusercontent.com/abc=w1200-h1200-l90-rj"
        );
        let deezer = "https://cdn-images.dzcdn.net/images/cover/x/1000x1000-000000-80-0-0.jpg";
        assert_eq!(large_web_cover(deezer), deezer);
    }

    #[test]
    fn a_pillarboxed_frame_keeps_only_its_art() {
        let frame = image::RgbImage::from_fn(640, 360, |x, _| if (140..500).contains(&x) { image::Rgb([30, 90, 200]) } else { image::Rgb([0, 0, 0]) });
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(frame).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let c = process_cover(&png, 1200).unwrap();
        assert!(cover_frame::is_frame(c.width, c.height));
        let cut = cut_bars(c);
        assert_eq!((cut.width, cut.height), (360, 360));
        // A frame without bars stays as it is.
        let still = process_cover(&crate::cover::testimg::solid_jpeg([90, 90, 90], 640, 360), 1200).unwrap();
        assert_eq!((cut_bars(still.clone()).width, still.height), (640, 360));
    }
}

#[cfg(test)]
mod roll_back_tests {
    use nikgob_core::model::ArtistCredit;

    use super::*;

    fn master(track: TrackId, hash: &str) -> Master {
        Master { track_id: track, codec: Codec::Flac, file_hash: hash.into(), size_bytes: 1, origin: None, file_path: None, replicas: Vec::new() }
    }

    /// What a cancelled upgrade of `track` that registered the master
    /// `new` over `before` left to take back.
    fn undo(dir: &Path, track: &Track, new: &str, before: Option<Master>) -> Undo {
        Undo {
            track: track.id,
            before: Some(track.clone()),
            source: SourceId::new(),
            dest: dir.join("Music").join("gone.opus"),
            master_file: None,
            master_hash: Some(new.into()),
            master_before: before,
            master_registered: Some(master(track.id, new)),
            sidecar_before: None,
            sidecar_written: None,
            replaced: Replaced::default(),
        }
    }

    /// A cancelled replacement that brought its own master gives the track
    /// its master record as it was: the one before, or none at all.
    #[test]
    fn a_cancelled_replacement_gives_back_the_master_before() {
        let dir = tempfile::tempdir().unwrap();
        let lib = Library::open_in_memory().unwrap();
        let importer = Importer::new(ImportConfig::with_root(dir.path()), lib.clone(), None);
        let track = lib.upsert_track(&Track::new("Песня", ArtistCredit::single("Кто-то", None))).unwrap();
        let old = master(track.id, "old");
        lib.put_master(&master(track.id, "new")).unwrap();
        roll_back(&importer.inner, vec![undo(dir.path(), &track, "new", Some(old.clone()))]);
        assert_eq!(lib.get_master(track.id).unwrap(), Some(old));

        lib.put_master(&master(track.id, "new")).unwrap();
        roll_back(&importer.inner, vec![undo(dir.path(), &track, "new", None)]);
        assert_eq!(lib.get_master(track.id).unwrap(), None);
    }

    /// A cancelled track of an image an earlier job imported in part puts
    /// the image's shared sidecar back as it was; a sidecar the cancelled
    /// import made goes.
    #[test]
    fn a_cancelled_track_of_a_shared_image_keeps_the_sidecar_of_the_others() {
        let dir = tempfile::tempdir().unwrap();
        let lib = Library::open_in_memory().unwrap();
        let importer = Importer::new(ImportConfig::with_root(dir.path()), lib.clone(), None);
        let masters = importer.inner.cfg.masters_dir();
        std::fs::create_dir_all(&masters).unwrap();
        let sidecar = masters.join("image.json");
        let earlier = br#"{"track_id":"x","track_ids":["x"]}"#.to_vec();
        let ours = br#"{"track_id":"y","track_ids":["x","y"]}"#.to_vec();
        std::fs::write(&sidecar, &ours).unwrap();
        let track = lib.upsert_track(&Track::new("Песня", ArtistCredit::single("Кто-то", None))).unwrap();
        lib.put_master(&master(track.id, "image")).unwrap();
        let mut u = undo(dir.path(), &track, "image", None);
        u.sidecar_before = Some(earlier.clone());
        u.sidecar_written = Some(ours.clone());
        roll_back(&importer.inner, vec![u]);
        assert_eq!(std::fs::read(&sidecar).unwrap(), earlier, "the earlier tracks' sidecar is back");
        assert_eq!(lib.get_master(track.id).unwrap(), None);

        std::fs::write(&sidecar, &ours).unwrap();
        lib.put_master(&master(track.id, "image")).unwrap();
        let mut u = undo(dir.path(), &track, "image", None);
        u.sidecar_written = Some(ours.clone());
        roll_back(&importer.inner, vec![u]);
        assert!(!sidecar.exists(), "made by the cancelled import: gone");

        // A sidecar a later import wrote since stays as that one wrote it.
        let later = br#"{"track_id":"z","track_ids":["x","y","z"]}"#.to_vec();
        std::fs::write(&sidecar, &later).unwrap();
        lib.put_master(&master(track.id, "image")).unwrap();
        let mut u = undo(dir.path(), &track, "image", None);
        u.sidecar_before = Some(earlier.clone());
        u.sidecar_written = Some(ours.clone());
        roll_back(&importer.inner, vec![u]);
        assert_eq!(std::fs::read(&sidecar).unwrap(), later);
    }

    /// A cancelled job whose track another job upgraded since (its own
    /// master registered over the cancelled one's) leaves that later master
    /// and its file as they are; only the cancelled job's own file goes,
    /// since no row keeps it any more.
    #[test]
    fn a_cancel_keeps_a_later_upgrade_of_the_same_track() {
        let dir = tempfile::tempdir().unwrap();
        let lib = Library::open_in_memory().unwrap();
        let importer = Importer::new(ImportConfig::with_root(dir.path()), lib.clone(), None);
        let album = importer.inner.cfg.masters_dir().join("Кто-то").join("Альбом");
        std::fs::create_dir_all(&album).unwrap();
        let (first_file, later_file) = (album.join("01 - Песня.flac"), album.join("01 - Песня (2).flac"));
        std::fs::write(&first_file, b"first").unwrap();
        std::fs::write(&later_file, b"later").unwrap();
        let track = lib.upsert_track(&Track::new("Песня", ArtistCredit::single("Кто-то", None))).unwrap();
        let at = |hash: &str, file: &Path| Master { file_path: Some(file.to_string_lossy().into_owned()), ..master(track.id, hash) };
        // The first job registered its master; the later job its own over it.
        lib.put_master(&at("later", &later_file)).unwrap();

        let mut u = undo(dir.path(), &track, "first", None);
        u.master_registered = Some(at("first", &first_file));
        u.master_file = Some(first_file.clone());
        roll_back(&importer.inner, vec![u]);
        assert_eq!(lib.get_master(track.id).unwrap(), Some(at("later", &later_file)), "the later master stays");
        assert_eq!(std::fs::read(&later_file).unwrap(), b"later");
        assert!(!first_file.exists(), "the cancelled job's own file, kept by no row: to the Trash");
    }

    /// A cancelled import's master that another import took as its own
    /// (the very same file) stays for it, with its row; once no row keeps
    /// it, a cancel takes it to the Trash.
    #[test]
    fn a_cancel_keeps_a_master_another_track_took() {
        let dir = tempfile::tempdir().unwrap();
        let lib = Library::open_in_memory().unwrap();
        let importer = Importer::new(ImportConfig::with_root(dir.path()), lib.clone(), None);
        let file = importer.inner.cfg.masters_dir().join("Кто-то").join("Альбом").join("01 - Песня.flac");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"the song").unwrap();
        let named = |track: TrackId| Master { file_path: Some(file.to_string_lossy().into_owned()), ..master(track, "h") };
        let first = lib.upsert_track(&Track::new("Песня", ArtistCredit::single("Кто-то", None))).unwrap();
        let second = lib.upsert_track(&Track::new("Песня (ещё раз)", ArtistCredit::single("Кто-то", None))).unwrap();
        lib.put_master(&named(first.id)).unwrap();
        lib.put_master(&named(second.id)).unwrap();

        let mut u = undo(dir.path(), &first, "h", None);
        u.master_file = Some(file.clone());
        u.master_registered = Some(named(first.id));
        roll_back(&importer.inner, vec![u]);
        assert_eq!(std::fs::read(&file).unwrap(), b"the song", "the other track's master stays");
        assert_eq!(lib.get_master(second.id).unwrap(), Some(named(second.id)));
        assert_eq!(lib.get_master(first.id).unwrap(), None);

        let mut u = undo(dir.path(), &second, "h", None);
        u.master_file = Some(file.clone());
        u.master_registered = Some(named(second.id));
        roll_back(&importer.inner, vec![u]);
        assert!(!file.exists(), "no row keeps it: to the Trash");
        assert!(std::fs::read_dir(&importer.inner.cfg.trash_dir).unwrap().count() >= 1);
        assert!(!file.parent().unwrap().exists(), "its empty album folder goes");
    }
}

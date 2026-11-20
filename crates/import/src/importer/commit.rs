use super::*;
use super::cover::{COVER_FILE, write_cover_files};

// ------------------------------------------------------------------ stages

pub(super) fn analyze(cfg: &ImportConfig, path: &Path, origin: SourceOrigin) -> Result<Analyzed> {
    analyze_track(cfg, path, origin, None)
}

/// [`analyze`] for a file that may be a track cut out of an image: it is
/// taken for what the sheet says and ranked as the image.
pub(super) fn analyze_track(cfg: &ImportConfig, path: &Path, origin: SourceOrigin, cue: Option<Arc<Segment>>) -> Result<Analyzed> {
    let (mut tags, mut info) = tags::read_tags(path)?;
    if let Some(seg) = &cue {
        cue_import::apply(seg, &mut tags, &mut info);
    }
    let lossless = info.codec.is_lossless();
    // A cut track is checked as its image: the whole image's verdict, one
    // for all its tracks.
    let image_check = cue.as_ref().filter(|s| s.image.spectrum.is_some()).map(|s| &s.image);
    let mut audio = analyze_file(path, cfg.check_fake_lossless && lossless && image_check.is_none(), info.duration)?;
    if let Some(img) = image_check {
        audio.spectrum = img.spectrum;
        audio.spectrum_db = img.spectrum_db.clone();
    }
    let verdict = audio.spectrum.map(|s| s.verdict);
    let fake_lossless = lossless && verdict == Some(Verdict::Transcoded);
    let upsampled = lossless && verdict == Some(Verdict::Upsampled);
    if info.duration.is_zero() {
        info.duration = audio.duration();
    }
    // Upsampled "Hi-Res" is CD quality in a big container.
    let (rate, depth) = if upsampled {
        (Some(44_100), Some(16))
    } else {
        (info.sample_rate, info.bit_depth)
    };
    let score = if origin == SourceOrigin::Stream {
        nikgob_core::model::score::SAVED_STREAM
    } else if fake_lossless {
        // As what its content really is: an MP3 of about that bitrate.
        nikgob_core::quality::fake_score(sound_origin(&audio), audio.spectrum.map_or(0.0, |s| s.cutoff_hz))
    } else {
        quality_score(&info.codec, info.bitrate_kbps, rate, depth, false)
    };
    // What the file is as it came: a file found holding another song is
    // known by it again.
    let came_hash = fsutil::hash_file(path).ok();
    Ok(Analyzed {
        path: path.to_path_buf(),
        origin,
        tags,
        info,
        audio,
        fake_lossless,
        score,
        targets: Arc::default(),
        force_track: None,
        came_hash,
        job_started: nikgob_library::now_ms(),
        cue,
    })
}

/// The likely origin of an analysed file's sound (unknown without a check).
fn sound_origin(audio: &AudioAnalysis) -> nikgob_core::quality::SoundOrigin {
    audio.spectrum.map_or(nikgob_core::quality::SoundOrigin::Unknown, |s| {
        nikgob_core::quality::SoundOrigin::estimate(s.verdict, s.cutoff_hz)
    })
}

/// What a file really is, for «Подтверди» and the words of a copy that was
/// no better («FLAC из MP3 ~320», «MP3 320»), kept as data so it is said in
/// the reader's locale.
pub(super) fn quality_line(a: &Analyzed) -> nikgob_core::i18n::status::QualityWords {
    let check = a.audio.spectrum.map(|s| nikgob_core::quality::CheckSummary {
        verdict: s.verdict,
        origin: nikgob_core::quality::SoundOrigin::estimate(s.verdict, s.cutoff_hz),
        made_from: None,
        cutoff_hz: (s.cutoff_hz >= 1.0).then_some(s.cutoff_hz as u32),
    });
    nikgob_core::i18n::status::QualityWords::Label {
        codec: a.info.codec.clone(),
        bitrate_kbps: a.info.bitrate_kbps,
        check,
    }
}

/// What [`find_existing`] needs about an analyzed file.
pub(super) fn incoming<'a>(a: &'a Analyzed, id: &'a Identity) -> Incoming<'a> {
    Incoming {
        identity: id,
        tag_track_id: a.force_track.or(a.tags.track_id),
        fingerprint: a.audio.fingerprint.as_ref().map(|f| f.raw.as_slice()),
        targets: &a.targets.tracks,
        duration_ms: (!a.info.duration.is_zero()).then(|| a.info.duration_ms()),
    }
}

pub(super) fn decide_for(store: &mut Store, a: &Analyzed, id: &Identity) -> Result<Decision> {
    let existing = find_existing(store, &incoming(a, id))?;
    Ok(decide(existing, a.score))
}

/// Files of an album without a stored print that an import hears first,
/// at most (once each: their prints are kept).
const BACKFILL_MAX: usize = 24;

/// Where a file came from, as [`crate::same_audio::file_keys`] names it:
/// the place its job says (a Soulseek peer's file, a service's id) and its
/// torrent.
fn came_from(a: &Analyzed) -> Vec<String> {
    let mut out: Vec<String> = a.targets.from.iter().cloned().collect();
    if let SourceOrigin::Torrent { info_hash, .. } = &a.origin
        && !info_hash.is_empty()
    {
        out.push(format!("torrent:{info_hash}"));
    }
    out
}

/// Hears the files of `release` that have no stored print yet (a few, at
/// the import's low priority) and keeps their prints.
fn backfill_prints(lib: &Library, release: ReleaseId) {
    let todo = match lib.prints_todo(Some(release), BACKFILL_MAX) {
        Ok(t) => t,
        Err(e) => {
            log::warn!("import: the album's files without a print: {e}");
            return;
        }
    };
    let stop = std::sync::atomic::AtomicBool::new(false);
    for t in todo {
        let path = PathBuf::from(&t.locator);
        let (print, duration_ms, failed) = match print_file(&path, &stop) {
            Ok((p, d)) => (Some(p), d, None),
            // Not there right now: heard another time.
            Err(crate::quality::CheckError::Unavailable(_)) => continue,
            Err(e) => (None, None, Some(format!("{e:?}"))),
        };
        let row = nikgob_library::AudioPrint {
            source_id: t.source_id,
            track_id: t.track_id,
            print,
            duration_ms,
            came_hash: None,
            came_from: Vec::new(),
            failed,
        };
        if let Err(e) = lib.put_audio_print(&row) {
            log::warn!("import: the print of {} is not stored: {e}", path.display());
        }
    }
}

/// Whether the file is the song it is taken for (see [`crate::same_audio`]).
/// `track` is the library track it would become (`None`: a new one) on
/// `release`. A file marked before for the track, or one that sounds like
/// a song of its album the library had, is refused: what it is and where
/// it came from are noted for the track, and the error says so. One that
/// sounds like a song that came with it is flagged (`Some`: that file, its
/// track and how alike; the pair goes to «Подтверди» once it is in).
/// `backfill`: the album's files without a stored print are heard first.
pub(super) fn audio_check(
    inner: &Inner,
    a: &Analyzed,
    id: &Identity,
    track: Option<&Track>,
    release: Option<ReleaseId>,
    backfill: bool,
) -> Result<Option<(SourceId, TrackId, f64)>> {
    let lib = &inner.library;
    // Audio saved from a stream is what the stream is.
    if a.origin == SourceOrigin::Stream {
        return Ok(None);
    }
    let Some(print) = a.audio.fingerprint.as_ref().map(|f| f.raw.as_slice()) else {
        return Ok(None);
    };
    let keys = file_keys(a.came_hash.as_deref(), &came_from(a));
    let title = track.map_or(id.title.as_str(), |t| t.title.as_str());
    let title_of = |t: TrackId| lib.get_track(t).ok().flatten().map(|t| t.title);
    if let Some(t) = track {
        let marked = lib.wrong_audio_of(t.id)?;
        if let Some(m) = marked_before(&keys, Some(print), &marked) {
            let sounds_like = m.sounds_like;
            log::warn!("import: {} was found holding another song for «{title}» before, refused", a.path.display());
            lib.put_wrong_audio(t.id, &keys, sounds_like, Some(print))?;
            return Err(ImportError::WrongAudio {
                track: Some(t.id),
                title: title.to_owned(),
                sounds_like: sounds_like.and_then(title_of),
            });
        }
    }
    if backfill && let Some(r) = release {
        backfill_prints(lib, r);
    }
    let credit = track.map_or_else(|| id.artist_line.clone(), |t| t.artist_credit.display());
    let artist_key = nikgob_library::sort_key(&credit);
    let near = lib.prints_near(release, track.map(|t| t.id), Some(&artist_key))?;
    let own = match track {
        Some(t) => lib.prints_of_track(t.id)?,
        None => Vec::new(),
    };
    let known: Vec<Known<'_>> = near
        .iter()
        .map(|n| Known {
            source: n.source_id,
            track: n.track_id,
            title: &n.title,
            print: &n.print,
            duration_ms: n.duration_ms,
            settled: n.added_at < a.job_started,
            same_release: release.is_some() && n.release_id == release,
        })
        .collect();
    let heard = Heard {
        title,
        track: track.map(|t| t.id),
        print,
        duration_ms: (!a.info.duration.is_zero()).then(|| a.info.duration_ms()),
        expected_ms: track.and_then(|t| t.duration_ms),
        own: &own,
    };
    match judge(&heard, &known) {
        Judgement::Fine => Ok(None),
        Judgement::Flag { source, track: other, similarity } => Ok(Some((source, other, similarity))),
        Judgement::Refuse { track: other, similarity, .. } => {
            let sounds_like = near.iter().find(|n| n.track_id == other).map(|n| n.title.clone());
            log::warn!(
                "import: {} sounds like «{}» ({similarity:.2}), not «{title}»: refused",
                a.path.display(),
                sounds_like.as_deref().unwrap_or("?")
            );
            if let Some(t) = track {
                lib.put_wrong_audio(t.id, &keys, Some(other), Some(print))?;
            }
            Err(ImportError::WrongAudio { track: track.map(|t| t.id), title: title.to_owned(), sounds_like })
        }
    }
}

pub(super) struct Prepared {
    pub(super) tmp: PathBuf,
    ext: String,
    /// Lossless input encoded to Opus (so the original is a master).
    converted: bool,
    original_hash: Option<String>,
    /// A fake lossless input written as MP3 (no master: it is not lossless).
    fake_mp3: bool,
}

/// Lossless -> Opus, everything else copied as is (never lossy to lossy). The
/// result is an untagged temporary file inside the target album folder.
pub(super) fn convert(inner: &Inner, a: &Analyzed, album_dir: &Path) -> Result<Prepared> {
    fsutil::create_dir_all(album_dir)?;
    // Audio saved from a stream is kept exactly as it came.
    let lossless = a.info.codec.is_lossless() && a.origin != SourceOrigin::Stream;
    let encode = lossless && !a.fake_lossless;
    // A plain fake becomes an MP3 that keeps all it has, when the user lets
    // that happen without asking; without ffmpeg (or a doubtful cut) it
    // stays as it is, labelled for what it is.
    let hi_res = a.info.bit_depth.is_some_and(|b| b > 16) || a.info.sample_rate.is_some_and(|r| r > 48_000);
    let clear = a
        .audio
        .spectrum
        .is_some_and(|r| nikgob_core::quality::clear_fake(r.verdict, r.cutoff_hz, r.steepness_db, hi_res));
    let mp3 = (lossless && a.fake_lossless && clear && inner.cfg.convert_fake_lossless)
        .then(Mp3Encoder::find)
        .flatten();
    let ext = if encode {
        "opus".to_owned()
    } else if mp3.is_some() {
        "mp3".to_owned()
    } else {
        a.path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_else(|| "bin".to_owned())
    };
    let tmp = album_dir.join(format!(".{}.nikgob-tmp.{ext}", TrackId::new()));
    // A cut track's original is the image, whatever the track became.
    let image_hash = a.cue.as_ref().map(|s| s.image.hash.clone());
    let original_hash = if encode {
        inner.encoder.encode_to_opus(
            &a.path,
            &tmp,
            &EncodeOptions {
                bitrate_kbps: inner.cfg.opus_bitrate_kbps,
            },
        )?;
        match (&a.cue, &a.came_hash) {
            // A cut track's original is the image.
            (Some(seg), _) => Some(seg.image.hash.clone()),
            (None, Some(h)) => Some(h.clone()),
            (None, None) => Some(fsutil::hash_file(&a.path)?),
        }
    } else if let Some(enc) = &mp3 {
        let target = nikgob_core::quality::Mp3Target::for_origin(sound_origin(&a.audio));
        enc.encode(&a.path, &tmp, target)?;
        image_hash
    } else {
        fsutil::copy_file(&a.path, &tmp)?;
        image_hash
    };
    Ok(Prepared {
        tmp,
        ext,
        converted: encode,
        original_hash,
        fake_mp3: mp3.is_some(),
    })
}

pub(super) struct Done {
    pub(super) track_id: TrackId,
    pub(super) upgraded: bool,
    pub(super) master: Option<Master>,
    master_hash: Option<String>,
    pub(super) undo: Option<Undo>,
}

pub(super) enum Committed {
    Imported(Done),
    Duplicate { kept: TrackId },
}

fn build_tagset(
    a: &Analyzed,
    id: &Identity,
    track_id: TrackId,
    cover: Option<&ProcessedCover>,
) -> TagSet {
    TagSet {
        title: Some(id.title.clone()),
        artists: id.artists.clone(),
        artist: Some(id.artist_line.clone()),
        album: id.album.clone(),
        album_artist: Some(Store::release_artist_line(id)),
        track_no: (id.track_no > 0).then_some(id.track_no),
        track_total: id.track_total,
        disc_no: Some(id.disc_no),
        disc_total: id.disc_total,
        year: id.year,
        date: id.date.clone(),
        genre: id.genre.clone(),
        compilation: id.compilation,
        recording_mbid: id.recording_mbid.clone(),
        release_mbid: id.release_mbid.clone(),
        release_group_mbid: id.release_group_mbid.clone(),
        artist_mbid: id.artist_mbid.clone(),
        isrc: id.isrc.clone(),
        acoustid_id: id.acoustid_id.clone(),
        acoustid_fingerprint: a.audio.fingerprint.as_ref().map(|f| f.encoded.clone()),
        track_id: Some(track_id),
        lyrics: a.tags.lyrics.clone(),
        loudness_lufs: a.audio.loudness_lufs,
        cover: cover.map(|c| c.jpeg.clone()),
        ..TagSet::default()
    }
}

/// The critical section: decide again, tag, trash the replaced file, rename into
/// place, write the library rows, register the master.
pub(super) fn commit(
    inner: &Inner,
    a: &Analyzed,
    id: &Identity,
    prep: &Prepared,
    cover: Option<&ProcessedCover>,
) -> Result<Committed> {
    let result = commit_locked(inner, a, id, prep, cover);
    if result.is_err() || matches!(result, Ok(Committed::Duplicate { .. })) {
        let _ = fsutil::remove_file(&prep.tmp);
        if let Some(dir) = prep.tmp.parent() {
            fsutil::prune_empty_dirs(dir, &inner.cfg.library_dir);
        }
    }
    result
}

fn commit_locked(
    inner: &Inner,
    a: &Analyzed,
    id: &Identity,
    prep: &Prepared,
    cover: Option<&ProcessedCover>,
) -> Result<Committed> {
    let cfg = &inner.cfg;
    let lib = &inner.library;
    let mut store = lock(&inner.store);
    let decision = decide_for(&mut store, a, id)?;
    let existing = match &decision {
        Decision::Duplicate(e) => return Ok(Committed::Duplicate { kept: e.track.id }),
        Decision::New => None,
        Decision::Upgrade(e) | Decision::AddSource(e) => Some(e),
    };
    let before = existing.map(|e| e.track.clone());
    // A track of the wished album keeps its place there (named and tagged
    // so), whatever number the file brought.
    let kept_place;
    let id = match existing.and_then(|e| crate::album::keep_place(lib, &a.targets, &e.track, id)) {
        Some(placed) => {
            kept_place = placed;
            &kept_place
        }
        None => id,
    };
    // Another song's audio: asked again now that the album's other files
    // of this job may be in (a file that came with this one is flagged).
    let release = match existing {
        Some(e) => e.track.release_id,
        None => store.find_release(id)?,
    };
    let flagged = audio_check(inner, a, id, existing.map(|e| &e.track), release, false)?;
    let track_id = match existing {
        Some(e) => e.track.id,
        // Keep the id a rebuilt/backed-up file already carries, if it is free.
        None => a
            .tags
            .track_id
            .filter(|t| lib.get_track(*t).ok().flatten().is_none())
            .unwrap_or_default(),
    };

    tags::write_tags(&prep.tmp, &build_tagset(a, id, track_id, cover))?;

    // The replaced file goes to Trash first: it may sit exactly where the new one lands.
    let mut replaced = Replaced::default();
    if let Decision::Upgrade(e) = &decision
        && let Some(old) = &e.local
    {
        let old_path = PathBuf::from(&old.locator);
        let trash_path = if long_path(&old_path).exists() {
            let dest = trash_file(&cfg.trash_dir, &old_path)?;
            log::info!("import: {} is replaced, the old file is in the Trash at {}", old_path.display(), dest.display());
            replaced.file = Some((old_path.clone(), dest.clone()));
            let lrc = old_path.with_extension("lrc");
            if long_path(&lrc).exists()
                && let Ok(lrc_dest) = trash_file(&cfg.trash_dir, &lrc)
            {
                // Confirmed files (an album's better copy) can be taken back
                // later: where the lyrics went is kept for that.
                if !a.targets.pinned.is_empty() {
                    crate::replaced::keep_replaced_lyrics(lib, old.id, &lrc_dest);
                }
                replaced.lrc = Some((lrc, lrc_dest));
            }
            Some(dest.to_string_lossy().into_owned())
        } else {
            None
        };
        if let Err(e) = lib.trash_source(old.id, trash_path.as_deref()) {
            replaced.undo(lib);
            return Err(e.into());
        }
        replaced.source = Some(old.id);
    }

    let layout = layout_input(id);
    let album_dir = layout.album_dir(&cfg.library_dir);
    let dest = layout.unique_target_path(&cfg.library_dir, &prep.ext);
    if let Err(e) = fsutil::move_file(&prep.tmp, &dest) {
        // Put the replaced file back; the library still lists it as trashed, so
        // restore that too.
        replaced.undo(lib);
        return Err(e);
    }

    let finish = (|| -> Result<Done> {
        // The master the track had, for a cancel (this import may register
        // its own over it).
        let master_before = match existing {
            Some(e) => lib.get_master(e.track.id)?,
            None => None,
        };
        let out_info = tags::probe_info(&dest)?;
        let file_hash = fsutil::hash_file(&dest)?;
        let size = fsutil::file_size(&dest)?;

        let mut new_cover = false;
        let cover_ref = match cover {
            Some(c) => {
                new_cover = write_cover_files(&album_dir, c)?;
                Some(rel_to_library(cfg, &album_dir.join(COVER_FILE)))
            }
            None => None,
        };

        let release = store.ensure_release(id, cover_ref)?;
        let credit = store.track_credit(id)?;
        let mut track = Track::new(id.title.clone(), credit);
        track.id = track_id;
        track.recording_mbid = id.recording_mbid.clone();
        track.isrc = id.isrc.clone();
        track.acoustid = id.acoustid_id.clone();
        track.release_id = Some(release.id);
        track.disc_no = id.disc_no;
        track.track_no = id.track_no;
        track.duration_ms = Some(out_info.duration_ms());
        let track = lib.upsert_track(&track)?;

        let mut source = Source::new(
            track.id,
            SourceKind::LocalFile,
            dest.to_string_lossy().into_owned(),
            out_info.codec.clone(),
        );
        source.bitrate = out_info.bitrate_kbps;
        source.sample_rate = out_info.sample_rate;
        source.bit_depth = out_info.bit_depth;
        // A converted file keeps the rank of the lossless original it came from,
        // so re-importing that original is a duplicate and not an endless upgrade.
        source.quality_score = a.score;
        source.file_hash = Some(file_hash);
        source.size_bytes = Some(size);
        source.origin = a.origin.clone();
        let stored = lib.add_source(&source)?;
        // What the check found, kept for the labels and the quality panel;
        // a converted file's check is of the original it was made from.
        if let Some(report) = &a.audio.spectrum {
            let made_from = (prep.converted || prep.fake_mp3).then(|| a.info.codec.clone());
            let check = check_of(report, a.audio.spectrum_db.clone(), a.audio.spec.sample_rate, made_from, file_stamp(&dest));
            if let Err(e) = lib.put_quality_check(stored.id, &check) {
                log::warn!("the check of {} is not stored: {e}", dest.display());
            }
        }
        // What it sounds like, what it was as it came and where from.
        if let Some(fp) = &a.audio.fingerprint {
            let print = nikgob_library::AudioPrint {
                source_id: stored.id,
                track_id: track.id,
                print: Some(fp.raw.clone()),
                duration_ms: Some(out_info.duration_ms()),
                came_hash: a.came_hash.clone(),
                came_from: came_from(a),
                failed: None,
            };
            if let Err(e) = lib.put_audio_print(&print) {
                log::warn!("the print of {} is not stored: {e}", dest.display());
            }
        }
        if let Some((other_source, other_track, similarity)) = flagged {
            log::warn!("import: {} sounds like the file of {other_track} ({similarity:.2}), asked about", dest.display());
            if let Err(e) = lib.put_same_audio((stored.id, track.id), (other_source, other_track), similarity) {
                log::warn!("the pair of {} is not stored: {e}", dest.display());
            }
        }

        // The lossless original becomes the track's master. Every track cut
        // out of an image has the image as its master, even one kept as it
        // is (a fake lossless image: the image is still the only whole copy
        // of the disc, with its sheet). A master that cannot be registered
        // fails the import: the track's rows written now go again and the
        // original stays where it is, for the next try to register it.
        let mut master_file = None;
        let mut sidecar_before = None;
        let mut sidecar_written = None;
        let master = match (&prep.original_hash, prep.converted || a.cue.is_some()) {
            (Some(hash), true) => {
                sidecar_before = std::fs::read(long_path(&cfg.masters_dir().join(format!("{hash}.json")))).ok();
                let album_cover = album_dir.join(COVER_FILE);
                let album_cover = long_path(&album_cover).is_file().then_some(album_cover);
                match register_master(inner, a, hash, track.id, id, album_cover.as_deref()) {
                    Ok(r) => {
                        master_file = r.written;
                        sidecar_written = r.sidecar;
                        Some(r.master)
                    }
                    Err(e) => {
                        log::warn!("master of {} not registered, the import fails: {e}", a.master_path().display());
                        if let Err(e) = lib.forget_source(stored.id) {
                            log::warn!("the row of {} stays: {e}", dest.display());
                        }
                        let back = match &before {
                            Some(t) => lib.put_track_back(t).map(|_| ()),
                            None => lib.forget_new_track(track.id).map(|_| ()),
                        };
                        if let Err(e) = back {
                            log::warn!("track {} is not as it was: {e}", track.id);
                        }
                        // Albums and artists made now may be gone: read again.
                        *store = Store::new(lib.clone());
                        return Err(e);
                    }
                }
            }
            _ => None,
        };

        // From here on the track is in the library: nothing below may fail the import.
        // Palette and ambient texture, once per release (or when the cover grew).
        if cover.is_some() {
            let palette = (|| -> Result<()> {
                if new_cover || lib.get_palette(release.id)?.is_none() {
                    let bytes = std::fs::read(long_path(&album_dir.join(COVER_FILE)))
                        .map_err(fsutil::io_err(&album_dir))?;
                    let visuals = derive_visuals(&bytes)?;
                    let mut palette = Palette::new(
                        release.id,
                        visuals.seed_argb.unwrap_or(cfg.fallback_seed_argb),
                    );
                    palette.ambient_blur = Some(visuals.ambient_png);
                    lib.put_palette(&palette)?;
                }
                Ok(())
            })();
            if let Err(e) = palette {
                log::warn!("palette of {} not stored: {e}", dest.display());
            }
        }

        // A lossy file taken from the Inbox now lives in the library; the
        // fake lossless file an MP3 was made from goes to the Trash.
        if !prep.converted && a.path.starts_with(&cfg.inbox_dir) {
            let gone = if prep.fake_mp3 {
                trash_file(&cfg.trash_dir, &a.path).map(|to| {
                    log::info!("import: {} came as an MP3, the fake FLAC is in the Trash at {}", a.path.display(), to.display());
                })
            } else {
                fsutil::remove_file(&a.path)
            };
            if let Err(e) = gone {
                log::warn!("cannot remove imported {}: {e}", a.path.display());
            }
        }
        Ok(Done {
            track_id: track.id,
            upgraded: matches!(decision, Decision::Upgrade(_)),
            master_hash: master.as_ref().map(|m| m.file_hash.clone()),
            master: master.clone(),
            undo: Some(Undo {
                track: track.id,
                before: before.clone(),
                source: stored.id,
                dest: dest.clone(),
                master_file,
                master_hash: None,
                master_before,
                master_registered: master.clone(),
                sidecar_before,
                sidecar_written,
                replaced: Replaced::default(),
            }),
        })
    })();
    match finish {
        Ok(mut done) => {
            if let Some(u) = done.undo.as_mut() {
                u.master_hash = done.master_hash.take();
                u.replaced = replaced;
            }
            Ok(Committed::Imported(done))
        }
        Err(e) => {
            let _ = fsutil::remove_file(&dest);
            // The old file may have to come back to the very path just freed.
            replaced.undo(lib);
            Err(e)
        }
    }
}

/// What an upgrade moved out of the way, so a failed import can put it back.
#[derive(Default)]
pub(super) struct Replaced {
    /// `(original place, place in Trash)` of the old file.
    file: Option<(PathBuf, PathBuf)>,
    lrc: Option<(PathBuf, PathBuf)>,
    /// The old source, once the library lists it as trashed.
    source: Option<nikgob_core::model::SourceId>,
}

impl Replaced {
    pub(super) fn undo(&self, lib: &Library) {
        for (orig, in_trash) in self.file.iter().chain(&self.lrc) {
            if let Err(e) = fsutil::move_file(in_trash, orig) {
                log::warn!("cannot restore {} from Trash: {e}", orig.display());
            }
        }
        if let Some(id) = self.source
            && let Err(e) = lib.restore_source(id)
        {
            log::warn!("cannot restore the replaced source: {e}");
        }
    }
}

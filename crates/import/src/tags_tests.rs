use super::*;
use crate::cover::testimg::solid_png;
use crate::encode::testwav::*;
use crate::encode::{EncodeOptions, Encoder, RopusEncoder};

fn full_set(id: TrackId) -> TagSet {
    TagSet {
        title: Some("Группа крови".into()),
        artists: vec!["Кино".into()],
        artist: Some("Кино".into()),
        album: Some("Группа крови".into()),
        album_artist: Some("Кино".into()),
        track_no: Some(3),
        track_total: Some(9),
        disc_no: Some(1),
        disc_total: Some(2),
        year: Some(1988),
        date: Some("1988-04-24".into()),
        genre: Some("Rock".into()),
        compilation: false,
        recording_mbid: Some("11111111-2222-3333-4444-555555555555".into()),
        release_mbid: Some("66666666-7777-8888-9999-000000000000".into()),
        release_group_mbid: Some("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into()),
        artist_mbid: Some("01234567-89ab-cdef-0123-456789abcdef".into()),
        isrc: Some("RUA010000001".into()),
        acoustid_id: Some("acoustid-track-id".into()),
        acoustid_fingerprint: Some("AQADtEmUaEmU".into()),
        track_id: Some(id),
        lyrics: Some("Тёплое место".into()),
        loudness_lufs: Some(-14.5),
        cover: Some(solid_png([10, 20, 30], 64, 64)),
        ..TagSet::default()
    }
}

fn check_full(back: &TagSet, want: &TagSet) {
    assert_eq!(back.title, want.title);
    assert_eq!(back.artist, want.artist);
    assert_eq!(back.album, want.album);
    assert_eq!(back.album_artist, want.album_artist);
    assert_eq!(back.track_no, want.track_no);
    assert_eq!(back.track_total, want.track_total);
    assert_eq!(back.disc_no, want.disc_no);
    assert_eq!(back.disc_total, want.disc_total);
    assert_eq!(back.year, want.year);
    assert_eq!(back.genre, want.genre);
    assert_eq!(back.recording_mbid, want.recording_mbid);
    assert_eq!(back.release_mbid, want.release_mbid);
    assert_eq!(back.release_group_mbid, want.release_group_mbid);
    assert_eq!(back.artist_mbid, want.artist_mbid);
    assert_eq!(back.isrc, want.isrc);
    assert_eq!(back.acoustid_id, want.acoustid_id);
    assert_eq!(back.acoustid_fingerprint, want.acoustid_fingerprint);
    assert_eq!(back.track_id, want.track_id);
    assert_eq!(back.lyrics, want.lyrics);
    assert!(back.cover.is_some());
}

#[test]
fn gain_helpers() {
    // -14.5 LUFS needs -8.5 dB to reach -23 LUFS: -8.5 * 256 = -2176.
    assert_eq!(r128_gain_q8(-14.5), Some(-2176));
    assert_eq!(r128_gain_q8(-23.0), Some(0));
    assert_eq!(r128_gain_q8(f64::NEG_INFINITY), None);
    assert!((replaygain_db(-14.5).unwrap() + 3.5).abs() < 1e-6);
    assert_eq!(parse_gain_db("-3.50 dB"), Some(-3.5));
    assert_eq!(parse_pair("3/12"), (Some(3), Some(12)));
    assert_eq!(parse_pair("0"), (None, None));
    assert_eq!(parse_year("1988-04-24"), Some(1988));
    assert_eq!(parse_year("88"), None);
}

#[test]
fn opus_tag_roundtrip_keeps_track_id_cover_and_audio() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("in.wav");
    let opus = dir.path().join("song.opus");
    write_wav(&wav, &sine(440.0, 48_000, 2.0, 0.4, 2), 48_000, 2);
    RopusEncoder
        .encode_to_opus(&wav, &opus, &EncodeOptions { bitrate_kbps: 160 })
        .unwrap();

    let id = TrackId::new();
    let want = full_set(id);
    write_tags(&opus, &want).unwrap();
    let (back, info) = read_tags(&opus).unwrap();
    check_full(&back, &want);
    assert_eq!(back.r128_track_gain, Some(-2176));
    assert_eq!(info.codec, Codec::Opus);
    assert!((info.duration.as_secs_f64() - 2.0).abs() < 0.03);

    // The audio still decodes after the rewrite.
    let mut frames = 0usize;
    crate::decode::decode_file(&opus, &mut |c, s| {
        frames += c.len() / s.channels;
        std::ops::ControlFlow::Continue(())
    })
    .unwrap();
    assert!(frames.abs_diff(96_000) <= 2, "{frames}");

    // Rewriting replaces fields, keeps the rest, and never loses the id.
    write_tags(
        &opus,
        &TagSet {
            title: Some("Новое имя".into()),
            ..TagSet::default()
        },
    )
    .unwrap();
    let (again, _) = read_tags(&opus).unwrap();
    assert_eq!(again.title.as_deref(), Some("Новое имя"));
    assert_eq!(again.album, want.album);
    assert_eq!(again.track_id, Some(id));
}

#[test]
fn flac_uses_vorbis_comments_and_replaygain() {
    let dir = tempfile::tempdir().unwrap();
    let flac = dir.path().join("a.flac");
    write_flac(&flac, &sine(440.0, 44_100, 1.0, 0.4, 2), 44_100, 2);
    let (_, info) = read_tags(&flac).unwrap();
    assert_eq!(info.codec, Codec::Flac);
    assert_eq!(info.bit_depth, Some(16));
    assert_eq!(info.sample_rate, Some(44_100));

    let id = TrackId::new();
    let want = full_set(id);
    write_tags(&flac, &want).unwrap();
    let (back, _) = read_tags(&flac).unwrap();
    check_full(&back, &want);
    let rg = back.replaygain_track_gain_db.expect("replaygain");
    assert!((rg + 3.5).abs() < 0.01, "{rg}");
    assert_eq!(back.r128_track_gain, None);
}

#[test]
fn untagged_files_read_as_empty() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("plain.wav");
    write_wav(&wav, &sine(440.0, 44_100, 0.5, 0.4, 1), 44_100, 1);
    let (set, info) = read_tags(&wav).unwrap();
    assert_eq!(set.title, None);
    assert_eq!(set.track_id, None);
    assert_eq!(info.codec, Codec::Wav);
    assert_eq!(info.bit_depth, Some(16));
    assert!(read_tags(&dir.path().join("missing.opus")).is_err());
}

#[test]
fn wav_files_take_generic_tags() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("t.wav");
    write_wav(&wav, &sine(440.0, 44_100, 0.5, 0.4, 2), 44_100, 2);
    write_tags(
        &wav,
        &TagSet {
            title: Some("T".into()),
            artist: Some("A".into()),
            track_no: Some(2),
            ..TagSet::default()
        },
    )
    .unwrap();
    let (back, _) = read_tags(&wav).unwrap();
    assert_eq!(back.title.as_deref(), Some("T"));
    assert_eq!(back.artist.as_deref(), Some("A"));
    assert_eq!(back.track_no, Some(2));
}

#[test]
#[ignore = "needs ffmpeg on PATH"]
fn mp3_and_m4a_keep_the_track_id() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("in.wav");
    write_wav(&wav, &sine(440.0, 44_100, 2.0, 0.4, 2), 44_100, 2);
    for (name, args) in [
        ("a.mp3", vec!["-c:a", "libmp3lame", "-b:a", "192k"]),
        ("a.m4a", vec!["-c:a", "aac", "-b:a", "192k"]),
    ] {
        let out = dir.path().join(name);
        let status = std::process::Command::new("ffmpeg")
            .args(["-nostdin", "-v", "error", "-y", "-i"])
            .arg(&wav)
            .args(&args)
            .arg(&out)
            .status()
            .expect("ffmpeg");
        assert!(status.success());
        let id = TrackId::new();
        let want = full_set(id);
        write_tags(&out, &want).unwrap();
        let (back, info) = read_tags(&out).unwrap();
        assert_eq!(back.track_id, Some(id), "{name}");
        assert_eq!(back.title, want.title, "{name}");
        assert_eq!(back.recording_mbid, want.recording_mbid, "{name}");
        assert!(back.cover.is_some(), "{name}");
        assert!(info.bitrate_kbps.is_some());
        // A second write must not lose the id.
        write_tags(&out, &TagSet::default()).unwrap();
        assert_eq!(read_tags(&out).unwrap().0.track_id, Some(id), "{name}");
    }
}

#[test]
fn windows_1251_text_labelled_as_latin1_is_repaired() {
    use super::fix_cp1251_mojibake as fix;
    // "Кино", "Группа крови", "Ёлка 2" as Latin-1 renders them.
    assert_eq!(fix("\u{CA}\u{E8}\u{ED}\u{EE}"), "Кино");
    assert_eq!(
        fix("\u{C3}\u{F0}\u{F3}\u{EF}\u{EF}\u{E0} \u{EA}\u{F0}\u{EE}\u{E2}\u{E8}"),
        "Группа крови"
    );
    assert_eq!(fix("\u{A8}\u{EB}\u{EA}\u{E0} 2"), "Ёлка 2");
    // Real Western text, plain ASCII, real Cyrillic and mixed strings are left alone.
    for same in ["Café del Mar", "Björk", "Beyoncé", "Кино", "Track 01", "ÀÉ x", "Ñ"] {
        assert_eq!(fix(same), same);
    }
}

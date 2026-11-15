use std::time::Duration;

use nikgob_metadata::acoustid::{AcoustIdArtist, AcoustIdRecording, AcoustIdResult};
use nikgob_metadata::deezer::{DeezerAlbum, DeezerArtist, DeezerTrack};
use nikgob_metadata::matcher::{
    Candidate, CandidateIds, Confidence, LocalEvidence, Source, TrackMetadata,
    candidate_from_deezer, candidates_from_recording, candidates_from_release, match_candidates,
};
use nikgob_metadata::musicbrainz::{Recording, Release};

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!("fixtures/", $name))
    };
}

fn secs(s: u64) -> Duration {
    Duration::from_secs(s)
}

/// A MusicBrainz-style candidate.
fn mb(rec: &str, title: &str, artist: &str, album: &str, dur: u64) -> Candidate {
    Candidate {
        source: Source::MusicBrainz,
        ids: CandidateIds {
            recording_mbid: Some(rec.to_owned()),
            ..CandidateIds::default()
        },
        metadata: TrackMetadata {
            title: title.to_owned(),
            artist: artist.to_owned(),
            artists: vec![artist.to_owned()],
            album: Some(album.to_owned()),
            duration: Some(secs(dur)),
            release_status: Some("Official".into()),
            release_type: Some("Album".into()),
            ..TrackMetadata::default()
        },
    }
}

fn evidence(title: &str, artist: &str, album: &str, dur: u64) -> LocalEvidence {
    LocalEvidence {
        title: Some(title.into()),
        artist: Some(artist.into()),
        album: Some(album.into()),
        duration: Some(secs(dur)),
        ..LocalEvidence::default()
    }
}

#[test]
fn exact_match_is_sure() {
    let ev = evidence("Heroes", "David Bowie", "Heroes", 370);
    let result = match_candidates(
        &ev,
        vec![
            mb("rec-1", "Heroes", "David Bowie", "Heroes", 370),
            mb("rec-2", "Sound and Vision", "David Bowie", "Low", 183),
        ],
    );
    assert_eq!(result.confidence, Confidence::Sure);
    let best = result.best().unwrap();
    assert_eq!(best.ids.recording_mbid.as_deref(), Some("rec-1"));
    assert!(best.score > 0.99, "score {}", best.score);
    assert!(result.ranked[1].score < 0.5);
}

#[test]
fn remaster_suffix_and_feat_do_not_hurt() {
    let candidates = vec![mb("rec-1", "Heroes", "David Bowie", "Heroes", 370)];
    for title in [
        "Heroes (Remastered 2017)",
        "Heroes - 2017 Remaster",
        "Heroes [Deluxe Edition]",
        "heroes",
        "HEROES",
        "Heroes (feat. Someone Else)",
    ] {
        let ev = evidence(title, "David Bowie", "Heroes", 370);
        let result = match_candidates(&ev, candidates.clone());
        assert_eq!(result.confidence, Confidence::Sure, "{title}");
        assert!(result.best().unwrap().score > 0.99, "{title}");
    }
    // And the other way round: the remaster noise is on the candidate.
    let noisy = vec![mb(
        "rec-1",
        "Heroes (2017 Remaster)",
        "David Bowie feat. Brian Eno",
        "Heroes (Deluxe)",
        371,
    )];
    let ev = evidence("Heroes", "David Bowie", "Heroes", 370);
    assert_eq!(match_candidates(&ev, noisy).confidence, Confidence::Sure);
}

#[test]
fn two_close_candidates_are_unsure_with_top_three() {
    let ev = evidence("Yesterday", "The Beatles", "Help!", 125);
    let result = match_candidates(
        &ev,
        vec![
            mb("rec-a", "Yesterday", "The Beatles", "Help!", 125),
            mb("rec-b", "Yesterday", "The Beatles", "Help!", 126),
            mb("rec-c", "Yesterday", "The Beatles", "Anthology 2", 127),
            mb("rec-d", "Tomorrow", "Someone Else", "Other", 300),
        ],
    );
    assert_eq!(result.confidence, Confidence::Unsure);
    assert_eq!(result.options.len(), 3);
    let ids: Vec<_> = result
        .options
        .iter()
        .map(|o| o.ids.recording_mbid.clone().unwrap())
        .collect();
    assert!(
        !ids.contains(&"rec-d".to_owned()),
        "weak candidate is not offered: {ids:?}"
    );
    assert!(result.options[0].score >= result.options[1].score);
    assert!(result.options[1].score >= result.options[2].score);
    // Every option is a different recording.
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), 3);
}

#[test]
fn the_same_recording_on_many_releases_is_not_competition() {
    let ev = evidence("Heroes", "David Bowie", "Heroes", 370);
    let mut remaster = mb(
        "rec-1",
        "Heroes",
        "David Bowie",
        "Heroes (2017 Remaster)",
        370,
    );
    remaster.ids.release_mbid = Some("rel-2".into());
    let mut second = mb("rec-1", "Heroes", "David Bowie", "Heroes", 370);
    second.ids.release_mbid = Some("rel-1".into());
    let result = match_candidates(&ev, vec![remaster, second]);
    assert_eq!(result.confidence, Confidence::Sure);
    assert_eq!(result.ranked.len(), 2);
    assert_eq!(result.options.len(), 1, "one distinct recording");
}

#[test]
fn cyrillic_and_latin_spellings_match() {
    // Tags in Cyrillic, MusicBrainz candidate transliterated (and vice versa).
    let ev = evidence("Группа крови", "Кино", "Группа крови", 284);
    let result = match_candidates(
        &ev,
        vec![mb("rec-1", "Gruppa krovi", "Kino", "Gruppa krovi", 284)],
    );
    assert_eq!(result.confidence, Confidence::Sure);
    assert!(result.best().unwrap().score > 0.99);

    let ev = evidence("Kukushka", "Kino", "Zvezda po imeni Solntse", 366);
    let result = match_candidates(
        &ev,
        vec![
            mb("rec-1", "Кукушка", "Кино", "Звезда по имени Солнце", 366),
            mb("rec-2", "Мама, мы все сошли с ума", "Кино", "Ночь", 300),
        ],
    );
    assert_eq!(result.confidence, Confidence::Sure);
    assert_eq!(
        result.best().unwrap().ids.recording_mbid.as_deref(),
        Some("rec-1")
    );
}

#[test]
fn a_different_artist_is_not_a_match() {
    let ev = evidence("Yesterday", "Kino", "Help!", 125);
    let result = match_candidates(
        &ev,
        vec![mb("rec-1", "Yesterday", "The Beatles", "Help!", 125)],
    );
    assert_eq!(result.confidence, Confidence::Unsure);
    assert!(result.best().unwrap().score < 0.85);
}

#[test]
fn duration_mismatch_lowers_the_score() {
    let candidates = |dur| vec![mb("rec-1", "Heroes", "David Bowie", "Heroes", dur)];
    let ev = evidence("Heroes", "David Bowie", "Heroes", 370);

    let score = |dur| match_candidates(&ev, candidates(dur)).best().unwrap().score;
    let exact = score(370);
    let strong = score(372); // within 3 s
    let weak = score(378); // within 10 s
    let far = score(410);
    assert!((exact - strong).abs() < 1e-9, "+-3 s counts as a match");
    assert!(weak < exact, "{weak} < {exact}");
    assert!(far < weak, "{far} < {weak}");
    assert_eq!(
        match_candidates(&ev, candidates(410)).confidence,
        Confidence::Unsure,
        "a 40 s difference cannot be Sure"
    );
    assert_eq!(
        match_candidates(&ev, candidates(372)).confidence,
        Confidence::Sure
    );
}

#[test]
fn missing_duration_is_neutral() {
    let mut ev = evidence("Heroes", "David Bowie", "Heroes", 370);
    ev.duration = None;
    let result = match_candidates(
        &ev,
        vec![mb("rec-1", "Heroes", "David Bowie", "Heroes", 370)],
    );
    assert_eq!(result.confidence, Confidence::Sure);
}

#[test]
fn title_only_evidence_is_never_sure() {
    let ev = LocalEvidence {
        title: Some("Heroes".into()),
        duration: Some(secs(370)),
        ..LocalEvidence::default()
    };
    let result = match_candidates(
        &ev,
        vec![mb("rec-1", "Heroes", "David Bowie", "Heroes", 370)],
    );
    assert_eq!(result.confidence, Confidence::Unsure);
    assert!(result.best().unwrap().score <= 0.8 + 1e-9);
}

#[test]
fn filename_fills_gaps_in_the_tags() {
    let ev = LocalEvidence {
        filename: Some("C:\\Inbox\\01 - Кино - Группа крови.flac".into()),
        duration: Some(secs(284)),
        ..LocalEvidence::default()
    };
    assert_eq!(ev.resolved_title().as_deref(), Some("Группа крови"));
    assert_eq!(ev.resolved_artist().as_deref(), Some("Кино"));
    assert_eq!(ev.resolved_track_no(), Some(1));
    let result = match_candidates(
        &ev,
        vec![mb("rec-1", "Группа крови", "Кино", "Группа крови", 284)],
    );
    assert_eq!(result.confidence, Confidence::Sure);
}

#[test]
fn studio_album_is_preferred_over_live_and_compilation_releases() {
    let ev = LocalEvidence {
        title: Some("Группа крови".into()),
        artist: Some("Кино".into()),
        duration: Some(secs(284)),
        ..LocalEvidence::default()
    };
    let rec: Recording = serde_json::from_str(fixture!("mb_recording_lookup.json")).unwrap();
    let mut cands = candidates_from_recording(&rec);
    assert_eq!(cands.len(), 2);
    // Same recording on a live album and a bootleg compilation, listed first.
    let mut live = cands[0].clone();
    live.ids.release_mbid = Some("live-release".into());
    live.metadata.album = Some("Концерт".into());
    live.metadata.release_secondary_types = vec!["Live".into()];
    let mut boot = cands[0].clone();
    boot.ids.release_mbid = Some("boot-release".into());
    boot.metadata.release_status = Some("Bootleg".into());
    cands.insert(0, live);
    cands.insert(0, boot);

    let result = match_candidates(&ev, cands);
    assert_eq!(result.confidence, Confidence::Sure);
    let best = result.best().unwrap();
    assert_eq!(
        best.ids.release_mbid.as_deref(),
        Some("2f2a1b6c-6d8e-4f53-9a7f-5d5a0f3b9c01"),
        "the official studio album wins"
    );
    assert_eq!(best.metadata.release_type.as_deref(), Some("Album"));
    // Order: studio album, then compilation, ... and the live/bootleg ones behind.
    let rank = |id: &str| {
        result
            .ranked
            .iter()
            .position(|c| c.ids.release_mbid.as_deref() == Some(id))
            .unwrap()
    };
    assert!(
        rank("2f2a1b6c-6d8e-4f53-9a7f-5d5a0f3b9c01") < rank("3a3b2c7d-7e9f-4064-8b80-6e6b1a4c0d02")
    );
    assert!(rank("3a3b2c7d-7e9f-4064-8b80-6e6b1a4c0d02") < rank("live-release"));
    assert!(rank("2f2a1b6c-6d8e-4f53-9a7f-5d5a0f3b9c01") < rank("boot-release"));
}

#[test]
fn live_evidence_prefers_the_live_recording() {
    let ev = evidence("Heroes - Live", "David Bowie", "Stage", 400);
    let mut live = mb("rec-live", "Heroes", "David Bowie", "Stage", 400);
    live.metadata.release_secondary_types = vec!["Live".into()];
    let studio = mb("rec-studio", "Heroes", "David Bowie", "Heroes", 370);
    let result = match_candidates(&ev, vec![studio, live]);
    assert_eq!(
        result.best().unwrap().ids.recording_mbid.as_deref(),
        Some("rec-live")
    );
    assert_eq!(result.confidence, Confidence::Sure);

    // Without a live hint the studio recording wins.
    let ev = evidence("Heroes", "David Bowie", "Heroes", 370);
    let mut live = mb("rec-live", "Heroes", "David Bowie", "Stage", 400);
    live.metadata.release_secondary_types = vec!["Live".into()];
    let studio = mb("rec-studio", "Heroes", "David Bowie", "Heroes", 370);
    let result = match_candidates(&ev, vec![live, studio]);
    assert_eq!(
        result.best().unwrap().ids.recording_mbid.as_deref(),
        Some("rec-studio")
    );
}

#[test]
fn remix_is_a_different_song() {
    let ev = evidence("Heroes", "David Bowie", "Heroes", 370);
    let mut remix = mb(
        "rec-remix",
        "Heroes (DJ X Remix)",
        "David Bowie",
        "Heroes",
        370,
    );
    remix.metadata.release_secondary_types = vec!["Remix".into()];
    let original = mb("rec-1", "Heroes", "David Bowie", "Heroes", 370);
    let result = match_candidates(&ev, vec![remix, original]);
    assert_eq!(
        result.best().unwrap().ids.recording_mbid.as_deref(),
        Some("rec-1")
    );
    assert!(result.ranked[1].score < result.ranked[0].score - 0.05);
}

fn acoustid_hit(score: f64, rec: &str) -> AcoustIdResult {
    AcoustIdResult {
        id: "track".into(),
        score,
        recordings: vec![AcoustIdRecording {
            id: rec.into(),
            title: None,
            duration: None,
            artists: vec![AcoustIdArtist {
                id: String::new(),
                name: "x".into(),
            }],
            release_groups: vec![],
        }],
    }
}

#[test]
fn a_strong_fingerprint_beats_garbage_tags() {
    let ev = LocalEvidence {
        title: Some("Track 01".into()),
        artist: Some("Unknown Artist".into()),
        duration: Some(secs(370)),
        acoustid: vec![acoustid_hit(0.98, "rec-1")],
        ..LocalEvidence::default()
    };
    let result = match_candidates(
        &ev,
        vec![
            mb("rec-1", "Heroes", "David Bowie", "Heroes", 370),
            mb("rec-2", "Heroes", "Other Band", "Heroes", 370),
        ],
    );
    assert_eq!(result.confidence, Confidence::Sure);
    let best = result.best().unwrap();
    assert_eq!(best.ids.recording_mbid.as_deref(), Some("rec-1"));
    assert!(best.breakdown.acoustid.unwrap() > 0.9);
}

#[test]
fn a_weak_fingerprint_is_ignored_and_a_contradicting_one_hurts() {
    let ev_tags = evidence("Heroes", "David Bowie", "Heroes", 370);
    let cands = vec![mb("rec-1", "Heroes", "David Bowie", "Heroes", 370)];
    let plain = match_candidates(&ev_tags, cands.clone())
        .best()
        .unwrap()
        .score;

    // Below 0.5: noise, no effect.
    let mut ev = ev_tags.clone();
    ev.acoustid = vec![acoustid_hit(0.3, "rec-other")];
    let weak = match_candidates(&ev, cands.clone()).best().unwrap().score;
    assert!((plain - weak).abs() < 1e-9);

    // The fingerprint points at another recording: the tag match becomes doubtful.
    let mut ev = ev_tags;
    ev.acoustid = vec![acoustid_hit(0.97, "rec-other")];
    let contradicted = match_candidates(&ev, cands).best().unwrap().score;
    assert!(contradicted < plain);
    assert!(contradicted < 0.85);
}

fn deezer_track(id: u64, title: &str, artist: &str, album: &str, dur: u32) -> DeezerTrack {
    DeezerTrack {
        id,
        title: title.into(),
        duration: Some(dur),
        artist: Some(DeezerArtist {
            id: 1,
            name: artist.into(),
        }),
        album: Some(DeezerAlbum {
            id: 10,
            title: album.into(),
            cover_xl: Some("https://img/1000x1000.jpg".into()),
            record_type: Some("album".into()),
            ..DeezerAlbum::default()
        }),
        ..DeezerTrack::default()
    }
}

#[test]
fn deezer_fallback_can_be_sure() {
    let ev = evidence("Группа крови", "Кино", "Группа крови", 284);
    let result = match_candidates(
        &ev,
        vec![
            candidate_from_deezer(&deezer_track(
                1,
                "Группа крови",
                "Кино",
                "Группа крови",
                285,
            )),
            candidate_from_deezer(&deezer_track(
                2,
                "Кукушка",
                "Кино",
                "Звезда по имени Солнце",
                366,
            )),
        ],
    );
    assert_eq!(result.confidence, Confidence::Sure);
    let best = result.best().unwrap();
    assert_eq!(best.source, Source::Deezer);
    assert_eq!(best.ids.deezer_track_id, Some(1));
    assert_eq!(best.ids.deezer_album_id, Some(10));
    assert_eq!(
        best.metadata.cover_url.as_deref(),
        Some("https://img/1000x1000.jpg")
    );
}

#[test]
fn musicbrainz_and_deezer_hits_for_one_song_do_not_clash() {
    let ev = evidence("Группа крови", "Кино", "Группа крови", 284);
    let mb_cand = mb("rec-1", "Группа крови", "Кино", "Группа крови", 284);
    let dz_cand = candidate_from_deezer(&deezer_track(
        1,
        "Группа крови",
        "Кино",
        "Группа крови",
        285,
    ));
    let result = match_candidates(&ev, vec![dz_cand, mb_cand]);
    assert_eq!(result.confidence, Confidence::Sure);
    assert_eq!(
        result.best().unwrap().source,
        Source::MusicBrainz,
        "MusicBrainz wins ties"
    );
    assert_eq!(result.options.len(), 1);
}

#[test]
fn deezer_compilations_are_flagged_and_dates_cleaned() {
    let mut track = deezer_track(5, "Song", "Artist", "Hits", 200);
    track.album.as_mut().unwrap().record_type = Some("compile".into());
    track.release_date = Some("0000-00-00".into());
    track.isrc = Some("XX1234567890".into());
    let cand = candidate_from_deezer(&track);
    assert_eq!(cand.metadata.release_secondary_types, ["Compilation"]);
    assert_eq!(cand.metadata.release_type.as_deref(), Some("Album"));
    assert_eq!(cand.metadata.date, None);
    assert_eq!(cand.metadata.isrcs, ["XX1234567890"]);
}

fn release_of(c: Candidate, kind: &str, date: &str, track_no: u32) -> Candidate {
    let mut c = c;
    c.ids.release_mbid = Some(format!("{kind}-{date}"));
    c.metadata.release_type = Some(kind.into());
    c.metadata.date = Some(date.into());
    c.metadata.track_no = Some(track_no);
    c
}

/// «Рисунки» came out first as a single, then as the first song of the
/// album «Рисунки». A file of the album (its tags say 11 tracks) matched
/// the single, which is earlier, and took the single's cover.
#[test]
fn a_file_of_an_album_takes_the_album_over_its_earlier_single() {
    let song = || mb("rec-1", "Рисунки", "Artist", "Рисунки", 200);
    let single = || release_of(song(), "Single", "2023-03-01", 1);
    let album = || release_of(song(), "Album", "2023-09-01", 1);
    let ev = LocalEvidence {
        track_no: Some(1),
        track_total: Some(11),
        ..evidence("Рисунки", "Artist", "Рисунки", 200)
    };
    let best = match_candidates(&ev, vec![single(), album()]);
    assert_eq!(best.best().unwrap().metadata.release_type.as_deref(), Some("Album"));
    assert_eq!(best.confidence, Confidence::Sure);
    // A later song of the album, its tags without the count.
    let fifth = |c: Candidate| Candidate { metadata: TrackMetadata { title: "Пятая".into(), ..c.metadata }, ..c };
    let ev5 = LocalEvidence {
        track_no: Some(5),
        album: None,
        ..evidence("Пятая", "Artist", "", 200)
    };
    let result = match_candidates(&ev5, vec![fifth(single()), release_of(fifth(song()), "Album", "2023-09-01", 5)]);
    assert_eq!(result.best().unwrap().metadata.release_type.as_deref(), Some("Album"));
    // The single's own file (one track, or nothing that says more) keeps
    // the earliest release.
    for total in [Some(1), None] {
        let ev = LocalEvidence {
            track_no: Some(1),
            track_total: total,
            ..evidence("Рисунки", "Artist", "Рисунки", 200)
        };
        let result = match_candidates(&ev, vec![album(), single()]);
        assert_eq!(result.best().unwrap().metadata.release_type.as_deref(), Some("Single"), "{total:?}");
    }
    // The album's song at another place is not the file's.
    let elsewhere = release_of(song(), "Album", "2023-09-01", 7);
    let ev3 = LocalEvidence { track_no: Some(3), ..evidence("Рисунки", "Artist", "", 200) };
    let result = match_candidates(&ev3, vec![single(), elsewhere]);
    assert_eq!(result.best().unwrap().metadata.release_type.as_deref(), Some("Single"));
}

/// Two editions of an album: the earliest still wins the tie.
#[test]
fn a_tie_between_editions_of_an_album_goes_to_the_earliest() {
    let song = || mb("rec-1", "Heroes", "David Bowie", "Heroes", 370);
    let ev = LocalEvidence {
        track_no: Some(3),
        track_total: Some(10),
        ..evidence("Heroes", "David Bowie", "Heroes", 370)
    };
    let result = match_candidates(
        &ev,
        vec![release_of(song(), "Album", "1999-01-01", 3), release_of(song(), "Album", "1977-10-14", 3)],
    );
    assert_eq!(result.best().unwrap().metadata.date.as_deref(), Some("1977-10-14"));
}

#[test]
fn nothing_to_choose_from_is_unsure_and_empty() {
    let ev = evidence("Heroes", "David Bowie", "Heroes", 370);
    let result = match_candidates(&ev, vec![]);
    assert_eq!(result.confidence, Confidence::Unsure);
    assert!(result.best().is_none());
    assert!(result.options.is_empty());
}

#[test]
fn candidates_from_a_search_hit_carry_release_and_track_position() {
    #[derive(serde::Deserialize)]
    struct Search {
        recordings: Vec<Recording>,
    }
    let search: Search = serde_json::from_str(fixture!("mb_recording_search.json")).unwrap();
    let cands = candidates_from_recording(&search.recordings[0]);
    assert_eq!(cands.len(), 1);
    let m = &cands[0].metadata;
    assert_eq!(m.album.as_deref(), Some("Группа крови"));
    assert_eq!((m.disc_no, m.track_no), (Some(1), Some(9)));
    assert_eq!(m.release_type.as_deref(), Some("Album"));
    assert_eq!(m.date.as_deref(), Some("1988-06-01"));
    assert_eq!(m.isrcs, ["RUA011100001"]);
    assert_eq!(
        cands[0].ids.release_group_mbid.as_deref(),
        Some("7c5d4a3b-1e2f-4a6b-8c9d-0e1f2a3b4c5d")
    );
    // A recording without releases still yields one candidate.
    let live = candidates_from_recording(&search.recordings[1]);
    assert_eq!(live.len(), 1);
    assert!(live[0].ids.release_mbid.is_none());
    assert_eq!(
        live[0].metadata.disambiguation.as_deref(),
        Some("live, 1990-06-24: Luzhniki")
    );
}

#[test]
fn candidates_from_a_release_pick_the_track_by_number() {
    let release: Release = serde_json::from_str(fixture!("mb_release_lookup.json")).unwrap();
    let cands = candidates_from_release(&release);
    assert_eq!(cands.len(), 2);
    assert_eq!(cands[1].metadata.track_no, Some(2));
    assert_eq!(cands[1].metadata.label.as_deref(), Some("Мелодия"));

    // The local file says: track 2 of this album, tags fine.
    let mut ev = evidence("Группа крови", "Кино", "Группа крови", 284);
    ev.track_no = Some(2);
    ev.disc_no = Some(1);
    let result = match_candidates(&ev, cands.clone());
    assert_eq!(result.confidence, Confidence::Sure);
    assert_eq!(
        result.best().unwrap().ids.recording_mbid.as_deref(),
        Some("b1a9c0e9-d987-4042-ae91-78d6a3267d69")
    );
    // The right track number is worth a little: wrong number scores lower.
    let right = result.best().unwrap().score;
    ev.track_no = Some(7);
    let wrong = match_candidates(&ev, cands).best().unwrap().score;
    assert!(wrong < right);
}

#[test]
fn recording_query_is_built_from_cleaned_evidence() {
    let ev = LocalEvidence {
        title: Some("Heroes (Remastered 2017)".into()),
        artist: Some("David Bowie feat. Brian Eno".into()),
        album: Some("Heroes [Deluxe Edition]".into()),
        duration: Some(secs(370)),
        ..LocalEvidence::default()
    };
    let q = ev.recording_query(true).unwrap();
    assert_eq!(
        q.to_lucene(),
        r#"recording:"Heroes" AND artist:"David Bowie" AND release:"Heroes" AND dur:[360000 TO 380000]"#
    );
    assert!(
        !ev.recording_query(false)
            .unwrap()
            .to_lucene()
            .contains("release:")
    );
    assert!(LocalEvidence::default().recording_query(true).is_none());
}

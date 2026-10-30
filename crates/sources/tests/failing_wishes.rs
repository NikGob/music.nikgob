//! The wishes the old torrent search found nothing for (the user's
//! «Хочу скачать» on 2026-10-30, «Не нашёл раздачу» for 42 of ~80), run
//! through the new search with answers like the sources give: which
//! queries go out, and what each source's offer turns into. No network.
//!
//! `cargo test -p nikgob-sources --test failing_wishes -- --nocapture`
//! prints the queries of every wish.

use std::sync::{Arc, Mutex};

use nikgob_core::indexer::{BoxFuture, Candidate, CandidateHandle, Indexer, IndexerError, SearchQuery, TorrentPayload};
use nikgob_core::model::Codec;
use nikgob_core::source::{Format, Offer, OfferScope, Source, SourceKey, Want};
use nikgob_sources::select::{pick, rank};
use nikgob_sources::text::{catalogue_queries, is_soundtrack, torrent_queries};
use nikgob_sources::torrent::{IndexerSource, pick_files};

mod common;
use common::{WISHES, want_of};

#[test]
fn every_failing_wish_gets_queries_without_credit_noise() {
    for w in WISHES {
        let want = want_of(w);
        let tq = torrent_queries(&want, 5);
        let cq = catalogue_queries(&want);
        println!("{} — {}\n  torrents: {tq:?}\n  services: {cq:?}", w.1, w.2);
        assert!(!tq.is_empty() && !cq.is_empty(), "{w:?}");
        for q in tq.iter().chain(&cq) {
            assert!(!q.contains("и ещё"), "{q}");
            assert!(!q.contains("//"), "{q}");
            assert!(!q.to_lowercase().contains("feat"), "{q}");
            assert!(!q.contains(", David"), "one artist: {q}");
        }
    }
}

#[test]
fn soundtracks_among_them_go_to_khinsider_too() {
    let ost: Vec<&str> = WISHES
        .iter()
        .filter(|w| is_soundtrack(w.3) || nikgob_sources::text::franchise_of(w.2).is_some())
        .map(|w| w.2)
        .collect();
    assert!(ost.contains(&"Who's Ready for Tomorrow"));
    assert!(ost.contains(&"沈黙の魔女 - Chinmoku no Majo"));
    assert!(ost.contains(&"Meine Freundin ist kaputt"), "its album names the game");
}

struct Recorded {
    asked: Mutex<Vec<String>>,
    answers: Vec<(&'static str, Vec<&'static str>)>,
}

impl Indexer for Recorded {
    fn name(&self) -> &str {
        "Prowlarr"
    }
    fn search(&self, q: &SearchQuery) -> BoxFuture<'_, Result<Vec<Candidate>, IndexerError>> {
        self.asked.lock().unwrap().push(q.text.clone());
        let found: Vec<Candidate> = self
            .answers
            .iter()
            .filter(|(query, _)| *query == q.text)
            .flat_map(|(_, titles)| titles.iter())
            .map(|t| Candidate {
                id: (*t).into(),
                title: (*t).into(),
                indexer_name: "nyaa.si".into(),
                size_bytes: Some(900_000_000),
                seeders: Some(12),
                leechers: None,
                published: None,
                categories: vec![3040],
                info_url: None,
                info_hash: None,
                handle: CandidateHandle::default(),
            })
            .collect();
        Box::pin(async move { Ok(found) })
    }
    fn fetch_torrent(&self, _: &Candidate) -> BoxFuture<'_, Result<TorrentPayload, IndexerError>> {
        Box::pin(async { Err(IndexerError::NothingToFetch) })
    }
}

#[tokio::test]
async fn a_japanese_single_is_found_in_the_artists_discography() {
    // The old search asked "Yorushika 火星人 - Martian" and nothing else
    // about the song; nyaa names the band's discography, not the song.
    let idx = Arc::new(Recorded {
        asked: Mutex::new(Vec::new()),
        answers: vec![("Yorushika", vec!["[FLAC] Yorushika (ヨルシカ) Discography 2017-2025", "Yorushika - 盗作 (Plagiarism) [MP3 320]"])],
    });
    let want = Want { track_no: 2, ..want_of(&WISHES[24]) };
    let offers = IndexerSource::new(idx.clone()).search(&want).await.unwrap();
    assert_eq!(idx.asked.lock().unwrap().last().map(String::as_str), Some("Yorushika"), "the artist alone last");
    let ranked = rank(offers, &want, &[SourceKey::Torrents]);
    let first = pick(&ranked, None).map(|i| &ranked[i].offer).unwrap();
    assert_eq!(first.scope, OfferScope::Collection);
    // Its list: the single is one file of it.
    let files = vec![
        (0, "Yorushika Discography/2023 - 幻燈/03 - 夏の肖像.flac".into(), 30),
        (1, "Yorushika Discography/2023 - 二人称 (second person)/02 - 火星人.flac".into(), 30),
        (2, "Yorushika Discography/2023 - 二人称 (second person)/05 - 火星人 (Live).flac".into(), 30),
    ];
    assert_eq!(pick_files(&files, &want), Some(vec![1]));
}

#[tokio::test]
async fn a_soundtrack_torrent_named_after_the_game_is_taken() {
    let idx = Arc::new(Recorded {
        asked: Mutex::new(Vec::new()),
        answers: vec![("Secrets of the Silent Witch", vec!["[FLAC] Silent Witch: Chinmoku no Majo no Kakushigoto Original Soundtrack", "Secrets of the Silent Witch Original Soundtrack (2025) [FLAC 24/48]"])],
    });
    let want = want_of(&WISHES[25]);
    let offers = IndexerSource::new(idx.clone()).search(&want).await.unwrap();
    let ranked = rank(offers, &want, &[SourceKey::Torrents]);
    let best = &ranked[pick(&ranked, None).unwrap()].offer;
    assert_eq!(best.title, "Secrets of the Silent Witch Original Soundtrack (2025) [FLAC 24/48]");
}

#[test]
fn a_catalogue_has_the_russian_and_japanese_singles_torrents_lack() {
    // What a free service answers for three of them, as Qobuz lists it.
    let flac = |artist: &str, title: &str, secs: u32| Offer {
        source: SourceKey::Qobuz,
        provider: "Qobuz".into(),
        id: title.into(),
        title: format!("{artist} — {title}"),
        artist: Some(artist.into()),
        track_title: Some(title.into()),
        album: Some(title.into()),
        duration_ms: Some(secs * 1000),
        isrc: None,
        scope: OfferScope::Track,
        format: Some(Format { bit_depth: Some(24), sample_rate: Some(44_100), ..Format::new(Codec::Flac) }),
        size_bytes: None,
        seeders: None,
        info_url: None,
        handle: String::new(),
    };
    for (i, artist, title, secs) in [(12, "MORGENSHTERN", "Цветок", 156), (38, "sakanaction", "怪獣", 253), (16, "MellSher", "Разгром", 143)] {
        let want = want_of(&WISHES[i]);
        let ranked = rank(vec![flac(artist, title, secs), flac("Cover Band", title, secs)], &want, &[]);
        assert_eq!(ranked.len(), 1, "{title}: the cover band is out");
        assert_eq!(pick(&ranked, None), Some(0), "{title}");
    }
}

/// A Tidal track as the hifi-api answered on 2026-10-31.
fn tidal(artist: &str, title: &str, album: &str, secs: Option<u32>, scope: OfferScope) -> Offer {
    Offer {
        source: SourceKey::Tidal,
        provider: "Tidal".into(),
        id: title.into(),
        title: format!("{artist} — {}", if scope == OfferScope::Track { title } else { album }),
        artist: Some(artist.into()),
        track_title: (scope == OfferScope::Track).then(|| title.into()),
        album: Some(album.into()),
        duration_ms: secs.map(|s| s * 1000),
        isrc: None,
        scope,
        format: Some(Format { bit_depth: Some(16), sample_rate: Some(44_100), ..Format::new(Codec::Flac) }),
        size_bytes: None,
        seeders: None,
        info_url: None,
        handle: String::new(),
    }
}

#[test]
fn a_sped_up_upload_offers_the_performers_song_to_confirm() {
    // «Gg2» uploaded «Комсомольск - Мозги & деньги //speed up//»: the
    // release is Комсомольск's, at its own length.
    let want = want_of(&WISHES[7]);
    let ranked = rank(vec![tidal("Комсомольск", "Мозги & деньги", "Комсомольск-1", Some(149), OfferScope::Track)], &want, &[]);
    assert_eq!(ranked.len(), 1);
    assert!(!ranked[0].verdict.sure);
    assert_eq!(pick(&ranked, None), None, "asked in «Подтверди», not taken");
    // Another performer's song of that name is still not it.
    let ranked = rank(vec![tidal("Кто-то", "Мозги & деньги", "X", Some(149), OfferScope::Track)], &want, &[]);
    assert!(ranked.is_empty());
}

#[test]
fn a_compilation_of_various_artists_is_taken_by_its_name() {
    let want = want_of(&WISHES[41]);
    let album = "Spider-Man: Into the Spider-Verse (Deluxe Edition / Soundtrack From & Inspired By The Motion Picture)";
    let ranked = rank(
        vec![
            tidal("Daddyphatsnaps", "", "Spider-Man Into the Spiderverse Rap", None, OfferScope::Album),
            tidal("Various Artists", "", album, None, OfferScope::Album),
        ],
        &want,
        &[],
    );
    let first = pick(&ranked, None).map(|i| &ranked[i].offer);
    assert_eq!(first.and_then(|o| o.album.as_deref()), Some(album));
}

#[test]
fn what_the_catalogue_lacks_stays_not_found() {
    // LAZZY2WICE «Summer 15/16»: Tidal answers audiobook chapters.
    let want = want_of(&WISHES[10]);
    let got = vec![
        tidal("Lia Bergman", "Kapitel 15.10 & Kapitel 16.1 - Hot Swedish Summer", "Hot Swedish Summer", Some(185), OfferScope::Track),
        tidal("Minna Howard", "Chapter 15.6 & Chapter 16.1 - That Long Lost Summer", "That Long Lost Summer", Some(126), OfferScope::Track),
    ];
    assert!(rank(got, &want, &[]).is_empty());
    // sakanaction «怪獣»: other songs of the band, and other bands' «怪獣».
    let want = want_of(&WISHES[38]);
    let got = vec![
        tidal("Sakanaction", "Shin Takara Jima", "834.194", Some(305), OfferScope::Track),
        tidal("G.E.L.O.", "怪獣", "怪獣", Some(148), OfferScope::Track),
        tidal("夢見クジラ", "怪獣", "TVアニメ『リアデイルの大地にて』Original Soundtrack", Some(98), OfferScope::Track),
    ];
    assert!(rank(got, &want, &[]).is_empty());
}

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
use nikgob_core::source::{Format, Offer, OfferScope, Source, SourceKey, Want, WantScope};
use nikgob_sources::select::{pick, rank};
use nikgob_sources::text::{catalogue_queries, is_soundtrack, torrent_queries};
use nikgob_sources::torrent::{IndexerSource, pick_files};

/// (scope, artist as the wish shows it, title, album, seconds)
const WISHES: &[(&str, &str, &str, &str, u32)] = &[
    ("track", "9Lana", "ヨフケノパレエド - Midnight Parade", "Midnight Parade", 194),
    ("track", "Con Fetti Pesto, David Münnich, Alexx Grimm", "Meine Freundin ist kaputt", "Das Nennt Ihr Himmel (Supraland Six Inches Under Credits Song)", 188),
    ("track", "Dominic Fike", "Babydoll", "Don't Forget About Me, Demos", 98),
    ("track", "Ellie Minibot", "I Don't Want to Be an Engineer", "I Don't Want to Be an Engineer", 269),
    ("track", "Eve", "Underdog", "Underdog", 194),
    ("track", "Eve", "Wind's Anthem (feat. suis From Yorushika)", "Wind's Anthem", 223),
    ("track", "Eve", "Avant", "Kaizin", 205),
    ("track", "Gg2", "Комсомольск - Мозги & деньги //speed up//", "", 122),
    ("track", "IcyBeast", "7 Weeks & 3 Days", "7 Weeks & 3 Days", 169),
    ("track", "JVKE", "her", "her", 172),
    ("track", "LAZZY2WICE", "Summer 15/16", "Summer 15/16", 161),
    ("track", "MORGENSHTERN", "Turn It on! (feat. Palc)", "Turn It on!", 208),
    ("track", "MORGENSHTERN", "Цветок", "Цветок", 156),
    ("album", "MORGENSHTERN", "ЛЕГЕНДАРНАЯ ПЫЛЬ", "ЛЕГЕНДАРНАЯ ПЫЛЬ", 0),
    ("track", "MORGENSHTERN, ЛСП", "Зеленоглазые деффки!", "Улыбнись, дурак!", 205),
    ("track", "MellSher", "X", "X", 151),
    ("track", "MellSher, и ещё 1 исполнитель", "Разгром", "Разгром", 143),
    ("track", "Nakimushi", "Shake It Now. (feat. Ado)", "rendez-vous", 197),
    ("track", "RAT BOY, IBDY", "Who's Ready for Tomorrow", "Cyberpunk 2077: Radio, Vol. 2 (Original Soundtrack)", 117),
    ("track", "Rosa Walton, Hallie Coggins", "I Really Want to Stay at Your House", "Cyberpunk 2077: Radio, Vol. 2 (Original Soundtrack)", 247),
    ("track", "SLAVA MARLOW", "Каблуки", "ТУЗИК", 102),
    ("track", "SLAVA MARLOW", "О2", "ТУЗИК", 117),
    ("track", "SLAVA MARLOW", "Детские травмы (feat. Платина и Лера Маяк)", "ТУЗИК", 161),
    ("track", "SLAVA MARLOW", "Я в деле", "ТУЗИК", 94),
    ("track", "Yorushika", "火星人 - Martian", "second person", 233),
    ("track", "Yoshihisa Kato", "沈黙の魔女 - Chinmoku no Majo", "Secrets of the Silent Witch Original Soundtrack", 184),
    ("track", "aksfx", "Big Walk Theme (Jumping Voices)", "Big Walk Theme (Jumping Voices)", 226),
    ("album", "aksfx", "Big Walk Theme (Jumping Voices)", "Big Walk Theme (Jumping Voices)", 0),
    ("track", "emptiness", "7 weeks & 3 days (out of tune piano)", "7 weeks & 3 days (out of tune piano)", 61),
    ("track", "frutiger pm", "cloud city", "dreamcore", 139),
    ("track", "gotlibgotlibgotlib", "все хотят меня", "все хотят меня", 119),
    ("track", "gotlibgotlibgotlib, SABU", "агентство насилия™", "агентство насилия™", 135),
    ("track", "madk1d", "так похуй", "sexyswag", 108),
    ("track", "my head is empty", "personal space", "ambient vol. 12", 101),
    ("album", "mzlff", "не забытое старое", "не забытое старое", 0),
    ("track", "nyan.mp3", "Я не интроверт", "Я не интроверт", 100),
    ("album", "nyan.mp3", "nyan.pop", "nyan.pop", 0),
    ("track", "s0rrow", "stalk ur socials", "very sorrow days", 79),
    ("track", "sakanaction", "怪獣", "怪獣", 253),
    ("track", "Дайте танк (!)", "Маленький", "Маленький", 245),
    ("track", "Пошлая Молли", "Контракт", "Контракт", 205),
    ("album", "Разные исполнители", "Spider-Man: Into the Spider-Verse (Deluxe Edition / Soundtrack From & Inspired By The Motion Picture)", "Spider-Man: Into the Spider-Verse (Deluxe Edition / Soundtrack From & Inspired By The Motion Picture)", 0),
];

fn want_of(w: &(&str, &str, &str, &str, u32)) -> Want {
    let (scope, artist, title, album, secs) = *w;
    // The runtime takes the credit's names apart; the wish's display line
    // ("MellSher, и ещё 1 исполнитель") is what the old search sent.
    Want {
        artists: vec![artist.to_owned()],
        title: (scope == "track").then(|| title.to_owned()),
        album: (!album.is_empty()).then(|| album.to_owned()),
        scope: if scope == "album" { WantScope::Album } else { WantScope::Track },
        duration_ms: (secs > 0).then_some(secs * 1000),
        ..Want::default()
    }
}

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

//! `NIKGOB_SHOT_STEPS` `fake:<what>`: states the demo cannot reach by
//! itself, laid over the runtime's view state for screenshots only (the
//! runtime never sees them).
//!
//! - `fake:discography` — the open library artist gets a «Нет в библиотеке»
//!   section from the fake Spotify catalog.
//! - `fake:wishlist` — a wishlist with every status.
//! - `fake:confirm` — a «Хочу скачать» track in «Подтверди» whose offers
//!   have long release names.
//! - `fake:album-status` — the first stream album cards on screen (the
//!   discography, a service's artist page, search) are queued, downloading,
//!   fully local and partly local, in that order.
//! - `fake:buffering` — the player waits for a stream.
//! - `fake:audio:youtube` / `fake:audio:spotify` — where the sound comes from.
//! - `fake:link` — the open playlist follows a Spotify playlist.
//! - `fake:lyrics` — word-synced lyrics (LyricsPlus) for whatever plays:
//!   the demo has no network to fetch any for a stream track.
//! - `fake:paused:youtube` / `fake:paused:spotify` — the service asked to
//!   wait (until 14:05).
//! - `fake:search-wait:spotify` / `fake:search-wait:youtube` — the service
//!   answered the search with a short rate-limit pause: its section loads
//!   and says it answers late.
//! - `fake:search-busy:spotify` / `fake:search-busy:youtube` — a long
//!   pause: the section says the service is overloaded while the other
//!   answers.
//! - `fake:torrents` — Prowlarr looks set up (the demo has none).
//! - `fake:no-sound` — Spotify signed in with Premium, the sound consent
//!   not given yet.
//! - `fake:empty-home` — Главная of an empty library (the first launch).
//! - `fake:find-song-off` — «Найти песню…» cannot replace on YouTube Music
//!   (as when signed out).
//! - `fake:analysis` — the sound analysis is at 40% (the demo's vectors
//!   are all there).
//! - `fake:extending` — the radio of the queue is looking for more.
//! - `fake:no-recs` — Главная before any analysis and history: no moods,
//!   style mixes or discoveries.
//! - `fake:bento` — the board of Главная full: the daylist and all five
//!   tiles, with the demo library's covers. `fake:bento:2` — the daylist
//!   and two tiles; `fake:bento:no-daylist` — no daylist, three tiles
//!   («Твой микс» takes the big spot).
//! - `fake:keep` — the playing song is offered to be kept, as a new song
//!   heard out twice is («Оставить у себя «…»?»).
//! - `fake:lyrics-texts` — the playing song has four texts for «Текст
//!   песни»: LyricsPlus' own word timing, LRCLIB's lines of another take,
//!   Genius' plain text with words made from the recording, lrc.red lines.
//! - `fake:craft:decode|separate|listen|align|download` — timing for
//!   Genius' text is being made, at that stage; `fake:craft:done` — made,
//!   to check; `fake:craft:failed` — it could not be made.
//! - `fake:lyrics-home:words|online|saved|clip` — with `fake:lyrics-texts`,
//!   the shown text: Genius' with word timing the player made (to make
//!   again or save), LRCLIB's lines only online, a text saved on the
//!   computer, or a clip whose song is unclear (timing cannot be made).
//! - `fake:align-models` — Settings → «Текст песен»: models ready, one
//!   downloading, one missing; the background pass halfway; on the GPU.

use nikgob_core::streaming::{FakeService, PlaylistSyncMode, RemoteId, Service};
use nikgob_view::{
    DiscographyView, PlaylistSyncView, Status, StreamAlbumCard, ViewState, WishlistGroup, WishlistId,
    WishlistOrigin, WishlistRow, WishlistScope, WishlistStatus as St, WishlistView,
};

#[derive(Clone, Debug, Default)]
pub struct Fakes {
    discography: bool,
    wishlist: bool,
    confirm: bool,
    album_status: bool,
    buffering: bool,
    audio: Option<Service>,
    link: bool,
    lyrics: bool,
    paused: Option<Service>,
    search_wait: Option<(Service, bool)>,
    torrents: bool,
    no_sound: bool,
    empty_home: bool,
    find_song_off: bool,
    analysis: bool,
    extending: bool,
    no_recs: bool,
    /// `fake:bento`: how many tiles, and whether there is a daylist.
    bento: Option<(usize, bool)>,
    keep: bool,
    lyrics_texts: bool,
    craft: Option<String>,
    lyrics_home: Option<String>,
    align_models: bool,
    chip: Option<String>,
}

impl Fakes {
    /// Takes `arg` of a `fake:` step; false when it is unknown.
    pub fn add(&mut self, arg: &str) -> bool {
        match arg {
            "discography" => self.discography = true,
            "wishlist" => self.wishlist = true,
            "confirm" => self.confirm = true,
            "album-status" => self.album_status = true,
            "buffering" => self.buffering = true,
            "audio:youtube" => self.audio = Some(Service::Youtube),
            "audio:spotify" => self.audio = Some(Service::Spotify),
            "link" => self.link = true,
            "lyrics" => self.lyrics = true,
            "paused:youtube" => self.paused = Some(Service::Youtube),
            "paused:spotify" => self.paused = Some(Service::Spotify),
            "search-wait:spotify" => self.search_wait = Some((Service::Spotify, false)),
            "search-busy:spotify" => self.search_wait = Some((Service::Spotify, true)),
            "search-wait:youtube" => self.search_wait = Some((Service::Youtube, false)),
            "search-busy:youtube" => self.search_wait = Some((Service::Youtube, true)),
            "torrents" => self.torrents = true,
            "no-sound" => self.no_sound = true,
            "empty-home" => self.empty_home = true,
            "find-song-off" => self.find_song_off = true,
            "analysis" => self.analysis = true,
            "extending" => self.extending = true,
            "no-recs" => self.no_recs = true,
            "bento" => self.bento = Some((5, true)),
            "bento:2" => self.bento = Some((2, true)),
            "bento:no-daylist" => self.bento = Some((3, false)),
            "keep" => self.keep = true,
            "lyrics-texts" => self.lyrics_texts = true,
            "align-models" => self.align_models = true,
            c if c.starts_with("craft:") => self.craft = Some(c["craft:".len()..].to_owned()),
            c if c.starts_with("lyrics-home:") => self.lyrics_home = Some(c["lyrics-home:".len()..].to_owned()),
            c if c.starts_with("chip:") => self.chip = Some(c["chip:".len()..].to_owned()),
            _ => return false,
        }
        true
    }

    pub fn any(&self) -> bool {
        self.discography
            || self.wishlist
            || self.confirm
            || self.album_status
            || self.buffering
            || self.audio.is_some()
            || self.link
            || self.lyrics
            || self.paused.is_some()
            || self.search_wait.is_some()
            || self.torrents
            || self.no_sound
            || self.empty_home
            || self.find_song_off
            || self.analysis
            || self.extending
            || self.no_recs
            || self.bento.is_some()
            || self.keep
            || self.lyrics_texts
            || self.craft.is_some()
            || self.lyrics_home.is_some()
            || self.align_models
            || self.chip.is_some()
    }

    /// `state` with the fakes laid over it.
    pub fn apply(&self, state: &ViewState) -> ViewState {
        let mut s = state.clone();
        if self.discography
            && let Some(a) = s.artist.as_mut()
        {
            a.discography = discography(&a.card.name, &state.library.releases);
        }
        if self.wishlist {
            s.downloads.wishlist = wishlist();
        }
        if self.confirm {
            s.downloads.confirm = vec![confirm()];
        }
        if self.keep
            && let Some(t) = s.player.current.clone()
            && !s.toasts.iter().any(|x| x.id == u64::MAX)
        {
            let text = format!("Оставить у себя «{}»?", t.title);
            s.toasts.push(nikgob_view::Toast {
                id: u64::MAX,
                kind: nikgob_view::ToastKind::guess(&text),
                text,
                action: Some((
                    "♡ Оставить".into(),
                    nikgob_view::Command::Like(nikgob_core::model::LikeTarget::Track(t.id), true),
                )),
                open: None,
                at: std::time::Instant::now(),
                long: true,
                again: 0,
            });
        }

        if self.album_status {
            let mut rows = Vec::new();
            if let Some(a) = s.artist.as_mut() {
                album_statuses(&mut a.discography.albums, &mut rows);
            }
            if let Some(p) = s.stream_page.as_mut() {
                album_statuses(&mut p.albums, &mut rows);
            }
            for x in &mut s.search.streams {
                album_statuses(&mut x.albums, &mut rows);
            }
            s.downloads.wishlist.items.extend(rows);
        }
        if self.buffering {
            s.player.status = Status::Buffering;
        }
        if self.find_song_off
            && let Some(f) = s.find_song.as_mut()
        {
            f.youtube_off = Some("Нужен вход в YouTube Music".into());
        }
        if let Some(svc) = self.paused
            && let Some(a) = s.accounts.iter_mut().find(|a| a.service == svc)
        {
            a.paused_until = Some(0);
            a.paused_until_hm = Some("14:05".into());
        }
        if let Some((svc, long)) = self.search_wait {
            if !s.search.streams.iter().any(|x| x.service == svc) {
                s.search.streams.insert(0, nikgob_view::StreamSearchSection::new(svc));
            }
            let name = svc.display_name();
            let other = match svc {
                Service::Spotify | Service::Deezer => Service::Youtube,
                Service::Youtube => Service::Deezer,
            }
            .display_name();
            for x in s.search.streams.iter_mut().filter(|x| x.service == svc) {
                x.error = None;
                x.tracks.clear();
                x.albums.clear();
                x.artists.clear();
                x.playlists.clear();
                // The runtime's words (streams/search.rs `wait_note`).
                if long {
                    x.searching = false;
                    x.note = Some(format!("{name} сейчас перегружен до 14:05 — ищу в {other}"));
                } else {
                    x.searching = true;
                    x.note = Some(format!("{name} отвечает с задержкой — результаты появятся сами"));
                }
            }
        }
        if self.torrents {
            s.prefs.prowlarr_configured = true;
            s.indexer_check = Some(nikgob_view::IndexerCheck {
                checking: false,
                ok: true,
                text: "Prowlarr отвечает · 12 трекеров".into(),
            });
            // «Источники скачивания»: the sources a build has, one of
            // them down lately.
            s.downloads.sources = s
                .prefs
                .source_order()
                .iter()
                .map(|o| nikgob_view::SourceView {
                    key: o.key,
                    name: o.key.display_name().into(),
                    available: o.key != nikgob_view::SourceKey::Soulseek,
                    status: (o.key == nikgob_view::SourceKey::Tidal).then(|| "Не пускает — попробую позже".into()),
                })
                .collect();
        }
        if self.analysis {
            s.analysis = nikgob_view::AnalysisProgress { files_done: 1240, files_total: 2600, streams_done: 0, streams_total: 500 };
        }
        if self.extending
            && let Some(r) = s.queue.radio.as_mut()
        {
            r.extending = true;
        }
        if self.no_recs {
            s.home.moods.clear();
            s.home.style_mixes.clear();
            s.home.discover.clear();
            s.home.new_releases.clear();
        }
        if let Some((n, daylist)) = self.bento {
            bento(&mut s, n, daylist);
        }
        if self.empty_home {
            s.home = nikgob_view::HomeView { greeting_hour: s.home.greeting_hour, ..Default::default() };
        }
        if self.no_sound
            && let Some(a) = s.accounts.iter_mut().find(|a| a.service == Service::Spotify)
        {
            a.audio = false;
            if let nikgob_view::Account::LoggedIn { premium, .. } = &mut a.account {
                *premium = Some(true);
            }
        }
        if let Some(svc) = self.audio {
            s.player.audio_service = Some(svc);
            // What that service's sound is.
            s.player.audio_quality = Some(match svc {
                Service::Youtube | Service::Deezer => nikgob_core::streaming::quality_label("Opus", 160, true),
                Service::Spotify => nikgob_core::streaming::quality_label("Vorbis", 320, false),
            });
        }
        if self.lyrics {
            s.lyrics = word_lyrics(state.player.current.as_ref().map(|t| t.id));
        }
        if self.lyrics_texts || self.craft.is_some() || self.lyrics_home.is_some() {
            let track = state.player.current.as_ref().map(|t| t.id);
            s.lyrics = texts_lyrics(track);
            if let Some(home) = self.lyrics_home.as_deref() {
                lyrics_home(&mut s.lyrics, home);
            }
            if s.info.path.is_none() {
                s.info.path = Some("song.opus".into());
            }
            if let (Some(stage), Some(track)) = (self.craft.as_deref(), track) {
                craft(&mut s, track, stage);
            }
        }
        if self.align_models {
            align_models(&mut s);
        }
        if let Some(name) = self.chip.as_deref() {
            chip_state(&mut s, name);
        }
        if self.link
            && let Some(p) = s.playlist.as_mut()
        {
            p.sync = Some(PlaylistSyncView {
                remote: RemoteId::new(Service::Spotify, "sp-pl-evening"),
                mode: PlaylistSyncMode::Follow,
                synced: Some("5 мин назад".into()),
                syncing: false,
                error: None,
            });
        }
        s
    }
}

/// Lines of three words, one word a second, a line every four seconds.
fn word_lyrics(track: Option<nikgob_view::TrackId>) -> nikgob_view::LyricsView {
    use nikgob_core::lyrics::{LyricLine, LyricWord, LyricsSource};
    use std::time::Duration;
    let text = [
        "Тёплое место на улице ждёт",
        "Группа крови на рукаве",
        "Мой порядковый номер",
        "Пожелай мне удачи в бою",
        "Не остаться в этой траве",
        "Пожелай мне удачи",
    ];
    let ms = Duration::from_millis;
    let lines = (0..40)
        .map(|i| {
            let line = text[i % text.len()];
            let start = 1500 + 4000 * i as u64;
            let words: Vec<&str> = line.split(' ').collect();
            let step = 3200 / words.len() as u64;
            LyricLine {
                start: ms(start),
                end: Some(ms(start + 3400)),
                text: line.to_owned(),
                words: words
                    .iter()
                    .enumerate()
                    .map(|(j, w)| LyricWord {
                        start: ms(start + step * j as u64),
                        end: ms(start + step * (j as u64 + 1)),
                        text: if j + 1 < words.len() { format!("{w} ") } else { (*w).to_owned() },
                    })
                    .collect(),
                ..Default::default()
            }
        })
        .collect();
    nikgob_view::LyricsView {
        track,
        lines,
        plain: None,
        loading: false,
        source: Some(LyricsSource::LyricsPlus),
        picked: false,
        sources: Vec::new(),
        ..Default::default()
    }
}

/// The texts of a song whose only text is Genius' plain one, shown with
/// the line timing the player made (`fake:lyrics-texts`): LyricsPlus and
/// lrc.red have none, LRCLIB's is another version's.
fn texts_lyrics(track: Option<nikgob_core::model::TrackId>) -> nikgob_view::LyricsView {
    use nikgob_core::lyrics::{LyricLine, LyricsSource};
    use nikgob_view::{LyricsAvail, LyricsOption, LyricsSync, LyricsText};
    use std::time::Duration;
    let own = word_lyrics(track);
    let lines_of = |shift: u64| -> Vec<LyricLine> {
        own.lines
            .iter()
            .map(|l| LyricLine { start: l.start.saturating_sub(Duration::from_millis(shift)), end: None, words: Vec::new(), ..l.clone() })
            .collect()
    };
    let plain: String = own.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n");
    let made = lines_of(0);
    let texts = vec![
        LyricsText { source: Some(LyricsSource::Lrclib), lines: lines_of(2140), sync: Some(LyricsSync::Line), fits: Some(false), ..Default::default() },
        LyricsText { source: Some(LyricsSource::Genius), plain: Some(plain), sync: Some(LyricsSync::Plain), fits: Some(true), made: Some(made.clone()), ..Default::default() },
    ];
    let option = |source, state, fits, made| LyricsOption { source, state, fits, made };
    nikgob_view::LyricsView {
        lines: made,
        source: Some(LyricsSource::Genius),
        made: true,
        sources: vec![
            option(LyricsSource::LyricsPlus, LyricsAvail::Missing, None, false),
            option(LyricsSource::LrcRed, LyricsAvail::Missing, None, false),
            option(LyricsSource::Lrclib, LyricsAvail::Ready(LyricsSync::Line), Some(false), false),
            option(LyricsSource::Genius, LyricsAvail::Ready(LyricsSync::Plain), Some(true), true),
        ],
        texts,
        home: nikgob_view::LyricsHome::Library,
        craft: nikgob_view::LyricsCraft::Ready,
        savable: true,
        ..own
    }
}

/// Where the shown text of [`texts_lyrics`] lives (`fake:lyrics-home:…`).
fn lyrics_home(l: &mut nikgob_view::LyricsView, name: &str) {
    use nikgob_core::lyrics::LyricsSource;
    use nikgob_view::{LyricsCraft, LyricsHome};
    match name {
        "words" => {
            l.lines = word_lyrics(l.track).lines;
            if let Some(o) = l.sources.iter_mut().find(|o| o.source == LyricsSource::Genius) {
                o.state = nikgob_view::LyricsAvail::Ready(nikgob_view::LyricsSync::Word);
            }
        }
        "online" => {
            l.source = Some(LyricsSource::Lrclib);
            l.lines = l.texts[0].lines.clone();
            l.made = false;
            l.home = LyricsHome::Online;
            l.savable = false;
        }
        "saved" => {
            l.source = Some(LyricsSource::File);
            l.lines = word_lyrics(l.track).lines;
            l.made = false;
            l.home = LyricsHome::Computer;
            l.savable = false;
        }
        "clip" => {
            l.craft = LyricsCraft::No("Подсветку не сделать: это клип, неясно, какая в нём песня".into());
            l.savable = false;
        }
        _ => {}
    }
}

/// Word timing for Genius' text asked for and at `stage`
/// (`fake:craft:download|decode|separate|listen|align|done|failed`); done,
/// the made words are shown.
fn craft(s: &mut ViewState, track: nikgob_core::model::TrackId, name: &str) {
    use nikgob_core::lyrics::LyricsSource;
    use nikgob_view::{AlignJob, AlignStage, Craft, LyricsSync};
    let (stage, progress, bytes) = match name {
        "download" => (AlignStage::Download, 0.42, Some((310_378_496, 743_440_384))),
        "decode" => (AlignStage::Decode, 0.0, None),
        "separate" => (AlignStage::Separate, 0.45, None),
        "listen" => (AlignStage::Listen, 0.6, None),
        _ => (AlignStage::Align, 1.0, None),
    };
    let mut c = Craft { track, source: LyricsSource::Genius, target: LyricsSync::Word, lines: None, failed: None };
    match name {
        "done" => {
            // Applied at once: the words are the shown text, picked.
            let words = word_lyrics(Some(track)).lines;
            s.lyrics.lines = words.clone();
            s.lyrics.picked = true;
            if let Some(t) = s.lyrics.texts.iter_mut().find(|t| t.source == Some(LyricsSource::Genius)) {
                t.made = Some(words);
                t.saved = true;
            }
            return;
        }
        "failed" => c.failed = Some("Не удалось скачать модели".into()),
        _ => s.align.job = Some(AlignJob { track: Some(track), stage, progress, asked: true, bytes }),
    }
    s.align.craft = Some(c);
}

/// The text under the lyrics control in each state there is
/// (`fake:chip:genius-plain|genius-lines|lyricsplus-words|lrcred-syllables|file-lines|recognized|several`).
fn chip_state(s: &mut ViewState, name: &str) {
    use nikgob_core::lyrics::{LyricLine, LyricsSource};
    use nikgob_view::{LyricsAvail, LyricsOption, LyricsSync, LyricsText};
    let track = s.player.current.as_ref().map(|t| t.id);
    let words = word_lyrics(track).lines;
    let lines: Vec<LyricLine> = words.iter().map(|l| LyricLine { words: Vec::new(), ..l.clone() }).collect();
    // Syllables: every word cut in two.
    let syllables: Vec<LyricLine> = words
        .iter()
        .map(|l| {
            let mut l = l.clone();
            l.words = l
                .words
                .iter()
                .flat_map(|w| {
                    let cut = w.text.char_indices().nth(w.text.chars().count() / 2).map_or(w.text.len(), |(i, _)| i);
                    let mid = w.start + (w.end - w.start) / 2;
                    [
                        nikgob_view::LyricWord { start: w.start, end: mid, text: w.text[..cut].to_owned() },
                        nikgob_view::LyricWord { start: mid, end: w.end, text: w.text[cut..].to_owned() },
                    ]
                })
                .collect();
            l
        })
        .collect();
    let plain = lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n");
    let ready = |source, sync| LyricsOption { source, state: LyricsAvail::Ready(sync), fits: Some(true), made: false };
    let none = |source| LyricsOption { source, state: LyricsAvail::Missing, fits: None, made: false };
    let (source, shown, sources) = match name {
        "genius-plain" => (LyricsSource::Genius, Vec::new(), vec![none(LyricsSource::LyricsPlus), none(LyricsSource::Lrclib), ready(LyricsSource::Genius, LyricsSync::Plain)]),
        "genius-lines" => (LyricsSource::Genius, lines.clone(), vec![none(LyricsSource::LyricsPlus), none(LyricsSource::Lrclib), LyricsOption { made: true, ..ready(LyricsSource::Genius, LyricsSync::Plain) }]),
        "lyricsplus-words" => (LyricsSource::LyricsPlus, words.clone(), vec![ready(LyricsSource::LyricsPlus, LyricsSync::Word), none(LyricsSource::Lrclib)]),
        "lrcred-syllables" => (LyricsSource::LrcRed, syllables.clone(), vec![none(LyricsSource::LyricsPlus), ready(LyricsSource::LrcRed, LyricsSync::Syllable)]),
        "file-lines" => (LyricsSource::File, lines.clone(), vec![ready(LyricsSource::File, LyricsSync::Line), none(LyricsSource::LyricsPlus)]),
        "recognized" => (LyricsSource::Recognized, words.clone(), vec![none(LyricsSource::LyricsPlus), none(LyricsSource::Lrclib), none(LyricsSource::Genius), ready(LyricsSource::Recognized, LyricsSync::Word)]),
        // Several texts that light up differently: the list says how.
        _ => (
            LyricsSource::LyricsPlus,
            words.clone(),
            vec![
                ready(LyricsSource::LyricsPlus, LyricsSync::Word),
                ready(LyricsSource::LrcRed, LyricsSync::Syllable),
                ready(LyricsSource::Lrclib, LyricsSync::Line),
                LyricsOption { made: true, ..ready(LyricsSource::Genius, LyricsSync::Plain) },
            ],
        ),
    };
    let texts = sources
        .iter()
        .filter(|o| matches!(o.state, LyricsAvail::Ready(_)))
        .map(|o| LyricsText {
            source: Some(o.source),
            plain: (o.source == LyricsSource::Genius).then(|| plain.clone()),
            made: o.made.then(|| lines.clone()),
            fits: Some(true),
            ..Default::default()
        })
        .collect();
    s.lyrics = nikgob_view::LyricsView {
        track,
        plain: shown.is_empty().then(|| plain.clone()),
        lines: shown,
        source: Some(source),
        made: source == LyricsSource::Genius && name != "genius-plain",
        sources,
        texts,
        ..Default::default()
    };
    if s.info.path.is_none() {
        s.info.path = Some("song.opus".into());
    }
}

/// Settings → «Текст песен» with every model state (`fake:align-models`).
fn align_models(s: &mut ViewState) {
    use nikgob_view::{AlignModel, ModelState};
    let m = |key: &str, label: &str, size: u64, state| AlignModel { key: key.into(), label: label.into(), size, state };
    s.align.models = vec![
        m("runtime", "Основа для моделей (ONNX Runtime)", 25_111_930, ModelState::Ready),
        m("separator", "Отделение голоса от музыки", 66_759_214, ModelState::Ready),
        m("mms", "Слова по голосу: другие языки", 317_341_664, ModelState::Ready),
        m("ru", "Слова по голосу: русский", 317_349_889, ModelState::Ready),
        m("ja", "Слова по голосу: японский", 357_414_599, ModelState::Downloading(0.37)),
        m("whisper", "Расслышать текст", 1_087_361_337, ModelState::Missing),
    ];
    s.align.done = 642;
    s.align.total = 1318;
    s.align.on_gpu = Some(true);
}

/// Albums of the fake catalog the library has no release of (by `artist`
/// when there are such, else anyone's).
fn discography(artist: &str, library: &[nikgob_view::ReleaseCard]) -> DiscographyView {
    let all = FakeService::spotify().albums();
    let missing: Vec<_> = all.iter().filter(|a| !library.iter().any(|r| r.title == a.title)).collect();
    let mine: Vec<_> = missing.iter().copied().filter(|a| a.artists.iter().any(|x| x.name == artist)).collect();
    // The fake catalog is small: other artists' albums stand in for a
    // discography the library already has.
    let mut pick = if mine.is_empty() { missing } else { mine };
    // Too few for a grid: the albums the library has too stand in.
    for a in &all {
        if pick.len() >= 6 {
            break;
        }
        if !pick.iter().any(|x| x.id == a.id) {
            pick.push(a);
        }
    }
    let albums = pick
        .into_iter()
        .map(|a| StreamAlbumCard {
            remote: a.id.clone(),
            title: a.title.clone(),
            artist: nikgob_core::streaming::join_names(&a.artists),
            artists: a.artists.as_slice().into(),
            year: a.year,
            release_type: a.release_type,
            cover_url: a.cover_url.clone(),
            total_tracks: a.total_tracks,
            release: None,
            have: Default::default(),
        })
        .collect();
    DiscographyView { loading: false, error: None, albums }
}

/// Queued, downloading (3 of 10 on disk), all on disk, 4 of 12 on disk.
fn album_statuses(albums: &mut [StreamAlbumCard], rows: &mut Vec<WishlistRow>) {
    use nikgob_view::AlbumHave;
    let n = rows.len() as u128;
    for (i, a) in albums.iter_mut().take(4).enumerate() {
        match i {
            0 | 1 => {
                let st = if i == 0 { St::New } else { St::Downloading };
                let mut r = row(1000 + n + i as u128, &a.title, &a.artist, &a.title, st);
                r.scope = WishlistScope::Album;
                rows.push(r);
                if i == 1 {
                    a.have = AlbumHave { local: 3, in_library: 10, total: 10 };
                }
            }
            2 => a.have = AlbumHave { local: 9, in_library: 9, total: 9 },
            _ => a.have = AlbumHave { local: 4, in_library: 4, total: 12 },
        }
    }
}

fn row(n: u128, title: &str, artist: &str, album: &str, status: St) -> WishlistRow {
    WishlistRow {
        id: WishlistId(nikgob_core::model::Uuid::from_u128(n)),
        title: title.into(),
        artist: artist.into(),
        album: Some(album.into()),
        track: None,
        release: None,
        artist_id: None,
        status,
        scope: WishlistScope::Track,
        origin: WishlistOrigin::Like,
        note: match status {
            St::NoTorrent => Some("Не нашёл, где скачать".into()),
            St::Downloading => Some("FLAC · 14 сидов · 38%".into()),
            St::Confirm => Some("Длительность расходится на 40 с".into()),
            _ => None,
        },
        next_try: (status == St::NoTorrent).then(|| "поищу снова через 6 дн.".into()),
        attempts: u32::from(status == St::NoTorrent),
        can_save_from_youtube: status == St::NoTorrent,
        cover: None,
        progress: (status == St::Downloading).then_some(38),
        confirm_files: false,
        missing: 0,
        has_copy: false,
        source: (status == St::Downloading).then(|| "RuTracker".into()),
    }
}

/// A track in «Подтверди» with three offers, the first named at length.
fn confirm() -> nikgob_view::Confirm {
    let option = |label: &str, detail: &str, recommended| nikgob_view::ConfirmOption {
        label: label.into(),
        detail: detail.into(),
        recommended,
    };
    nikgob_view::Confirm {
        id: 1,
        title: "Shake It Now. (feat. Ado) — Nakimushi".into(),
        origin: "Хочу скачать · трек".into(),
        question: "Что из этого — этот трек?".into(),
        options: vec![
            option(
                "[Maid-chan] Nakimushi - Shake It Now. (feat. Ado) / Kimi to Boku 3rd Season ED Single Collection Vol.2 [2024.11.06] (FLAC 24bit/96kHz + Scans + BK + Booklet)",
                "FLAC 24/96 · 450 МБ · 12 сид. · Tokyo Toshokan",
                true,
            ),
            option("Nakimushi - Shake It Now. (2024) [MP3 320K]", "MP3 320 · 52 МБ · 3 сид. · Tokyo Toshokan", false),
            option("[Nipponsei] Nakimushi - Shake It Now. zip", "58 МБ · 2 сид. · Tokyo Toshokan", false),
        ],
        skip_label: "Не то — поищу позже".into(),
        accept_label: "Скачать".into(),
    }
}

/// A wishlist with every status, in two albums and a loose track.
fn wishlist() -> WishlistView {
    let items = vec![
        row(1, "Кукушка", "Кино", "Чёрный альбом", St::Waiting),
        row(2, "Красно-жёлтые дни", "Кино", "Чёрный альбом", St::Waiting),
        row(3, "Нам с тобой", "Кино", "Чёрный альбом", St::Searching),
        row(4, "Automatic", "宇多田ヒカル", "First Love", St::Downloading),
        row(5, "First Love", "宇多田ヒカル", "First Love", St::Confirm),
        row(6, "Movin' on without you", "宇多田ヒカル", "First Love", St::NoTorrent),
        row(7, "Hyperballad", "Björk", "Post", St::Done),
        row(8, "Jóga", "Björk", "Homogenic", St::NoTorrent),
        row(9, "Bachelorette", "Björk", "Homogenic", St::Dismissed),
    ];
    let group = |title: &str, subtitle: &str, ids: &[u128]| WishlistGroup {
        title: title.into(),
        subtitle: subtitle.into(),
        release: None,
        artist: None,
        items: ids.iter().map(|n| WishlistId(nikgob_core::model::Uuid::from_u128(*n))).collect(),
        waiting: ids
            .iter()
            .filter(|n| items.iter().any(|r| r.id.0 == nikgob_core::model::Uuid::from_u128(**n) && r.status == St::Waiting))
            .count() as u32,
    };
    let by_album = vec![
        group("Чёрный альбом", "Кино", &[1, 2, 3]),
        group("First Love", "宇多田ヒカル", &[4, 5, 6]),
        group("Homogenic", "Björk", &[8, 9]),
    ];
    let by_artist = vec![
        group("Кино", "", &[1, 2, 3]),
        group("宇多田ヒカル", "", &[4, 5, 6]),
        group("Björk", "", &[7, 8, 9]),
    ];
    let mut counts: Vec<(St, u32)> = Vec::new();
    for r in &items {
        match counts.iter_mut().find(|(s, _)| *s == r.status) {
            Some((_, n)) => *n += 1,
            None => counts.push((r.status, 1)),
        }
    }
    WishlistView { loaded: true, items, by_album, by_artist, counts, can_search: true }
}

/// The board of Главная: the daylist (when `daylist`) and the first `n`
/// tiles, with covers of the demo library.
fn bento(s: &mut ViewState, n: usize, daylist: bool) {
    use nikgob_view::{MixCard, MixId, TileKind};
    let covers: Vec<nikgob_view::Cover> = s.library.releases.iter().filter_map(|r| r.cover.clone()).collect();
    let pick = |from: usize, k: usize| -> Vec<nikgob_view::Cover> {
        (0..k).filter_map(|i| covers.get((from + i) % covers.len().max(1)).cloned()).collect()
    };
    let card = |id, kicker: &str, title: &str, subtitle: &str, covers| MixCard {
        id,
        kicker: kicker.into(),
        title: title.into(),
        subtitle: subtitle.into(),
        covers,
    };
    s.home.daylist = daylist.then(|| {
        card(MixId::Daylist, "НА ВЕЧЕР", "Тягучий вечер четверга", "что ты слушаешь по вечерам · до 23:00", pick(0, 4))
    });
    let tile = |k: TileKind, title: &str, subtitle: &str, from: usize| card(MixId::Tile(k), "", title, subtitle, pick(from, 1));
    let tiles = [
        tile(TileKind::OnRepeat, "На повторе", "25 треков · что крутил весь месяц", 4),
        tile(TileKind::ArtistOfDay, "Massive Attack", "24 трека · лучшее", 5),
        tile(TileKind::Season, "Лето 2025", "Кино, Земфира и другие", 6),
        tile(TileKind::SingAlong, "Со словами", "Кино, Аквариум и другие", 7),
        tile(TileKind::Soundtrack, "Made in Abyss", "Kevin Penkin и другие", 8),
    ];
    let tiles = tiles.map(|mut t| {
        if let MixId::Tile(k) = t.id {
            t.kicker = nikgob_recs::TileKind::ALL[k as usize].kicker().into();
        }
        t
    });
    s.home.tiles = tiles.into_iter().take(n).collect();
}

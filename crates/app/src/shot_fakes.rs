//! `NIKGOB_SHOT_STEPS` `fake:<what>`: states the demo cannot reach by
//! itself, laid over the runtime's view state for screenshots only (the
//! runtime never sees them).
//!
//! - `fake:discography` — the open library artist gets a «Нет в библиотеке»
//!   section from the fake Spotify catalog.
//! - `fake:wishlist` — a wishlist with every status.
//! - `fake:history-refusals` — «Загрузки → История» with wished downloads
//!   the sources turned away, one line each, as the runtime words them.
//! - `fake:confirm` — a «Хочу скачать» track in «Подтверди» whose offers
//!   have long release names.
//! - `fake:album-status` — the first stream album cards on screen (the
//!   discography, a service's artist page, search) are queued, downloading,
//!   fully local and partly local, in that order.
//! - `fake:buffering` — the player waits for a stream.
//! - `fake:audio:youtube` / `fake:audio:spotify` — where the sound comes from;
//!   `fake:audio:soloist` — a Spotify track Spotify Soloist plays.
//! - `fake:link` — the open playlist follows a Spotify playlist.
//! - `fake:lyrics` — word-synced lyrics (LyricsPlus) for whatever plays:
//!   the demo has no network to fetch any for a stream track;
//!   `fake:lyrics:dense` — the same 40 lines a line every 0.55 s, so all
//!   of them lie within the demo's 24 s tones (a click on any line seeks
//!   inside the file).
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
//! - `fake:lyrics-home:words|online|saved|stream|stream-saved|clip` —
//!   with `fake:lyrics-texts`, the shown text: Genius' with word timing
//!   the player made (to make again or save), LRCLIB's lines only online,
//!   a text saved on the computer, the made text of a song without a file
//!   (kept as «Что сохранять» says) or the one it kept, or a clip whose
//!   song is unclear (timing cannot be made).
//! - `fake:lyrics-state:<state>[:word|line|plain]` — the lyrics control in
//!   each of its states (`none`, `searching`, `plain`,
//!   `line`, `word`, `made`, `saved`, `making`, `failed`, `stream`, `clip`,
//!   `no-audio`), seen by words, by lines or without timing when the last
//!   part says so (`made` has LRCLIB's lines beside it, so the source rows
//!   say how each is timed). A later one replaces an earlier one (one run, many
//!   `snap:`s).
//! - `fake:align-models` — Settings → «Текст песен»: models ready, one
//!   downloading, one missing; the background pass halfway; on the GPU.
//! - `fake:lyrics-gap` — LRC lines with an empty stamp making a wait
//!   (8.26–16.57 s) before a line that wraps, and a hyphenated
//!   «do-dodge»; `fake:lyrics-gap:words` — the same with word timing.
//! - `fake:share` — «Загрузки → Раздача» busy (`route:seeding`): three
//!   torrents seeding, Soulseek sharing with uploads running, queued, done
//!   and cut off, and today's and overall totals.
//! - `fake:fake-flac` — «Подтверди» with a card of three fake FLACs of one
//!   album offered as MP3s, a row per song to tick (`route:confirm`), as
//!   the runtime words it (runtime/src/quality.rs).
//! - `fake:upgrade-offer` — a better copy of a library track found and
//!   waiting in «Подтверди» («FLAC вместо MP3 320»), and «Хочу скачать»
//!   with better copies looked for in each state (`route:confirm`,
//!   `route:wishlist`).
//! - `fake:downloads` — downloads of several sources running at once:
//!   «Активные» with a torrent, Soulseek (one coming, one in a peer's
//!   queue), Bandcamp, Internet Archive, a save from YouTube and an
//!   import; «Хочу скачать» with their items, the same numbers; «Подтверди»
//!   with a card and one without choices (never shown).
//! - `fake:quality:real` / `fake:quality:fake` — what plays is a FLAC whose
//!   check found it genuine, or made from an MP3 ~128 (its rows and the
//!   «Качество» section of «Инфо» say so, with the spectrum and its cut).

use nikgob_core::streaming::{FakeService, PlaylistSyncMode, RemoteId, Service};
use nikgob_view::{
    DiscographyView, PlaylistSyncView, Status, StreamAlbumCard, Transfer, TransferPhase, ViewState, WishlistGroup,
    WishlistId, WishlistOrigin, WishlistRow, WishlistScope, WishlistStatus as St, WishlistView,
};

#[derive(Clone, Debug, Default)]
pub struct Fakes {
    discography: bool,
    wishlist: bool,
    confirm: bool,
    album_status: bool,
    buffering: bool,
    audio: Option<Service>,
    soloist: bool,
    link: bool,
    lyrics: bool,
    /// `fake:lyrics:dense`.
    lyrics_dense: bool,
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
    /// `fake:lyrics-gap`: with word timing.
    lyrics_gap: Option<bool>,
    lyrics_state: Option<String>,
    history_refusals: bool,
    share: bool,
    upgrade_offer: bool,
    fake_flac: bool,
    /// `fake:quality:fake` (true) / `fake:quality:real`.
    quality: Option<bool>,
    downloads: bool,
}

impl Fakes {
    /// Lays nothing over the view any more.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

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
            "audio:soloist" => {
                self.audio = Some(Service::Spotify);
                self.soloist = true;
            }
            "link" => self.link = true,
            "lyrics" => self.lyrics = true,
            "lyrics:dense" => {
                self.lyrics = true;
                self.lyrics_dense = true;
            }
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
            "lyrics-gap" => self.lyrics_gap = Some(false),
            "lyrics-gap:words" => self.lyrics_gap = Some(true),
            "history-refusals" => self.history_refusals = true,
            "share" => self.share = true,
            "upgrade-offer" => self.upgrade_offer = true,
            "fake-flac" => self.fake_flac = true,
            "downloads" => self.downloads = true,
            "quality:real" => self.quality = Some(false),
            "quality:fake" => self.quality = Some(true),
            c if c.starts_with("craft:") => self.craft = Some(c["craft:".len()..].to_owned()),
            c if c.starts_with("lyrics-home:") => self.lyrics_home = Some(c["lyrics-home:".len()..].to_owned()),
            c if c.starts_with("chip:") => self.chip = Some(c["chip:".len()..].to_owned()),
            c if c.starts_with("lyrics-state:") => self.lyrics_state = Some(c["lyrics-state:".len()..].to_owned()),
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
            || self.lyrics_gap.is_some()
            || self.lyrics_state.is_some()
            || self.history_refusals
            || self.share
            || self.upgrade_offer
            || self.fake_flac
            || self.quality.is_some()
            || self.downloads
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
        if self.history_refusals {
            s.downloads.history = refusals_history();
        }
        if self.share {
            share(&mut s);
        }
        if self.upgrade_offer {
            s.downloads.confirm = vec![upgrade_offer()];
            s.downloads.wishlist = upgrades();
        }
        if self.fake_flac {
            s.downloads.confirm.retain(|c| c.pick.is_none());
            s.downloads.confirm.push(fake_flac_card());
        }
        if self.downloads {
            running_downloads(&mut s);
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
                Service::Spotify | Service::Deezer | Service::Soundcloud => Service::Youtube,
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
            s.home = nikgob_view::HomeView { greeting_hour: s.home.greeting_hour, library_empty: true, ..Default::default() };
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
                Service::Soundcloud => nikgob_core::streaming::quality_label("AAC", 160, false),
            });
            if self.soloist {
                s.player.soloist = true;
                s.player.audio_quality = Some("Soloist".into());
            }
        }
        if self.lyrics {
            s.lyrics = word_lyrics(state.player.current.as_ref().map(|t| t.id), self.lyrics_dense);
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
        if let Some(words) = self.lyrics_gap {
            s.lyrics = gap_lyrics(state.player.current.as_ref().map(|t| t.id), words);
        }
        if let Some(name) = self.lyrics_state.as_deref() {
            lyrics_state(&mut s, name);
        }
        if let Some(fake) = self.quality {
            sound_quality(&mut s, fake);
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

/// What plays is a FLAC, genuine or made from an MP3 ~128
/// (`fake:quality:*`): its rows, the «Инфо» line and «Качество» section,
/// as the runtime fills them from a stored check.
fn sound_quality(s: &mut ViewState, fake: bool) {
    use nikgob_core::model::Codec;
    use nikgob_core::quality::{QualityCheck, SPECTRUM_POINTS, SoundOrigin, Verdict, cutoff_text, label, verdict_text};
    let (verdict, cutoff) = if fake { (Verdict::Transcoded, 16_020.0) } else { (Verdict::Genuine, 22_050.0) };
    let nyquist = 22_050.0_f32;
    // A music-like slope; a lossy encoder's lowpass is a cliff.
    let spectrum_db: Vec<f32> = (0..SPECTRUM_POINTS)
        .map(|i| {
            let hz = nyquist * (i as f32 + 0.5) / SPECTRUM_POINTS as f32;
            let body = -32.0 - 2.6 * (hz / 1000.0) - 6.0 * (hz / 400.0 + 1.0).log10() + 1.5 * (i as f32 * 0.7).sin();
            if fake && hz > cutoff { body - 58.0 - (hz - cutoff) / 400.0 } else { body }
        })
        .collect();
    let check = QualityCheck {
        verdict,
        cutoff_hz: cutoff,
        steepness_db: if fake { 61.4 } else { 0.0 },
        origin: SoundOrigin::estimate(verdict, cutoff),
        sample_rate: 44_100,
        spectrum_db: spectrum_db.clone(),
        made_from: None,
        file_size: 26_733_824,
        file_mtime: 0,
        checked_at: 0,
    };
    let row = label(&Codec::Flac, None, Some(&check.summary()));
    for t in [s.player.current.as_mut(), s.queue.current.as_mut()].into_iter().flatten() {
        t.quality = Some(row.clone());
        t.has_local = true;
        t.source = Some(nikgob_view::SourceKind::LocalFile);
    }
    s.player.audio_service = None;
    s.player.audio_quality = None;
    s.info.source = Some("Локальный файл".into());
    s.info.quality = Some(format!("{row} · 16/44.1"));
    s.info.path = Some(r"C:\Music\Пошлая Молли\2017 - 8 способов как бросить\07 - Молли.flac".into());
    let facts = [
        ("Кодек", "FLAC"),
        ("Контейнер", "FLAC"),
        ("Битрейт", "1058 кбит/с"),
        ("Частота", "44,1 кГц"),
        ("Разрядность", "16 бит"),
        ("Каналы", "стерео"),
        ("Длительность", "2:32"),
        ("Размер", "25,5 МБ"),
        ("Откуда", "Скачан или добавлен файлом"),
    ];
    s.info.sound = Some(nikgob_view::SoundDetails {
        facts: facts.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect(),
        verdict: Some(verdict_text(&Codec::Flac, Some(1058), &check)),
        warning: fake,
        band: cutoff_text(&check),
        spectrum: Some(nikgob_view::SpectrumPlot {
            db: spectrum_db,
            nyquist_hz: nyquist,
            cutoff_hz: fake.then_some(cutoff),
        }),
        pending: false,
        to_mp3: fake,
        better: true,
    });
}

/// «Загрузки → История» of wished downloads the sources turned away
/// (`fake:history-refusals`): one line per wish with its latest state, in
/// the runtime's words (streams/wishlist.rs `why_words`, `later_line`),
/// among imports' lines.
fn refusals_history() -> Vec<nikgob_view::Job> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
    let lines: [(&str, &str, Option<&str>); 7] = [
        ("Пошлая Молли — 8 способов как бросить...", "Tidal сейчас не отдаёт этот трек · в 21:48 спрошу Tidal, Soulseek", None),
        (
            "MORGENSHTERN — ЛЕГЕНДАРНАЯ ПЫЛЬ",
            "Есть 1 из 10 · Qobuz просит подождать до 03:00, Tidal сейчас не отдаёт остальное · в 22:15 спрошу Tidal",
            None,
        ),
        ("frutiger pm — cloud city", "Tidal сейчас не отдаёт этот трек · дальше — Soulseek", None),
        (
            "Con Fetti Pesto, David Münnich, Alexx Grimm — Meine Freundin ist kaputt",
            "Tidal сейчас не отдаёт этот трек · есть варианты под вопросом — выбери в «Хочу скачать»",
            None,
        ),
        ("Soulseek: Я не интроверт", "добавлено 1", None),
        (
            "Комсомольск — Мозги & деньги",
            "",
            Some("Tidal сейчас не отдаёт этот трек, сервер Qobuz недоступен · поищу снова через неделю"),
        ),
        ("Импорт «Downloads»", "добавлено 12 · дублей 2", None),
    ];
    lines
        .iter()
        .enumerate()
        .map(|(i, (title, subtitle, error))| nikgob_view::Job {
            id: (1 << 48) | (1 << 40) | (900 - i as u64),
            title: (*title).into(),
            subtitle: (*subtitle).into(),
            progress: Some(1.0),
            rate: None,
            eta: None,
            paused: false,
            queued: false,
            controllable: false,
            pausable: false,
            error: error.map(str::to_owned),
            at: now - i as i64 * 7 * 60_000,
        })
        .collect()
}

/// Lines of three words, one word a second, a line every four seconds.
fn word_lyrics(track: Option<nikgob_view::TrackId>, dense: bool) -> nikgob_view::LyricsView {
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
    // A line every 4 s, or every 0.55 s (`dense`).
    let (every, first) = if dense { (550, 500) } else { (4000, 1500) };
    let lines = (0..40)
        .map(|i| {
            let line = text[i % text.len()];
            let start = first + every * i as u64;
            let words: Vec<&str> = line.split(' ').collect();
            let step = every * 4 / 5 / words.len() as u64;
            LyricLine {
                start: ms(start),
                end: Some(ms(start + every * 17 / 20)),
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

/// LRC lines with an empty stamp before a line that wraps (`fake:lyrics-gap`),
/// with words timed by their letters inside each line when `words`.
fn gap_lyrics(track: Option<nikgob_core::model::TrackId>, words: bool) -> nikgob_view::LyricsView {
    use nikgob_core::lyrics::{LyricLine, LyricWord, LyricsSource};
    use std::time::Duration;
    let stamps = [
        (5.88, "KOMA"),
        (8.26, ""),
        (16.57, "Дырявые кеды — моё отражение в луже у самого дома"),
        (21.4, "Я делаю do-dodge back — уклоняюсь от атаки"),
        (25.9, "Кто-то смотрит сверху, но мне всё равно"),
        (30.2, "Дырявые кеды — моё отражение"),
        (34.0, ""),
    ];
    let lines = stamps
        .iter()
        .enumerate()
        .map(|(i, (start, text))| {
            let mut l = LyricLine::new(Duration::from_secs_f32(*start), *text);
            if words && !text.is_empty() {
                let end = stamps.get(i + 1).map_or(start + 4.0, |n| n.0 - 0.4);
                let letters = |w: &str| w.chars().filter(|c| !c.is_whitespace()).count() as f32;
                let total = letters(text).max(1.0);
                let mut at = 0.0;
                for w in text.split_inclusive(' ') {
                    let (a, b) = (start + (end - start) * at / total, start + (end - start) * (at + letters(w)) / total);
                    at += letters(w);
                    l.words.push(LyricWord { start: Duration::from_secs_f32(a), end: Duration::from_secs_f32(b), text: w.to_owned() });
                }
                l.end = Some(Duration::from_secs_f32(end));
            }
            l
        })
        .collect();
    nikgob_view::LyricsView {
        track,
        lines,
        source: Some(if words { LyricsSource::LyricsPlus } else { LyricsSource::Lrclib }),
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
    let own = word_lyrics(track, false);
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
            l.lines = word_lyrics(l.track, false).lines;
            if let Some(o) = l.sources.iter_mut().find(|o| o.source == LyricsSource::Genius) {
                o.state = nikgob_view::LyricsAvail::Ready(nikgob_view::LyricsSync::Word);
            }
        }
        "online" => {
            l.source = Some(LyricsSource::Lrclib);
            l.lines = l.texts[0].lines.clone();
            l.made = false;
            l.home = LyricsHome::Online;
            // A found text can be saved too.
            l.savable = true;
        }
        "saved" | "stream-saved" => {
            l.stream = name == "stream-saved";
            l.source = Some(LyricsSource::File);
            l.lines = word_lyrics(l.track, false).lines;
            l.made = false;
            l.home = LyricsHome::Computer;
            l.savable = false;
        }
        "stream" => l.stream = true,
        "clip" => {
            // Its text can still be kept in the artist's folder.
            l.craft = LyricsCraft::No("Подсветку не сделать: в клипе неясно, какая песня".into());
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
            let words = word_lyrics(Some(track), false).lines;
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
    let words = word_lyrics(track, false).lines;
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

/// The lyrics control in one of its states, as the runtime would show it (`fake:lyrics-state:<state>[:<seen>]`).
fn lyrics_state(s: &mut ViewState, arg: &str) {
    use nikgob_core::lyrics::{LyricLine, LyricsSource as Src};
    use nikgob_view::{AlignJob, AlignStage, Craft, LyricsAvail, LyricsCraft, LyricsHome, LyricsOption, LyricsSync, LyricsText};
    let (name, seen) = arg.split_once(':').map_or((arg, None), |(n, v)| (n, Some(v)));
    let track = s.player.current.as_ref().map(|t| t.id);
    let words = word_lyrics(track, false).lines;
    let lines: Vec<LyricLine> = words.iter().map(|l| LyricLine { words: Vec::new(), ..l.clone() }).collect();
    let plain = lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n");
    let opt = |source, state| LyricsOption { source, state, fits: None, made: false };
    let ready = |source, sync| LyricsOption { fits: Some(true), ..opt(source, LyricsAvail::Ready(sync)) };
    let missing = |source| opt(source, LyricsAvail::Missing);
    let mut l = nikgob_view::LyricsView { track, ..Default::default() };
    // The shown text: its source, its own timing, the player's timing of
    // it, where it lives, whether timing can be made.
    let text = |l: &mut nikgob_view::LyricsView, source, own: LyricsSync, made: Option<&Vec<LyricLine>>, home, craft: LyricsCraft| {
        let own_lines = match own {
            LyricsSync::Plain => Vec::new(),
            LyricsSync::Line => lines.clone(),
            _ => words.clone(),
        };
        l.source = Some(source);
        l.home = home;
        l.craft = craft;
        l.made = made.is_some();
        // Any text not on the computer yet can be saved there.
        l.savable = home != LyricsHome::Computer;
        l.lines = made.cloned().unwrap_or(own_lines.clone());
        let finest = LyricsSync::of(&l.lines).unwrap_or(LyricsSync::Plain).max(own);
        l.shows = [LyricsSync::Word, LyricsSync::Line, LyricsSync::Plain].into_iter().filter(|s| *s <= finest).collect();
        l.texts = vec![LyricsText {
            source: Some(source),
            lines: own_lines,
            plain: (own == LyricsSync::Plain).then(|| plain.clone()),
            sync: Some(own),
            fits: Some(true),
            made: made.cloned(),
            saved: made.is_some(),
        }];
        if let Some(o) = l.sources.iter_mut().find(|o| o.source == source) {
            o.made = made.is_some();
        }
    };
    let file = LyricsCraft::Ready;
    match name {
        "none" => l.sources = vec![missing(Src::LyricsPlus), missing(Src::LrcRed), missing(Src::Lrclib), missing(Src::Genius)],
        "searching" => {
            l.loading = true;
            l.sources = vec![opt(Src::LyricsPlus, LyricsAvail::Loading), opt(Src::LrcRed, LyricsAvail::Loading), missing(Src::Lrclib)];
        }
        "line" => {
            l.sources = vec![missing(Src::LyricsPlus), ready(Src::Lrclib, LyricsSync::Line), ready(Src::Genius, LyricsSync::Plain)];
            text(&mut l, Src::Lrclib, LyricsSync::Line, None, LyricsHome::Online, file);
        }
        "word" => {
            l.sources = vec![ready(Src::LyricsPlus, LyricsSync::Word), ready(Src::Lrclib, LyricsSync::Line)];
            text(&mut l, Src::LyricsPlus, LyricsSync::Word, None, LyricsHome::Online, file);
        }
        "made" => {
            // LRCLIB's lines beside it: the rows say how each is timed.
            l.sources = vec![missing(Src::LyricsPlus), ready(Src::Lrclib, LyricsSync::Line), ready(Src::Genius, LyricsSync::Plain)];
            text(&mut l, Src::Genius, LyricsSync::Plain, Some(&words), LyricsHome::Library, file);
            l.picked = true;
        }
        "saved" => {
            l.sources = vec![ready(Src::File, LyricsSync::Word), missing(Src::LyricsPlus), ready(Src::Genius, LyricsSync::Plain)];
            text(&mut l, Src::File, LyricsSync::Word, None, LyricsHome::Computer, file);
        }
        "clip" => {
            l.sources = vec![missing(Src::LyricsPlus), missing(Src::Lrclib), ready(Src::Genius, LyricsSync::Plain)];
            let why = LyricsCraft::No("Подсветку не сделать: это клип, неясно, какая в нём песня".into());
            text(&mut l, Src::Genius, LyricsSync::Plain, None, LyricsHome::Online, why);
        }
        "no-audio" => {
            l.sources = vec![missing(Src::LyricsPlus), missing(Src::Lrclib), ready(Src::Genius, LyricsSync::Plain)];
            text(&mut l, Src::Genius, LyricsSync::Plain, None, LyricsHome::Online, LyricsCraft::Off);
        }
        // plain, making, failed, stream: Genius' plain text, timing can be
        // made (from the file, or the stream's audio for a while).
        _ => {
            l.sources = vec![missing(Src::LyricsPlus), missing(Src::Lrclib), ready(Src::Genius, LyricsSync::Plain)];
            text(&mut l, Src::Genius, LyricsSync::Plain, None, LyricsHome::Online, file);
        }
    }
    // Seen otherwise than it comes.
    match seen {
        Some("plain") => l.lines.clear(),
        Some("line") => l.lines = lines.clone(),
        Some("word") if l.shows.contains(&LyricsSync::Word) => l.lines = words.clone(),
        _ => {}
    }
    if l.lines.is_empty() && l.source.is_some() {
        l.plain = Some(plain.clone());
    }
    if let Some(t) = track {
        let craft = |failed: Option<&str>| Craft { track: t, source: Src::Genius, target: LyricsSync::Word, lines: None, failed: failed.map(str::to_owned) };
        match name {
            "making" => {
                s.align.craft = Some(craft(None));
                s.align.job = Some(AlignJob { track, stage: AlignStage::Listen, progress: 0.6, asked: true, bytes: None });
            }
            "failed" => s.align.craft = Some(craft(Some("Не удалось скачать модели"))),
            _ => {}
        }
    }
    s.lyrics = l;
    if name == "stream" || name == "clip" || name == "no-audio" {
        s.info.path = None;
    } else if s.info.path.is_none() {
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
/// «Раздача» with both kinds of sharing busy.
fn share(s: &mut ViewState) {
    use nikgob_view::{SeedRow, SharingView, SlskSharingView, UploadRow, UploadRowState as U};
    const MB: u64 = 1024 * 1024;
    let seed = |id: u64, title: &str, size: u64, uploaded: u64, up_bps: u64, peers: u32, days: u32| SeedRow {
        id,
        title: title.into(),
        uploaded_bytes: uploaded,
        downloaded_bytes: size,
        expires: Some(format!("чистка через {days} дн.")),
        up_bps,
        down_bps: 0,
        peers,
        size_bytes: size,
    };
    s.prefs.no_seed = false;
    s.downloads.seeding = vec![
        seed(1, "Aimer - Daydream (2016) [FLAC]", 612 * MB, 418 * MB, 240 * 1024, 4, 12),
        seed(2, "ZUTOMAYO - Gusare (2021) [FLAC 24-96]", 1480 * MB, 2210 * MB, 0, 1, 27),
        seed(3, "Radiohead - In Rainbows (2007) [FLAC]", 296 * MB, 37 * MB, 56 * 1024, 2, 3),
    ];
    s.downloads.seeded_bytes = s.downloads.seeding.iter().map(|r| r.uploaded_bytes).sum();
    let up = |id: u64, user: &str, file: &str, size: u64, sent: u64, bps: u64, state: U| UploadRow {
        id,
        user: user.into(),
        file: file.into(),
        size,
        sent,
        bytes_per_sec: bps,
        state,
        ended_at: 0,
    };
    let uploads = vec![
        up(9, "nightowl", "03 Brave Shine.flac", 38 * MB, 21 * MB, 310 * 1024, U::Sending),
        up(10, "mira_k", "05 Kansaete Kuyashiiwa.flac", 44 * MB, 6 * MB, 120 * 1024, U::Sending),
        up(11, "nightowl", "04 Insane Dream.flac", 41 * MB, 0, 0, U::Queued),
        up(8, "deepcuts", "Reckoner.flac", 33 * MB, 33 * MB, 280 * 1024, U::Done),
        up(7, "deepcuts", "cover.jpg", MB / 2, MB / 2, 400 * 1024, U::Done),
        up(6, "lofi_fan", "Nude.flac", 29 * MB, 11 * MB, 90 * 1024, U::Failed),
    ];
    s.downloads.sharing = SharingView {
        torrent_up_bps: 296 * 1024,
        torrent_down_bps: 1200 * 1024,
        slsk_up_bps: 430 * 1024,
        slsk_down_bps: 0,
        torrent_today: 860 * MB,
        slsk_today: 212 * MB,
        torrent_total: 2665 * MB + 41 * 1024 * MB,
        slsk_total: 6300 * MB,
        seeding_disk_bytes: (612 + 1480 + 296) * MB,
        torrent_peers: 7,
        slsk: SlskSharingView {
            configured: true,
            running: true,
            remote: false,
            share: true,
            files: 4812,
            folders: 391,
            upload_limit_kbps: 1024,
            upload_slots: 2,
            uploads,
        },
    };
}

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
                    a.have = AlbumHave { local: 3, in_library: 10, total: 10, mine: None };
                }
            }
            2 => a.have = AlbumHave { local: 9, in_library: 9, total: 9, mine: None },
            _ => a.have = AlbumHave { local: 4, in_library: 4, total: 12, mine: Some(4) },
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
        transfer: (status == St::Downloading).then_some(Transfer {
            phase: TransferPhase::Going,
            done: 38,
            total: Some(100),
            bps: 0,
        }),
        confirm_files: false,
        missing: 0,
        has_copy: false,
        source: (status == St::Downloading).then(|| "RuTracker".into()),
        upgrade: None,
    }
}

/// A better copy of a library track in «Подтверди» (`fake:upgrade-offer`),
/// as the runtime words it (streams/wishlist/upgrade.rs).
fn upgrade_offer() -> nikgob_view::Confirm {
    let option = |label: &str, detail: &str, recommended| nikgob_view::ConfirmOption {
        label: label.into(),
        detail: detail.into(),
        recommended,
    };
    nikgob_view::Confirm {
        yes_no: false,
        id: (1 << 47) | 1,
        title: "8 способов как бросить... — Пошлая Молли".into(),
        origin: "Качество лучше · трек".into(),
        question: "Заменить файл трека на это?".into(),
        options: vec![
            option(
                "Пошлая Молли - 8 способов как бросить... (2017) [FLAC]",
                "FLAC вместо MP3 320 · 31 МБ · 9 сид. · RuTracker",
                true,
            ),
            option("Пошлая Молли — 8 способов как бросить...", "FLAC 24/48 вместо MP3 320 · Qobuz", false),
        ],
        skip_label: "Оставить как есть".into(),
        accept_label: "Заменить".into(),
        pick: None,
    }
}

/// Three fake FLACs of one album offered as MP3s (`fake:fake-flac`), a row
/// per song, as the runtime words them (runtime/src/quality.rs).
fn fake_flac_card() -> nikgob_view::Confirm {
    let song = |title: &str, detail: &str| nikgob_view::ConfirmOption {
        label: title.into(),
        detail: detail.into(),
        recommended: false,
    };
    let ids = (1..=3u128).map(|n| nikgob_core::model::SourceId(nikgob_core::model::Uuid::from_u128(0xfa1e_0000 + n))).collect();
    nikgob_view::Confirm {
        yes_no: false,
        // The runtime's bit of the fakes cards (`quality::QUALITY_ID`).
        id: (1 << 45) | 1,
        title: "8 способов как бросить...".into(),
        origin: "Ненастоящий FLAC · 3 файла".into(),
        question: "Отметь, какие превратить в MP3: FLAC уйдут в Корзину, «Вернуть FLAC» в настройках вернёт их".into(),
        options: vec![
            song("Молли", "Вероятно, из MP3 ~128 · срез 16,1 кГц"),
            song("Почему ты ещё не фанат?", "Вероятно, из MP3 ~192 · срез 18,2 кГц"),
            song("Вайбмен", "Вероятно, из MP3 ~256 · срез 19,6 кГц"),
        ],
        skip_label: "Оставить FLAC".into(),
        accept_label: "Превратить в MP3".into(),
        pick: Some(nikgob_view::ConfirmPick {
            sources: ids,
            accept: ("Превратить".into(), "в MP3".into()),
            skip: ("Оставить".into(), "FLAC".into()),
        }),
    }
}

/// «Хочу скачать» with better copies looked for, one in each state
/// (`fake:upgrade-offer`).
fn upgrades() -> WishlistView {
    let up = |n: u128, title: &str, artist: &str, album: &str, status: St, scope: WishlistScope, words: &str| {
        let mut r = row(n, title, artist, album, status);
        r.origin = WishlistOrigin::Upgrade;
        r.scope = scope;
        r.can_save_from_youtube = false;
        r.next_try = None;
        r.upgrade = (!words.is_empty()).then(|| words.to_owned());
        r
    };
    let items = vec![
        up(21, "8 способов как бросить...", "Пошлая Молли", "8 способов как бросить...", St::Confirm, WishlistScope::Track, "MP3 320 · нашёл лучше — подтверди замену"),
        up(22, "ЛЕГЕНДАРНАЯ ПЫЛЬ", "MORGENSHTERN", "ЛЕГЕНДАРНАЯ ПЫЛЬ", St::Searching, WishlistScope::Album, "MP3 128 у 9 из 10 · ищу lossless"),
        up(23, "Cadillac", "MORGENSHTERN", "MILLION DOLLAR: HAPPINESS", St::Downloading, WishlistScope::Track, ""),
        up(24, "Розовое вино", "Элджей, Feduk", "Sayonara детка", St::NoTorrent, WishlistScope::Track, "OPUS 160 · лучше пока нет, поищу снова через 6 дн."),
        up(25, "Hyperballad", "Björk", "Post", St::Done, WishlistScope::Track, "было MP3 128 — теперь FLAC"),
    ];
    let mut counts: Vec<(St, u32)> = Vec::new();
    for r in &items {
        match counts.iter_mut().find(|(s, _)| *s == r.status) {
            Some((_, n)) => *n += 1,
            None => counts.push((r.status, 1)),
        }
    }
    WishlistView { loaded: true, items, by_album: Vec::new(), by_artist: Vec::new(), counts, can_search: true }
}

/// Downloads of several sources at once (`fake:downloads`), as the runtime
/// makes them (runtime/src/running.rs, torrents.rs): each row of
/// «Активные» and its item's line in «Хочу скачать» from one [`Transfer`].
fn running_downloads(s: &mut ViewState) {
    const MB: u64 = 1 << 20;
    const KB: u64 = 1 << 10;
    let going = |done: u64, total: Option<u64>, bps: u64| Transfer { phase: TransferPhase::Going, done, total, bps };
    let direct = |n: u64| (1 << 48) | (1 << 44) | n;
    // (row id, title, source, from, transfer, pausable; its item: title, artist, album, from)
    let runs: [(u64, &str, Option<&str>, Option<&str>, Transfer, bool, (&str, &str, &str, &str)); 6] = [
        (
            1,
            "Creepy Nuts - LEGION (BBE VIVA) [FLAC 48kHz / 24bit]",
            None,
            None,
            going(39 * MB, Some(42 * MB), 62 * KB),
            true,
            ("オトノケ - Otonoke", "Creepy Nuts", "LEGION", "Knaben"),
        ),
        (
            direct(1),
            "Jagwar Twin — Happy Face",
            Some("Soulseek"),
            Some("mugilaw"),
            going(12 * MB + 300 * KB, Some(33 * MB), 410 * KB),
            false,
            ("Happy Face", "Jagwar Twin", "Happy Face", "mugilaw"),
        ),
        (
            direct(2),
            "Panic! At The Disco — House of Memories",
            Some("Soulseek"),
            Some("rotted"),
            Transfer { phase: TransferPhase::Queued(Some(12)), done: 0, total: Some(31 * MB), bps: 0 },
            false,
            ("House of Memories", "Panic! At The Disco", "Death of a Bachelor", "rotted"),
        ),
        (
            direct(3),
            "Hiatus Kaiyote — Mood Variant",
            None,
            Some("Bandcamp"),
            going(212 * MB, Some(388 * MB), 2 * MB + 300 * KB),
            false,
            ("Mood Variant", "Hiatus Kaiyote", "Mood Variant", "Bandcamp"),
        ),
        (
            direct(4),
            "Yellow Magic Orchestra — Rydeen",
            None,
            Some("Internet Archive"),
            going(5 * MB + 600 * KB, None, 180 * KB),
            false,
            ("Rydeen", "Yellow Magic Orchestra", "Solid State Survivor", "Internet Archive"),
        ),
        (
            direct(5),
            "Gorillaz — Rhinestone Eyes",
            None,
            Some("YouTube"),
            Transfer::default(),
            false,
            ("Rhinestone Eyes", "Gorillaz", "Plastic Beach", "YouTube"),
        ),
    ];
    let mut active: Vec<nikgob_view::Job> = runs
        .iter()
        .map(|(id, title, via, from, t, pausable, _)| t.job(*id, (*title).into(), *via, *from, true, *pausable))
        .collect();
    // The files of one came: the import takes them, under the item's name.
    active.push(nikgob_view::Job {
        id: (1 << 48) | 5,
        title: "Björk — Jóga".into(),
        subtitle: "сверяю с библиотекой · Björk - Homogenic - 05 - Jóga.flac".into(),
        progress: None,
        rate: None,
        eta: None,
        paused: false,
        queued: false,
        controllable: true,
        pausable: false,
        error: None,
        at: 0,
    });
    s.downloads.active = active;

    let mut items: Vec<WishlistRow> = runs
        .iter()
        .enumerate()
        .map(|(i, (.., t, _, (title, artist, album, from)))| {
            let mut r = row(60 + i as u128, title, artist, album, St::Downloading);
            r.transfer = Some(*t);
            r.source = Some((*from).into());
            r.note = None;
            if i == 3 {
                r.scope = WishlistScope::Album;
            }
            r
        })
        .collect();
    let mut joga = row(70, "Jóga", "Björk", "Homogenic", St::Downloading);
    joga.transfer = Some(Transfer { phase: TransferPhase::Importing, ..Transfer::default() });
    joga.source = Some("Soulseek".into());
    items.push(joga);
    items.push(row(71, "Нам с тобой", "Кино", "Чёрный альбом", St::Searching));
    items.push(row(72, "Кукушка", "Кино", "Чёрный альбом", St::Waiting));
    items.push(row(73, "Hyperballad", "Björk", "Post", St::Done));
    let group = |title: &str, subtitle: &str, ids: &[u128]| WishlistGroup {
        title: title.into(),
        subtitle: subtitle.into(),
        release: None,
        artist: None,
        items: ids.iter().map(|n| WishlistId(nikgob_core::model::Uuid::from_u128(*n))).collect(),
        waiting: u32::from(ids.contains(&72)),
    };
    let by_album = vec![
        group("LEGION", "Creepy Nuts", &[60]),
        group("Happy Face", "Jagwar Twin", &[61]),
        group("Death of a Bachelor", "Panic! At The Disco", &[62]),
        group("Mood Variant", "Hiatus Kaiyote", &[63]),
        group("Solid State Survivor", "Yellow Magic Orchestra", &[64]),
        group("Plastic Beach", "Gorillaz", &[65]),
        group("Homogenic", "Björk", &[70]),
        group("Чёрный альбом", "Кино", &[71, 72]),
    ];
    let mut counts: Vec<(St, u32)> = Vec::new();
    for r in &items {
        match counts.iter_mut().find(|(s, _)| *s == r.status) {
            Some((_, n)) => *n += 1,
            None => counts.push((r.status, 1)),
        }
    }
    s.downloads.wishlist = WishlistView { loaded: true, items, by_album, by_artist: Vec::new(), counts, can_search: true };

    // A card whose offers could not be read: the runtime never sends it any
    // more, and the page would not show it.
    let empty = nikgob_view::Confirm {
        id: 2,
        title: "Rhinestone Eyes — Gorillaz".into(),
        options: Vec::new(),
        ..confirm()
    };
    s.downloads.confirm = vec![confirm(), empty];
}

/// A track in «Подтверди» with three offers, the first named at length.
fn confirm() -> nikgob_view::Confirm {
    let option = |label: &str, detail: &str, recommended| nikgob_view::ConfirmOption {
        label: label.into(),
        detail: detail.into(),
        recommended,
    };
    nikgob_view::Confirm {
        yes_no: false,
        id: 1,
        title: "Shake It Now. (feat. Ado) — Nakimushi".into(),
        origin: String::new(),
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
        pick: None,
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
        card(MixId::Daylist, "НА ВЕЧЕР", "Тягучий вечер четверга", "25 треков", pick(0, 4))
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

//! Scripted-screenshot and automation hooks on [`Md3eUi`].

use super::*;

impl Md3eUi {
    /// Opens the first run at a step (`sources`, `keep`, `connect`,
    /// `lyrics`, `more`) with `picks` («spotify+folder») chosen
    /// (screenshots). False for a step it does not know.
    pub fn first_run_at(&mut self, state: &ViewState, step: &str, picks: &str, out: &mut Vec<Command>) -> bool {
        let Some(step) = pages::first_run::Step::named(step) else { return false };
        pages::first_run::jump(state, &mut self.pages, step, picks, out);
        true
    }

    /// Chooses a world on the card at the top of Главная (screenshots).
    pub fn choose_world(&self, ctx: &Context, key: &str) {
        pages::choose_world(ctx, key);
    }

    /// Opens the queue: Now Playing with the queue beside the cover
    /// (scripted screenshots).
    pub fn open_queue(&mut self, ctx: &Context, out: &mut Vec<Command>) {
        self.show_queue(ctx, out);
    }

    /// Opens the side pane on the lyrics (scripted screenshots).
    pub fn open_lyrics(&mut self) {
        if self.now_playing {
            self.chrome.np_slot = NpSlot::Lyrics;
        } else {
            self.pane = Some(PaneTab::Lyrics);
        }
    }

    /// Switches the library page's tab (scripted screenshots).
    pub fn set_library_tab(&mut self, tab: nikgob_view::LibraryTab) {
        self.pages.library_tab = tab;
    }

    /// Types into the library's filter (scripted screenshots).
    pub fn set_library_filter(&mut self, text: &str) {
        self.pages.set_library_filter(text);
    }

    /// The library as a list (true) or a grid (scripted screenshots).
    pub fn set_library_list(ctx: &egui::Context, list: bool) {
        pages::set_library_list(ctx, list);
    }

    /// Opens Settings on a tab, scrolled to the row named `row` (scripted
    /// screenshots): `sources`, `spotify`, `youtube`, `import`, `torrents`,
    /// `appearance`, anything else the controls of the track screen.
    pub fn open_settings_at(&mut self, row: &str) {
        use pages::SettingId as S;
        // As «Настройки текста…» under the lyrics opens it: the whole group.
        if row == "lyrics-group" {
            self.pages.open_setting_group(lyrics_ui::SETTINGS);
            return;
        }
        self.pages.open_setting(match row {
            "sources" | "spotify" => S::SpotifyEnabled,
            "youtube" => S::YouTubeEnabled,
            "deezer" => S::DeezerEnabled,
            "soundcloud" => S::SoundCloudEnabled,
            "personal" => S::SpotifyPersonal,
            "soloist" => S::SpotifySoloist,
            "sound" => S::Gapless,
            "likes" => S::LikeDownload,
            "import" => S::ImportFiles,
            "torrents" => S::Prowlarr,
            "download-sources" => S::DownloadSource(nikgob_view::SourceKey::Torrents),
            "soulseek" => S::Soulseek,
            "graph" => S::GraphUse,
            "no-repeat" => S::RadioNoRepeat,
            "random" => S::RadioExplore,
            "unheard" => S::RadioUnheard,
            "recs" => S::RadioNewShare,
            "autoplay" => S::Autoplay,
            "search-play" => S::SearchPlay,
            "mixes" => S::MixesProfiles,
            "dislikes" => S::DislikeButton,
            "blacklist" => S::Marks,
            "albums" => S::AlbumOpens,
            "stream-analysis" => S::StreamAnalysis,
            "appearance" => S::Theme,
            "pane" => S::PaneLyrics,
            "lyrics" => S::LyricsModels,
            "lyrics-timing" => S::LyricsGenius,
            "lyrics-ui" => S::LyricsUi,
            "lyrics-highlight" => S::LyricsWordsAll,
            "lyrics-check" => S::LyricsCheck,
            "lyrics-lead" => S::LyricsLead,
            "lyrics-edges" => S::LyricsSoftEdges,
            "lyrics-streams" => S::LyricsStreamKeep,
            "lyrics-presets" => S::Preset(nikgob_view::presets::Area::Lyrics),
            "home-sections" => S::HomeSections,
            "source-presets" => S::Preset(nikgob_view::presets::Area::Sources),
            "devices" => S::MeshDevices,
            "keep-masters" => S::MeshKeepMasters,
            "api" => S::ApiEnabled,
            _ => S::CoverClickPause,
        });
    }

    /// Opens «Карта сети», with the passport of device `select` open (0:
    /// this computer, n: the n-th paired one) (scripted screenshots).
    pub fn open_devices_map(&mut self, ctx: &Context, state: &ViewState, select: Option<usize>, out: &mut Vec<Command>) {
        if let Some(i) = select {
            let id = if i == 0 { Some(String::new()) } else { state.mesh.nodes.get(i - 1).map(|n| n.id.clone()) };
            if let Some(id) = id {
                self.pages.mesh.map.select(id);
            }
        }
        self.navigate(ctx, Route::Devices, out);
    }

    /// Opens «Добавить устройство» on «Подключить» or, with `install`,
    /// «Установить» (scripted screenshots).
    pub fn open_add_device(&mut self, ctx: &Context, state: &ViewState, install: bool, out: &mut Vec<Command>) {
        pages::open_add_device(ctx, state, &mut self.pages, install, out);
    }

    /// Opens the code window of the first nearby device (scripted
    /// screenshots).
    pub fn open_pair_code(&mut self, ctx: &Context, state: &ViewState, out: &mut Vec<Command>) {
        pages::open_pair_code(ctx, state, &mut self.pages, out);
    }

    /// Opens the output popover («Где играть») out of the output button
    /// drawn last, else out of the window's bottom right corner (scripted
    /// screenshots).
    pub fn open_output(&mut self, ctx: &Context) {
        let corner = ctx.content_rect().right_bottom();
        let fallback = Rect::from_min_max(corner - vec2(120.0, 72.0), corner - vec2(80.0, 32.0));
        self.chrome.output.open_from_last(fallback);
    }

    /// Opens the cookies form of «Сервисы»
    /// (scripted screenshots).
    pub fn unfold_sources(&mut self) {
        self.pages.unfold_sources();
    }

    /// Types into the search bar (used by scripted screenshots); sent like
    /// typed text, once typing pauses.
    pub fn set_search(&mut self, query: &str, _out: &mut Vec<Command>) {
        self.search = query.to_owned();
        self.search_edited = Some(std::time::Instant::now());
    }
}

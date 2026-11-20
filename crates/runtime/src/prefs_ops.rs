use super::*;

impl Worker {
    /// `Command::SetPrefs`: every preference at once, and what each change
    /// sets going.
    pub(crate) fn set_prefs(&mut self, mut p: Box<nikgob_view::Prefs>) {
        let old = self.state.read().prefs.clone();
        if p.masters_dir.as_os_str().is_empty() && !old.masters_dir.as_os_str().is_empty() {
            p.masters_dir = old.masters_dir.clone();
        }
        self.playback.configure(crossfade(&p), p.normalization);
        if (old.eq_on, old.eq_gains) != (p.eq_on, p.eq_gains) {
            self.playback.set_equalizer(p.eq_on, p.eq_gains);
        }
        let speed_changed = (old.speed_enabled, old.keep_timbre) != (p.speed_enabled, p.keep_timbre);
        let speed_enabled = p.speed_enabled;
        let lang_changed = old.language != p.language;
        if lang_changed {
            nikgob_core::i18n::apply_pref(p.language);
        }
        self.torrent_prefs(&p);
        self.publish(|s| {
            s.prefs = *p.clone();
            s.speeds.enabled = speed_enabled;
        });
        if speed_changed {
            self.apply_speed(None);
        }
        self.save_prefs();
        self.import_prefs(&p);
        if (old.check_fake_lossless, old.convert_fake_lossless, old.fake_flac_auto)
            != (p.check_fake_lossless, p.convert_fake_lossless, p.fake_flac_auto)
        {
            self.quality_prefs();
        }
        self.align_settings();
        if (old.lyrics_words_playing, old.lyrics_words_all, old.lyrics_lines_auto, old.lyrics_check, old.lyrics_recognize, old.lyrics_streams, old.lyrics_genius)
            != (p.lyrics_words_playing, p.lyrics_words_all, p.lyrics_lines_auto, p.lyrics_check, p.lyrics_recognize, p.lyrics_streams, p.lyrics_genius)
        {
            self.lyrics_prefs_changed();
        }
        let lyrics_track = self.state.read().lyrics.track;
        if !old.save_found_lyrics && p.save_found_lyrics
            && let Some(track) = lyrics_track
        {
            self.embed_lyrics(track);
        }
        if old.gapless != p.gapless {
            self.preload_next();
        }
        if old.output_device != p.output_device {
            self.switch_device(p.output_device.clone());
        }
        if (old.spotify_enabled, old.youtube_enabled, old.deezer_enabled, old.soundcloud_enabled)
            != (p.spotify_enabled, p.youtube_enabled, p.deezer_enabled, p.soundcloud_enabled)
        {
            self.soloist_wanted();
            self.publish_accounts();
            self.preload_next();
        }
        if old.album_like_tracks != p.album_like_tracks {
            // The album hearts read the likes the new way.
            self.lib.set_album_likes_tracks(p.album_like_tracks);
            self.load_library();
            let release = self.state.read().release.as_ref().map(|r| r.card.id);
            if let Some(r) = release {
                self.load_release(r);
            }
        }
        if old.library_only_downloaded != p.library_only_downloaded {
            self.load_library();
        }
        if old.album_mode != p.album_mode || old.album_other_versions != p.album_other_versions {
            let release = self.state.read().release.as_ref().map(|r| r.card.id);
            if let Some(r) = release {
                self.load_release(r);
            }
        }
        if old.album_mode != p.album_mode {
            self.load_home();
        }
        if old.auto_download_limit != p.auto_download_limit {
            self.wish_tick();
        }
        // A source turned on: what was not found is searched for again once
        // it can search.
        for key in nikgob_view::SourceKey::ALL {
            if !old.source_on(key) && p.source_on(key) {
                self.streaming.wish.enabled.insert(key);
            } else if !p.source_on(key) {
                self.streaming.wish.enabled.remove(&key);
            }
        }
        if old.source_on(nikgob_view::SourceKey::Soulseek) != p.source_on(nikgob_view::SourceKey::Soulseek) {
            self.soulseek_apply();
        }
        if old.source_order() != p.source_order() {
            self.publish_sources();
            self.publish_wishlist();
            self.wish_sources_ready();
            self.wish_tick();
        }
        if old.stream_cache_mb != p.stream_cache_mb {
            self.apply_stream_cache();
        }
        if old.stream_analysis != p.stream_analysis {
            self.stream_analysis_pref(p.stream_analysis);
        }
        if (old.sound_neural, old.sound_device) != (p.sound_neural, p.sound_device) {
            self.sound_neural_prefs();
        }
        if old.mixes_downloaded_only != p.mixes_downloaded_only {
            // The rows it hides go (or come back) at once; the
            // mixes follow when they are picked again.
            self.load_home();
            self.recs_prefs_changed();
        }
        if old.home_card != p.home_card || old.home_hidden_worlds != p.home_hidden_worlds {
            self.load_home();
        }
        if old.mixes_downloaded_only != p.mixes_downloaded_only || old.radio_new_share != p.radio_new_share || old.radio_new_share_by != p.radio_new_share_by {
            self.radio_source_changed();
        }
        if old.dislike_in_recs != p.dislike_in_recs
            || old.background_counts_less != p.background_counts_less
            || old.profiles_split != p.profiles_split
            || old.profile_names != p.profile_names
            || old.mixes_profiles != p.mixes_profiles
            || old.mixes_everything != p.mixes_everything
            || old.mixes_moods != p.mixes_moods
            || old.mixes_on_repeat != p.mixes_on_repeat
            || old.background_cards != p.background_cards
            || old.home_hidden_mixes != p.home_hidden_mixes
            || old.graph_use != p.graph_use
            || old.graph_personal != p.graph_personal
            || old.library_as_taste != p.library_as_taste
            || old.radio_unheard != p.radio_unheard
            || old.radio_new_share != p.radio_new_share
            || old.radio_new_share_by != p.radio_new_share_by
        {
            self.recs_prefs_changed();
        }
        if (!old.graph_services && p.graph_services) || (!old.graph_open_data && p.graph_open_data) {
            self.graph_now();
        }
        if old.autoplay && !p.autoplay {
            self.autoplay_off();
        }
        if !old.autoplay && p.autoplay {
            self.publish_player();
        }
        if lang_changed {
            let loc = p.resolved_locale();
            if self.connect.remote() {
                self.publish(move |s| {
                    s.player.sync_context(loc);
                    if let Some(r) = s.queue.radio.as_mut() {
                        r.sync_locale(loc);
                    }
                });
            }
            self.relocalize_recs(loc);
            self.publish_now_playing();
            self.load_library();
            if !self.state.read().library.downloaded.is_empty() {
                self.load_downloaded(0, self.downloaded_shown().max(crate::import::DOWNLOADED_PAGE));
            }
            if !self.state.read().library.trash.is_empty() {
                self.load_trash();
            }
            self.load_home();
            let (pl, world, release, artist) = {
                let s = self.state.read();
                (
                    s.playlist.as_ref().map(|d| d.card.id),
                    s.world.as_ref().map(|w| w.world.key.clone()),
                    s.release.as_ref().map(|r| r.card.id),
                    s.artist.as_ref().map(|a| a.card.id),
                )
            };
            if let Some(pl) = pl {
                self.load_playlist(pl);
            }
            if let Some(w) = world {
                self.load_world(&w);
            }
            if let Some(r) = release {
                self.load_release(r);
            }
            if let Some(a) = artist {
                self.load_artist(a);
            }
            self.publish_downloads();
            // Everything that holds words made in the old language: the
            // cards of «Подтверди», the wishlist's rows and cards, the
            // running downloads' lines.
            self.publish_fake_offers();
            self.replica_publish();
            self.publish_api_confirms();
            self.publish_wishlist();
            self.relocalize_running();
            self.relocalize_torrents();
            // Cold-start jobs and their history are data too: said again.
            self.publish_cold_jobs();
        }
    }
}

impl Worker {
    fn sources_file(&self) -> Option<std::path::PathBuf> {
        self.settings_path.as_ref().map(|p| nikgob_sources::config::beside(p))
    }

    /// `sources.toml` changed: the new lists take over. A pause of an
    /// instance that stays is kept. A file that does not parse is left as
    /// it is, and the lists in use stay.
    pub(crate) fn reload_sources_file(&mut self) {
        let Some(path) = self.sources_file() else { return };
        let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if modified.is_none() || modified == self.sources_mtime {
            return;
        }
        self.sources_mtime = modified;
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                log::warn!("sources.toml: {e}");
                return;
            }
        };
        let defaults = source_defaults();
        let lists = match nikgob_sources::config::parse(&text, defaults) {
            Ok(l) => l,
            Err(e) => {
                log::warn!("sources.toml: {e}");
                return;
            }
        };
        if self.qobuz.as_ref().is_some_and(|q| q.instances_now() != (lists.qobuz.clone(), lists.qobuz_own)) {
            if let Some(q) = &self.qobuz {
                q.set_instances(&lists.qobuz, lists.qobuz_own);
            }
            log::info!("sources.toml: qobuz {:?}", lists.qobuz);
        }
        if self.tidal.as_ref().is_some_and(|t| t.instances_now() != (lists.tidal.clone(), lists.tidal_own)) {
            if let Some(t) = &self.tidal {
                t.set_instances(&lists.tidal, lists.tidal_own);
            }
            log::info!("sources.toml: tidal {:?}", lists.tidal);
        }
        if let Some(j) = &self.junkyard {
            j.set_sites(&lists.junkyard);
            log::info!("sources.toml: junkyard {:?}", lists.junkyard);
        }
    }

    /// Creates `sources.toml` if it is missing and opens it for editing.
    pub(crate) fn open_sources_config(&mut self) {
        let loc = self.locale();
        let err = nikgob_core::i18n::errors_in(loc);
        let Some(settings) = self.settings_path.clone() else {
            self.toast_err(err.no_settings_path, None);
            return;
        };
        let path = nikgob_sources::config::beside(&settings);
        let saved = nikgob_core::settings::Settings::load(&settings).unwrap_or_default();
        if let Err(e) = nikgob_sources::config::ensure(&path, source_defaults(), &saved.sources) {
            log::error!("sources.toml: {e}");
            self.toast_err(err.sources_file_failed, None);
            return;
        }
        self.sources_mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if let Err(e) = open_file(&path) {
            log::warn!("sources.toml: {e}");
            self.toast_info(
                nikgob_core::i18n::ErrorStrings::instances_file_in(loc, &path.display().to_string()),
                None,
            );
        }
    }
}

fn source_defaults() -> nikgob_sources::config::Defaults<'static> {
    nikgob_sources::config::Defaults {
        qobuz: nikgob_sources::qobuz::DEFAULT_INSTANCES,
        tidal: nikgob_sources::tidal::DEFAULT_INSTANCES,
    }
}

/// Opens `path` with the system editor (a folder opener on Windows selects
/// the file when it has no editor of its own). A test never launches one.
fn open_file(path: &std::path::Path) -> std::io::Result<()> {
    if cfg!(test) {
        log::info!("sources.toml: not opened from a test ({})", path.display());
        return Ok(());
    }
    let mut cmd = if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.arg("/C").arg("start").arg("").arg(path);
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(path);
        c
    };
    let child = cmd.spawn()?;
    #[cfg(unix)]
    let _ = std::thread::Builder::new().name("opener".into()).spawn(move || {
        let mut child = child;
        let _ = child.wait();
    });
    #[cfg(not(unix))]
    drop(child);
    Ok(())
}

impl Worker {
    /// Stores the Prowlarr address and key in settings.toml (never in prefs).
    pub(crate) fn set_indexer(&mut self, url: String, api_key: String) {
        let err = nikgob_core::i18n::errors_in(self.locale());
        let Some(path) = self.settings_path.clone() else {
            self.toast_err(err.no_settings_path, None);
            return;
        };
        // A file that cannot be read or parsed is not replaced by defaults: that
        // would silently drop the other secrets stored in it.
        let mut settings = match nikgob_core::settings::Settings::load(&path) {
            Ok(s) => s,
            Err(e) => {
                log::error!("loading settings: {e}");
                self.toast_err(err.settings_unreadable, None);
                return;
            }
        };
        settings.indexer.prowlarr = Some(nikgob_core::settings::ProwlarrSettings {
            url: url.clone(),
            api_key,
        });
        match settings.save(&path) {
            Ok(()) => {
                let had = self.indexer.is_some();
                self.indexer = search::indexer_from(Some(&path));
                let usable = self.indexer.is_some();
                self.publish(|s| {
                    s.prefs.prowlarr_url = url;
                    s.prefs.prowlarr_configured = usable;
                });
                if usable && !had {
                    self.streaming.wish.enabled.insert(nikgob_view::SourceKey::Torrents);
                }
                self.publish_sources();
                self.wish_sources_ready();
                if usable {
                    self.toast_ok(err.prowlarr_saved, None);
                } else {
                    self.toast_err(err.prowlarr_invalid, None);
                }
                // Whether it answers, right away (the settings show it).
                if usable {
                    self.check_indexer(String::new(), String::new());
                }
            }
            Err(e) => {
                log::error!("saving settings: {e}");
                self.toast_failed_save_prefs();
            }
        }
    }
}

impl Worker {
    /// MusicBrainz contact and AcoustID key → settings.toml, applied live.
    pub(crate) fn set_metadata(&mut self, contact: String, acoustid_key: Option<String>) {
        let err = nikgob_core::i18n::errors_in(self.locale());
        let Some(path) = self.settings_path.clone() else {
            self.toast_err(err.no_settings_path, None);
            return;
        };
        let mut settings = match nikgob_core::settings::Settings::load(&path) {
            Ok(s) => s,
            Err(e) => {
                log::error!("loading settings: {e}");
                self.toast_err(err.settings_unreadable, None);
                return;
            }
        };
        settings.metadata.contact = contact.trim().to_owned();
        if let Some(k) = acoustid_key {
            settings.metadata.acoustid_key = Some(k.trim().to_owned()).filter(|k| !k.is_empty());
        }
        if let Err(e) = settings.save(&path) {
            log::error!("saving settings: {e}");
            self.toast_failed_save_prefs();
            return;
        }
        self.set_metadata_config(&metadata_config(&settings.metadata));
        let configured = settings.metadata.acoustid_key.is_some();
        self.publish(|s| {
            s.prefs.metadata_contact = settings.metadata.contact;
            s.prefs.acoustid_configured = configured;
        });
        self.toast_ok(err.saved, None);
    }

    /// The Last.fm key of the graph of similar music → settings.toml
    /// (empty: none), applied live.
    pub(crate) fn set_lastfm_key(&mut self, key: &str) {
        let Some(configured) = self.set_metadata_secret(|m| &mut m.lastfm_key, key) else { return };
        self.publish(|s| s.prefs.lastfm_configured = configured);
        let err = nikgob_core::i18n::errors_in(self.locale());
        if configured {
            self.toast_ok(err.saved, None);
        } else {
            self.toast_info(err.key_removed, None);
        }
    }

    /// The ListenBrainz token of the graph's popularity → settings.toml
    /// (empty: none), applied live: the popularity is asked again at once.
    pub(crate) fn set_listenbrainz_token(&mut self, token: &str) {
        let Some(configured) = self.set_metadata_secret(|m| &mut m.listenbrainz_token, token) else { return };
        self.publish(|s| s.prefs.listenbrainz_configured = configured);
        self.graph_popularity_reopened();
        let err = nikgob_core::i18n::errors_in(self.locale());
        if configured {
            self.toast_ok(err.saved, None);
        } else {
            self.toast_info(err.token_removed, None);
        }
    }

    /// Keeps one secret of the online metadata in settings.toml (empty:
    /// none) and applies the config live; whether one is set now, none when
    /// it could not be kept (a toast says why).
    fn set_metadata_secret(&mut self, field: impl FnOnce(&mut nikgob_core::settings::MetadataSettings) -> &mut Option<String>, value: &str) -> Option<bool> {
        let err = nikgob_core::i18n::errors_in(self.locale());
        let Some(path) = self.settings_path.clone() else {
            self.toast_err(err.no_settings_path, None);
            return None;
        };
        let mut settings = match nikgob_core::settings::Settings::load(&path) {
            Ok(s) => s,
            Err(e) => {
                log::error!("loading settings: {e}");
                self.toast_err(err.settings_unreadable, None);
                return None;
            }
        };
        let slot = field(&mut settings.metadata);
        *slot = Some(value.trim().to_owned()).filter(|k| !k.is_empty());
        let configured = slot.is_some();
        if let Err(e) = settings.save(&path) {
            log::error!("saving settings: {e}");
            self.toast_failed_save_prefs();
            return None;
        }
        self.set_metadata_config(&metadata_config(&settings.metadata));
        Some(configured)
    }
}

/// The online metadata's config of settings.toml's.
fn metadata_config(m: &nikgob_core::settings::MetadataSettings) -> MetadataConfig {
    let mut cfg = MetadataConfig::new(m.contact.clone());
    if let Some(k) = &m.acoustid_key {
        cfg = cfg.with_acoustid_key(k.clone());
    }
    if let Some(k) = &m.lastfm_key {
        cfg = cfg.with_lastfm_key(k.clone());
    }
    if let Some(t) = &m.listenbrainz_token {
        cfg = cfg.with_listenbrainz_token(t.clone());
    }
    cfg
}

/// Puts `prefs` at `path` through a temporary file.
fn write_prefs(path: &std::path::Path, prefs: &nikgob_view::Prefs) -> std::io::Result<()> {
    let json = serde_json::to_vec_pretty(prefs).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

impl Worker {
    /// Writes the prefs; a failure is said, and they are written again at
    /// the next tick and at the end ([`Self::save_prefs_due`]).
    pub(crate) fn save_prefs(&mut self) {
        self.write_prefs_now(true);
    }

    /// Writes the prefs now: done, nothing left to write; failed, written
    /// again later, the error said when `say`. True when written.
    pub(crate) fn write_prefs_now(&mut self, say: bool) -> bool {
        let Some(path) = self.prefs_path.clone() else {
            self.prefs_unsaved = false;
            return true;
        };
        let prefs = self.state.read().prefs.clone();
        match write_prefs(&path, &prefs) {
            Ok(()) => {
                if std::mem::take(&mut self.prefs_failing) {
                    log::info!("{} written again", path.display());
                }
                self.prefs_unsaved = false;
                true
            }
            Err(e) => {
                if self.prefs_failing {
                    log::debug!("cannot save {} yet: {e}", path.display());
                } else {
                    log::error!("cannot save {}: {e}", path.display());
                }
                self.prefs_failing = true;
                self.prefs_unsaved = true;
                if say {
                    self.toast_failed_save_prefs();
                }
                false
            }
        }
    }

    /// At the start: an export next to the library that this database no
    /// longer matches (a backup of the database put back since) is written
    /// again at once, not at the next change.
    pub(crate) fn export_if_stale(&mut self) {
        if !self.userdata_export {
            return;
        }
        let dir = self.res.path(".musiclib/userdata");
        match self.lib.userdata_export_stale(&dir) {
            Ok(false) => {}
            Ok(true) => {
                log::info!("userdata export in {} is not this library's, writing it again", dir.display());
                let fp = self.lib.userdata_fingerprint().ok().map(|fp| (dir.clone(), fp));
                match self.lib.export_userdata(&dir) {
                    Ok(_) => self.exported_fp = fp,
                    Err(e) => log::warn!("userdata export to {}: {e}", dir.display()),
                }
            }
            Err(e) => log::warn!("userdata export in {}: {e}", dir.display()),
        }
    }

    /// Writes playlists, likes and history next to the library
    /// (the DB can be rebuilt from files + this export). At most once a
    /// minute while things change, and once more on shutdown; only when the
    /// user data itself changed (most publishes are playback, downloads and
    /// pages, which leave it alone).
    pub(crate) fn maybe_export(&mut self, force: bool) {
        if !self.userdata_export || self.revision == self.exported_rev {
            return;
        }
        if !force && self.last_export.elapsed() < Duration::from_secs(60) {
            return;
        }
        let dir = self.res.path(".musiclib/userdata");
        let fp = self.lib.userdata_fingerprint().ok().map(|fp| (dir.clone(), fp));
        if fp.is_some() && fp == self.exported_fp {
            self.exported_rev = self.revision;
            self.last_export = Instant::now();
            return;
        }
        match self.lib.export_userdata(&dir) {
            Ok(_) => {
                self.exported_rev = self.revision;
                self.last_export = Instant::now();
                self.exported_fp = fp;
            }
            Err(e) => log::warn!("userdata export to {}: {e}", dir.display()),
        }
    }
}

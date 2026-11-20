use super::*;

pub(super) fn build_home(snap: &Snapshot, input: &BuildInput, tiles_input: &nikgob_recs::TilesInput) -> RecHome {
    let lib = &input.lib;
    let recs = snap.recs();

    // «Твой микс»: the day's, unless it is a new day or «Обновить».
    let cached: Option<DailyCache> = lib
        .cache_get(DAILY_KEY)
        .ok()
        .flatten()
        .and_then(|(_, json)| serde_json::from_str(&json).ok())
        .filter(|c: &DailyCache| {
            c.day == input.day
                && !input.refresh_daily
                && (c.history || !snap.model.taste.has_history())
                && c.local_only == input.local_only
                && new_kept(c.new_at, input.new_at, input.local_only)
        });
    let (picks, streams) = match cached {
        Some(c) => (c.picks, c.streams),
        None => {
            let new: Vec<&StreamTrack> = input.new.iter().filter(|t| !input.local_only && snap.is_new(t)).collect();
            let candidates: Vec<NewTrack> = new.iter().map(|t| snap.new_track(t)).collect();
            let seed = if input.refresh_daily { fastrand::u64(..) } else { day_seed(&input.day) };
            let picks = recs.daily_mix(
                &candidates,
                &DailyParams {
                    new_share: input.new_share,
                    seed,
                    ..DailyParams::default()
                },
            );
            let streams: Vec<StreamTrack> = new
                .into_iter()
                .filter(|t| picks.iter().any(|p| p.item == Item::Stream(t.id.clone())))
                .cloned()
                .collect();
            let cache = DailyCache {
                day: input.day.clone(),
                picks: picks.clone(),
                streams: streams.clone(),
                history: snap.model.taste.has_history(),
                local_only: input.local_only,
                new_at: input.new_at,
            };
            if let Ok(json) = serde_json::to_string(&cache)
                && let Err(e) = lib.cache_put(DAILY_KEY, input.now, &json)
            {
                log::warn!("recs: keeping the daily mix: {e}");
            }
            (picks, streams)
        }
    };
    let daily_streams: HashMap<RemoteId, StreamTrack> = streams.into_iter().map(|t| (t.id.clone(), t)).collect();
    // A pick disliked since (or gone) leaves the day's mix; its undo brings
    // it back.
    let daily: Vec<Pick> = picks.into_iter().filter(|p| still_fits(snap, p, &daily_streams)).collect();
    let daily_covers = covers_of(snap, &daily, &daily_streams);
    let (daylist_key, daylist) = build_daylist(snap, input);
    let tiles = tiles::build(snap, input, tiles_input);
    let built_lanes = lanes::build(snap, input, tiles_input);
    let top = if v::HOME_CARD { top::build(snap, input, tiles_input, None) } else { Vec::new() };

    // Moods: covers of the releases most liked among each mood's top.
    let mut moods = Vec::new();
    if !snap.index.is_empty() {
        for m in recs.mood_order() {
            let mut top: Vec<(TrackId, f32)> = recs.mood_pool(m).into_iter().take(MOOD_COVER_POOL).collect();
            let aff = |id: TrackId| snap.catalog.position(id).map_or(0.0, |i| snap.model.taste.affinity_at(i));
            top.sort_by(|a, b| aff(b.0).total_cmp(&aff(a.0)));
            let mut covers: Vec<ReleaseId> = Vec::new();
            for (id, _) in top {
                if let Some(r) = snap.catalog.get(id).and_then(|t| t.release)
                    && !covers.contains(&r)
                {
                    covers.push(r);
                    if covers.len() == COVER_CANDIDATES {
                        break;
                    }
                }
            }
            moods.push((m, covers));
        }
    }

    // Style mixes from the last centres.
    let previous: Vec<Vec<f32>> = lib
        .cache_get(STYLES_KEY)
        .ok()
        .flatten()
        .and_then(|(_, json)| serde_json::from_str(&json).ok())
        .unwrap_or_default();
    let mixes = recs.style_mixes(&previous, day_seed(&input.day));
    if !mixes.is_empty() {
        let centres: Vec<&Vec<f32>> = mixes.iter().map(|m| &m.centroid).collect();
        if let Ok(json) = serde_json::to_string(&centres) {
            let _ = lib.cache_put(STYLES_KEY, input.now, &json);
        }
    }
    let styles = mixes
        .into_iter()
        .map(|m| {
            let names: Vec<String> = m
                .name_artists
                .iter()
                .filter_map(|a| lib.get_artist(*a).ok().flatten().map(|a| a.name))
                .collect();
            let title = style_title(&names, m.artists.len(), input.locale);
            let subtitle = style_subtitle(&m, input.locale);
            StyleHome { mix: m, artist_names: names, title, subtitle }
        })
        .collect();

    RecHome {
        day: input.day.clone(),
        epoch: input.epoch.key.clone(),
        daily,
        daily_streams,
        daily_covers,
        history: snap.model.taste.has_history(),
        moods,
        styles,
        daylist_key,
        daylist,
        tiles,
        lanes: built_lanes.lanes,
        blend_profiles: built_lanes.blend_profiles,
        blend_profile_titles: built_lanes.blend_titles,
        top,
    }
}

/// The daylist of now's part of the day: the one kept for this part,
/// unless «Обновить» or a history appeared since.
fn build_daylist(snap: &Snapshot, input: &BuildInput) -> (String, Option<DaylistHome>) {
    let lib = &input.lib;
    let loc = input.locale;
    let history = snap.model.taste.has_history();
    let (part, day, key) = daylist_key(input.now, input.utc_offset_min);
    let cached = cache_read::<DaylistCache>(lib, DAYLIST_KEY)
        .filter(|c| c.key == key && !input.refresh_daylist && (c.history || !history) && c.local_only == input.local_only && new_kept(c.new_at, input.new_at, input.local_only));
    let c = match cached {
        Some(c) => c,
        None => {
            let new: Vec<&StreamTrack> = input.new.iter().filter(|t| !input.local_only && snap.is_new(t)).collect();
            let candidates: Vec<NewTrack> = new.iter().map(|t| snap.new_track(t)).collect();
            let seed = if input.refresh_daylist { fastrand::u64(..) } else { day_seed(&key) };
            let params = RadioParams {
                new_share: input.new_share,
                seed,
            };
            let (centre, state, picks) = snap.recs().daylist(part, &candidates, DAYLIST_LEN, &params);
            let streams = new
                .into_iter()
                .filter(|t| picks.iter().any(|p| p.item == Item::Stream(t.id.clone())))
                .cloned()
                .collect();
            let c = DaylistCache {
                key: key.clone(),
                title: daylist_title_in(loc, centre.axes, part, day),
                centre: Some(centre.axes.to_array()),
                state,
                picks,
                streams,
                history,
                local_only: input.local_only,
                new_at: input.new_at,
            };
            cache_write(lib, DAYLIST_KEY, input.now, &c);
            c
        }
    };
    let streams: HashMap<RemoteId, StreamTrack> = c.streams.into_iter().map(|t| (t.id.clone(), t)).collect();
    let picks: Vec<Pick> = c.picks.into_iter().filter(|p| still_fits(snap, p, &streams)).collect();
    if picks.is_empty() {
        return (key, None);
    }
    let centre = c.centre.or_else(|| match &c.state.seed {
        nikgob_recs::RadioSeed::Daylist { centre, .. } => Some(*centre),
        _ => None,
    });
    let title = match centre {
        Some(axes) => daylist_title_in(loc, Axes::from_array(axes), part, day),
        None => c.title,
    };
    // The number of songs only: no «what it is made of», no «until 23:00».
    let subtitle = tracks_count(picks.len(), loc);
    let covers = covers_of(snap, &picks, &streams);
    let home = DaylistHome {
        part,
        day,
        centre,
        kicker: daylist_kicker(part, loc),
        title,
        subtitle,
        state: c.state,
        picks,
        streams,
        covers,
    };
    (key, Some(home))
}

/// A mix of «Миксы для тебя» as [`HomeParts`] keeps it.
pub(super) struct LaneParts {
    key: v::MixKey,
    title: String,
    mix_title: Option<v::MixTitle>,
    short: Vec<String>,
    artists: Vec<String>,
    /// The new songs' artists.
    new_artists: Vec<String>,
    covers: Vec<CoverRef>,
    new_cover: Option<String>,
    /// Its tracks, read when the parts are resolved.
    rows: Vec<TrackId>,
    /// «С новым»: the new songs it mixes in, in the order they play.
    pub(super) new_rows: Vec<StreamTrack>,
    blend_base: Option<u32>,
    why: Option<String>,
}

/// The recommendation parts of the home view, ready to be drawn: owned
/// copies of the build's picks, so they can be resolved on another thread
/// (the home's builder, [`crate::home_build`]).
pub(crate) struct HomeParts {
    pub(crate) locale: Locale,
    hero: Option<(String, String, Vec<CoverRef>)>,
    /// The library tracks of «Твой микс», read when resolved.
    mix: Vec<TrackId>,
    /// Kicker, title, subtitle, covers.
    daylist: Option<(String, String, String, Vec<CoverRef>)>,
    /// Id, kicker, title, subtitle, covers, mix_title.
    tiles: Vec<(v::MixId, String, String, String, Vec<CoverRef>, Option<v::MixTitle>)>,
    moods: Vec<(v::Mood, Vec<CoverRef>)>,
    styles: Vec<(String, String, Vec<CoverRef>)>,
    discover: Vec<v::StreamTrackRow>,
    new_releases: Vec<v::StreamAlbumCard>,
    pub(super) lanes: Vec<LaneParts>,
    blend_profiles: Vec<v::BlendProfile>,
    history: bool,
    /// «Только скачанное»: nothing on the home may play without a file.
    local_only: bool,
    /// «Карточка наверху главной»: what its world shows is not repeated
    /// below.
    card: bool,
}

impl HomeParts {
    /// The parts of `home` (none before the first build) in the active locale. It reads
    /// nothing: the rows are read when the parts are resolved.
    #[cfg(test)]
    pub(crate) fn of(
        home: Option<&RecHome>,
        discover: Vec<v::StreamTrackRow>,
        new_releases: Vec<v::StreamAlbumCard>,
        local_only: bool,
        card: bool,
    ) -> Self {
        Self::of_in(nikgob_core::i18n::locale(), home, discover, new_releases, local_only, card)
    }

    pub(crate) fn of_in(
        loc: Locale,
        home: Option<&RecHome>,
        discover: Vec<v::StreamTrackRow>,
        new_releases: Vec<v::StreamAlbumCard>,
        local_only: bool,
        card: bool,
    ) -> Self {
        let Some(h) = home else {
            return Self {
                locale: loc,
                hero: None,
                mix: Vec::new(),
                daylist: None,
                tiles: Vec::new(),
                moods: Vec::new(),
                styles: Vec::new(),
                discover,
                new_releases,
                lanes: Vec::new(),
                blend_profiles: Vec::new(),
                history: false,
                local_only,
                card,
            };
        };
        let n = h.daily.len();
        let subtitle = hero_subtitle(h.history, n, loc);
        let hero = (n > 0).then(|| (home_recs_in(loc).your_mix.to_owned(), subtitle, h.daily_covers.clone()));
        let mix = h
            .daily
            .iter()
            .filter_map(|p| match p.item {
                Item::Track(id) => Some(id),
                Item::Stream(_) => None,
            })
            .collect();
        let daylist = h
            .daylist
            .as_ref()
            .map(|d| (d.kicker.to_owned(), d.title.clone(), d.subtitle.clone(), d.covers.clone()));
        let tiles = h
            .tiles
            .iter()
            .map(|t| {
                (
                    t.mix_id(),
                    t.kicker(loc).to_owned(),
                    t.title.clone(),
                    t.duration.clone(),
                    t.covers.clone(),
                    Some(t.mix_title()),
                )
            })
            .collect();
        let lanes = h
            .lanes
            .iter()
            .map(|l| {
                let mut new_artists: Vec<String> = Vec::new();
                for t in &l.new {
                    if let Some(a) = t.artists.first()
                        && !new_artists.contains(&a.name)
                    {
                        new_artists.push(a.name.clone());
                    }
                }
                let new_cover = l.new.iter().find_map(|t| t.cover_url.clone());
                let rows = l.tracks.clone();
                // What «Слушать» plays of them: those that find a place.
                let new_rows: Vec<StreamTrack> = l
                    .picks(true)
                    .into_iter()
                    .filter_map(|p| match p.item {
                        Item::Stream(r) => l.new.iter().find(|t| t.id == r).cloned(),
                        Item::Track(_) => None,
                    })
                    .collect();
                LaneParts {
                    key: l.key,
                    title: l.title.clone(),
                    mix_title: lane_mix_title(l.key, l.ru_title(), l.side_flavor, Some(&l.labels)),
                    short: l.short.clone(),
                    artists: l.artists.clone(),
                    new_artists,
                    covers: l.covers.clone(),
                    new_cover,
                    rows,
                    new_rows,
                    blend_base: l.blend_base,
                    why: l.why.clone(),
                }
            })
            .collect();
        let moods = h
            .moods
            .iter()
            .map(|(m, c)| (to_view_mood(*m), c.iter().copied().map(CoverRef::Release).collect()))
            .collect();
        let styles = h
            .styles
            .iter()
            .map(|s| (s.title.clone(), s.subtitle.clone(), s.mix.covers.iter().copied().map(CoverRef::Release).collect()))
            .collect();
        Self {
            locale: loc,
            hero,
            mix,
            daylist,
            tiles,
            moods,
            styles,
            discover,
            new_releases,
            lanes,
            blend_profiles: h.blend_profiles.clone(),
            history: h.history,
            local_only,
            card,
        }
    }

    /// Fills the recommendation fields of `home`.
    pub(crate) fn apply(self, home: &mut v::HomeView, res: &mut crate::Resolver) {
        self.resolve(res).apply(home);
    }

    /// The new-music rows, made after the parts were taken (the home's
    /// builder makes them, [`super::discover::DiscoverInput::rows`]).
    pub(crate) fn with_discover(mut self, discover: Vec<v::StreamTrackRow>, new_releases: Vec<v::StreamAlbumCard>) -> Self {
        self.discover = discover;
        self.new_releases = new_releases;
        self
    }

    /// Reads the rows and resolves the covers now, into what fills `home`
    /// with them ([`RecsFill::apply`] touches neither the library nor the
    /// resolver, so it can run under the state lock).
    pub(crate) fn resolve(self, res: &mut crate::Resolver) -> RecsFill {
        let local_only = self.local_only;
        let card = self.card;
        let lib = res.library().clone();
        let rows = |ids: &[TrackId]| -> Vec<nikgob_library::TrackRow> {
            ids.iter().filter_map(|id| lib.get_track_row(*id).ok().flatten()).collect()
        };
        let mix_rows = rows(self.mix.as_slice());
        let lane_rows: Vec<Vec<nikgob_library::TrackRow>> = self.lanes.iter().map(|l| rows(l.rows.as_slice())).collect();
        let lane_new: Vec<Vec<v::StreamTrackRow>> = self
            .lanes
            .iter()
            .map(|l| l.new_rows.iter().map(|t| crate::streams::stream_row_of(&lib, t, lib.track_for_stream(t).ok().flatten())).collect())
            .collect();
        let mut covers = |c: Vec<CoverRef>| -> Vec<v::Cover> {
            c.into_iter()
                .filter_map(|c| match c {
                    CoverRef::Release(r) => res.release_cover(r),
                    CoverRef::Web(url) => Some(v::Cover::web(url)),
                })
                .take(COVERS)
                .collect()
        };
        let loc = self.locale;
        let hero = self.hero.map(|(title, subtitle, c)| v::MixCard {
            id: v::MixId::Daily,
            title,
            mix_title: Some(v::MixTitle::YourMix),
            kicker: home_recs_in(loc).kicker_for_today.to_owned(),
            subtitle,
            covers: covers(c),
        });
        let daylist = self.daylist.map(|(kicker, title, subtitle, c)| v::MixCard {
            id: v::MixId::Daylist,
            title,
            mix_title: None,
            kicker,
            subtitle,
            covers: covers(c),
        });
        let tiles: Vec<_> = self
            .tiles
            .into_iter()
            .map(|(id, kicker, title, subtitle, c, mix_title)| v::MixCard {
                id,
                title,
                mix_title,
                kicker,
                subtitle,
                covers: covers(c),
            })
            .collect();
        let moods: Vec<_> = self
            .moods
            .into_iter()
            .map(|(m, c)| v::MixCard {
                id: v::MixId::Mood(m),
                title: m.title_in(loc).to_owned(),
                mix_title: Some(v::MixTitle::Mood(m.kind())),
                kicker: String::new(),
                subtitle: mood_subtitle(m, loc).to_owned(),
                covers: covers(c),
            })
            .collect();
        let style_mixes: Vec<_> = self
            .styles
            .into_iter()
            .enumerate()
            .map(|(i, (title, subtitle, c))| v::MixCard {
                id: v::MixId::Style(i as u8),
                title,
                mix_title: None,
                kicker: String::new(),
                subtitle,
                covers: covers(c),
            })
            .collect();
        let mut lane_covers: Vec<Vec<v::Cover>> = self.lanes.iter().map(|l| covers(l.covers.clone())).collect();
        distinct_leads(&mut lane_covers);
        let lanes: Vec<v::LaneCard> = self
            .lanes
            .into_iter()
            .zip(lane_covers)
            .zip(lane_rows)
            .zip(lane_new)
            .map(|(((l, covers), rows), new_tracks)| v::LaneCard {
                key: l.key,
                title: l.title,
                mix_title: l.mix_title,
                short_titles: l.short,
                artists: l.artists,
                new_artists: l.new_artists,
                covers,
                new_cover: l.new_cover.map(v::Cover::web),
                tracks: rows.iter().map(|t| res.track_in(t, loc)).collect(),
                new_tracks,
                blend_base: l.blend_base,
                queue: None,
                why: l.why,
            })
            .collect();
        let blend_profiles = self.blend_profiles;
        let history = self.history;
        let mix: Option<Vec<_>> = (!mix_rows.is_empty()).then(|| mix_rows.iter().map(|t| res.track_in(t, loc)).collect());
        let (discover, new_releases) = (self.discover, self.new_releases);
        RecsFill {
            locale: loc,
            hero,
            daylist,
            tiles,
            moods,
            style_mixes,
            mix,
            discover,
            new_releases,
            lanes,
            blend_profiles,
            history,
            local_only,
            card,
        }
    }
}

pub(crate) fn lane_mix_title(
    key: v::MixKey,
    ru_title: &str,
    side_flavor: Option<nikgob_recs::mixes::SideFlavor>,
    labels: Option<&lanes::PerLocale<lanes::LaneText>>,
) -> Option<v::MixTitle> {
    match key {
        v::MixKey::Daily(0) => Some(v::MixTitle::Daily),
        v::MixKey::Everything => Some(v::MixTitle::Everything),
        v::MixKey::Calm | v::MixKey::Lively => side_flavor
            .map(|flavor| v::MixTitle::Side {
                calm: key == v::MixKey::Calm,
                flavor,
            })
            .or_else(|| labels.map(|l| v::MixTitle::Localized(PerLocale::init(|loc| l.get(loc).title.clone())))),
        v::MixKey::OnRepeat => Some(v::MixTitle::OnRepeat),
        v::MixKey::Liked => Some(v::MixTitle::Liked),
        v::MixKey::Part(p) => Some(v::MixTitle::DayPart(p)),
        v::MixKey::Genre(g) => Some(v::MixTitle::Genre(g)),
        v::MixKey::World(w) => match v::MixTitle::from_legacy(ru_title) {
            mt @ v::MixTitle::World { .. } => Some(mt),
            _ => Some(v::MixTitle::World { world: None, mix: w.kind() }),
        },
        _ => match v::MixTitle::from_legacy(ru_title) {
            v::MixTitle::Custom(_) => {
                labels.map(|l| v::MixTitle::Localized(PerLocale::init(|loc| l.get(loc).title.clone())))
            }
            mt => Some(mt),
        },
    }
}

pub(crate) fn play_lane_mix_title(
    key: v::MixKey,
    ru_title: &str,
    display_title: &str,
    side_flavor: Option<nikgob_recs::mixes::SideFlavor>,
    labels: Option<&lanes::PerLocale<lanes::LaneText>>,
) -> v::MixTitle {
    lane_mix_title(key, ru_title, side_flavor, labels)
        .unwrap_or_else(|| v::MixTitle::Custom(display_title.to_owned()))
}

/// The recommendation parts of the home view resolved ([`HomeParts::resolve`]):
/// what fills the page, made off the state lock and, by the home's builder,
/// off the worker's thread.
pub(crate) struct RecsFill {
    locale: Locale,
    hero: Option<v::MixCard>,
    daylist: Option<v::MixCard>,
    tiles: Vec<v::MixCard>,
    moods: Vec<v::MixCard>,
    style_mixes: Vec<v::MixCard>,
    /// `None`: no library track of «Твой микс» (the page keeps its own).
    mix: Option<Vec<v::TrackRow>>,
    discover: Vec<v::StreamTrackRow>,
    new_releases: Vec<v::StreamAlbumCard>,
    lanes: Vec<v::LaneCard>,
    blend_profiles: Vec<v::BlendProfile>,
    history: bool,
    local_only: bool,
    card: bool,
}

impl RecsFill {
    /// Fills the recommendation fields of `home` (and drops what «Только
    /// скачанное», the rows' doubles and the card at the top leave out).
    pub(crate) fn apply(self, home: &mut v::HomeView) {
        home.hero = self.hero;
        home.daylist = self.daylist;
        home.tiles = self.tiles;
        home.moods = self.moods;
        home.style_mixes = self.style_mixes;
        if let Some(mix) = self.mix {
            home.mix = mix;
        }
        home.discover = self.discover;
        home.new_releases = self.new_releases;
        home.lanes = self.lanes;
        home.blend_profiles = self.blend_profiles;
        home.history |= self.history;
        if self.local_only {
            only_downloaded(home);
        }
        dedupe_rows(home);
        if self.card {
            card_first(home, self.locale);
        }
    }
}

/// A mix's card shows the first of its covers: each list of `covers` (one
/// for each card of the row, in the row's order) starts with a picture no
/// card before it starts with, while it has one.
pub(super) fn distinct_leads<T: PartialEq + Clone>(covers: &mut [Vec<T>]) {
    let mut leads: Vec<T> = Vec::new();
    for list in covers.iter_mut() {
        if let Some(at) = list.iter().position(|c| !leads.contains(c)) {
            list[..=at].rotate_right(1);
        }
        if let Some(first) = list.first() {
            leads.push(first.clone());
        }
    }
}

/// «Только скачанное» on the home: whatever would play without a file
/// goes, the rows of new music («Новое для тебя», «Новые релизы») and
/// albums streamed without one; the mixes are picked so already.
pub(crate) fn only_downloaded(home: &mut v::HomeView) {
    home.discover.clear();
    home.new_releases.clear();
    home.mix.retain(|t| t.has_local);
    home.continue_listening.retain(|c| matches!(c, v::ContinueCard::Release(r) if r.local_tracks > 0));
    home.long_unplayed.retain(|r| r.local_tracks > 0);
    home.recently_added.retain(|r| r.local_tracks > 0);
    home.listen_again.retain(|t| t.has_local);
    home.from_library.retain(|a| a.card.local_tracks > 0);
    home.shortcuts.retain(|s| !matches!(s, v::Shortcut::Release(r) if r.local_tracks == 0));
    for l in &mut home.lanes {
        l.tracks.retain(|t| t.has_local);
        l.new_artists.clear();
        l.new_cover = None;
        l.new_tracks.clear();
    }
    for w in &mut home.worlds {
        w.cards.retain(|c| !matches!(c.go, v::WorldGo::Stream(_)));
    }
}

/// With the card at the top: its world gets the new album of the services
/// as its third card, and what the world opened on shows (its albums on
/// the tiles and the stage, that new album) do not show again below; the
/// next in line takes their place.
pub(crate) fn card_first(home: &mut v::HomeView, loc: Locale) {
    if home.worlds.is_empty() {
        return;
    }
    if !home.new_releases.is_empty() {
        for k in 0..home.worlds.len() {
            if home.worlds[k].cards.iter().any(|c| matches!(c.go, v::WorldGo::Stream(_))) {
                continue;
            }
            if let Some(i) = crate::home_v2::world_release(&home.worlds[k], &home.new_releases) {
                let a = &home.new_releases[i];
                let mut name = crate::home_v2::short_album(&a.title, &home.worlds[k].title);
                // «FILM CONCERT LIVE RECORDING CD» reads «FILM CONCERT».
                for tail in [" LIVE RECORDING CD", " LIVE RECORDING", " CD"] {
                    if name.len() > tail.len() && name.get(name.len() - tail.len()..).is_some_and(|t| t.eq_ignore_ascii_case(tail)) {
                        name.truncate(name.len() - tail.len());
                    }
                }
                let card = v::WorldCard {
                    icon: v::WorldIcon::Album,
                    kicker: activity_in(loc).new_album.into(),
                    title: name,
                    go: v::WorldGo::Stream(a.remote.clone()),
                };
                let w = &mut home.worlds[k];
                w.cards.truncate(2);
                w.cards.push(card);
            }
        }
    }
    let first = &home.worlds[0];
    let mut shown: HashSet<v::ReleaseId> = home.worlds.iter().filter_map(|w| w.release).collect();
    shown.extend(first.release);
    let stream: Option<v::RemoteId> = first.cards.iter().find_map(|c| match &c.go {
        v::WorldGo::Stream(r) => Some(r.clone()),
        _ => None,
    });
    home.shortcuts.retain(|s| !matches!(s, v::Shortcut::Release(r) if shown.contains(&r.id)));
    home.from_library.retain(|a| !shown.contains(&a.card.id));
    if let Some(r) = stream {
        home.new_releases.retain(|a| a.remote != r);
    }
}

/// Keeps the first of each item in every row of the home: a row shows an
/// album, a song, an artist or a mix once (the UI keys its cards by them).
pub(crate) fn dedupe_rows(home: &mut v::HomeView) {
    fn keep_first<T, K: std::hash::Hash + Eq>(row: &mut Vec<T>, key: impl Fn(&T) -> K) {
        let mut seen = HashSet::new();
        row.retain(|x| seen.insert(key(x)));
    }
    keep_first(&mut home.mix, |t| t.id);
    keep_first(&mut home.tiles, |c| c.id);
    keep_first(&mut home.moods, |c| c.id);
    keep_first(&mut home.style_mixes, |c| c.id);
    keep_first(&mut home.discover, |r| r.remote.clone());
    keep_first(&mut home.new_releases, |a| a.remote.clone());
    keep_first(&mut home.continue_listening, |c| match c {
        v::ContinueCard::Release(r) => (Some(r.id), None),
        v::ContinueCard::Stream(a) => (None, Some(a.remote.clone())),
    });
    keep_first(&mut home.artist_mixes, |a| a.id);
    keep_first(&mut home.long_unplayed, |r| r.id);
    keep_first(&mut home.recently_added, |r| r.id);
}

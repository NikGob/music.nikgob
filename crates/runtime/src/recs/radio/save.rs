use super::*;
use serde::{Deserialize, Serialize};
use super::slots::{entries_at, places_of, queue_order};

/// Locale-neutral naming inputs persisted on an active radio run whose mix
/// title is synthesized dynamically (`Daylist`, `Style`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum ActiveMixNaming {
    Daylist {
        part: nikgob_recs::DayPart,
        centre: [f32; 3],
        day: i64,
    },
    Style {
        artists: Vec<String>,
        total_artists: usize,
    },
}

impl ActiveMixNaming {
    fn title_in(&self, loc: v::Locale) -> v::MixTitle {
        let title = match self {
            Self::Daylist { part, centre, day } => {
                nikgob_recs::daylist_title_in(loc, nikgob_recs::Axes::from_array(*centre), *part, *day)
            }
            Self::Style { artists, total_artists } => super::super::style_title(artists, *total_artists, loc),
        };
        v::MixTitle::Custom(title)
    }
}

/// What `session.json` keeps of a radio.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct RadioSave {
    pub(super) state: Engine,
    pub(super) title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) kind: Option<v::RadioTitle>,
    pub(super) context: EventContext,
    pub(super) seed: u64,
    pub(super) streams: Vec<(TrackId, RemoteId)>,
    pub(super) pool: Vec<StreamTrack>,
    #[serde(default)]
    pub(super) autoplay: bool,
    /// The queue places (play order) of the entries it added, and their
    /// tracks: entry ids are a run's own, places survive a restart.
    #[serde(default)]
    pub(super) added_at: Vec<(usize, TrackId)>,
    /// The card of «Миксы для тебя» a wave was started from (none: the
    /// plain «Моя волна»).
    #[serde(default)]
    pub(super) wave_preset: Option<v::MixKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) mix_naming: Option<ActiveMixNaming>,
}

impl Worker {
    /// The radio as `session.json` keeps it (while it plays its queue).
    pub(crate) fn radio_save(&mut self) -> Option<RadioSave> {
        if !self.radio_alive() {
            return None;
        }
        let r = &self.recs.radio;
        let run = r.run.as_ref()?;
        run.queue_gen?;
        let wave_preset = r.wave_preset.filter(|(id, _)| *id == run.id).and_then(|(_, k)| k);
        Some(RadioSave {
            state: run.state.clone(),
            title: run.title.title_in(v::Locale::Ru).into_owned(),
            kind: Some(run.title.clone()),
            context: run.ctx,
            seed: run.seed,
            streams: run.streams.iter().map(|(t, r)| (*t, r.clone())).collect(),
            pool: run.pool.values().cloned().collect(),
            autoplay: run.autoplay,
            added_at: places_of(&queue_order(&self.queue), &run.added),
            wave_preset,
            mix_naming: run.mix_naming.clone(),
        })
    }

    /// The radio of the restored queue goes on.
    pub(crate) fn radio_restore(&mut self, save: RadioSave) {
        // Its own entries again, by their places (a track that moved is
        // the user's now).
        let added = entries_at(&queue_order(&self.queue), &save.added_at);
        let r = &mut self.recs.radio;
        r.seq += 1;
        // A wave keeps the card it was started from.
        r.wave_preset = matches!(save.state.seed, RadioSeed::Wave(_)).then_some((r.seq, save.wave_preset));
        let mut title = save.kind.unwrap_or_else(|| v::RadioTitle::from_legacy(&save.title));
        if matches!(save.wave_preset, Some(v::MixKey::Calm))
            && title == v::RadioTitle::Mix(v::MixTitle::Mood(nikgob_core::i18n::MoodKind::Calm))
        {
            title = v::RadioTitle::Mix(v::MixTitle::Side {
                calm: true,
                flavor: v::SideMixFlavor::Plain,
            });
        }
        log::info!("radio «{}» goes on", title.title_in(v::Locale::Ru));
        let mut state = save.state;
        if is_wave(&state.seed) {
            state.set_wave_memory(super::super::cache_read(&self.lib, WAVE_MEMORY_KEY).unwrap_or_default());
        }
        self.streaming.learn(&save.pool);
        let origin = title.to_queue_origin();
        let r = &mut self.recs.radio;
        r.run = Some(RadioRun {
            id: r.seq,
            state,
            title,
            ctx: save.context,
            seed: save.seed,
            queue_gen: Some(self.queue.generation()),
            streams: save.streams.into_iter().collect(),
            pool: save.pool.into_iter().map(|t| (t.id.clone(), t)).collect(),
            extending: false,
            want: false,
            marks: Vec::new(),
            pending_new: Vec::new(),
            resume: false,
            vibe_skip: None,
            autoplay: save.autoplay,
            added,
            heard: Vec::new(),
            asked: HashSet::new(),
            queued: (0, 0),
            via: HashMap::new(),
            seeds: HashMap::new(),
            redo: None,
            stale: false,
            forgotten: Vec::new(),
            heard_later: Vec::new(),
            slots: Vec::new(),
            wave_dry: None,
            refill: None,
            mix_naming: save.mix_naming,
        });
        let loc = self.state.read().prefs.resolved_locale();
        if !self.relocalize_active_radio(loc) {
            // Preserve the queue's own origin for autoplay and search-started radios;
            // only fill a missing origin or upgrade legacy `"микса «Спокойное»"` waves.
            let normalize_calm_legacy = !save.autoplay
                && matches!(save.wave_preset, Some(v::MixKey::Calm))
                && self.queue.origin == v::QueueOrigin::Mix(v::MixTitle::Mood(nikgob_core::i18n::MoodKind::Calm));
            if !save.autoplay && (self.queue.origin == v::QueueOrigin::None || normalize_calm_legacy) {
                self.queue.set_origin(origin);
            }
        }
        self.play_context = self.recs.radio.run.as_ref().map_or(self.play_context, |r| r.ctx);
    }

    pub(crate) fn set_run_mix_naming(&mut self, naming: ActiveMixNaming) {
        if let Some(run) = self.recs.radio.run.as_mut() {
            run.mix_naming = Some(naming);
        }
    }

    /// Recomputes dynamic home-mix radio/queue titles (`Daylist`, `Style`)
    /// from the run's persisted locale-neutral naming inputs.
    pub(crate) fn relocalize_active_radio(&mut self, loc: v::Locale) -> bool {
        let Some(mt) = self
            .recs
            .radio
            .run
            .as_ref()
            .and_then(|r| r.mix_naming.as_ref())
            .map(|naming| naming.title_in(loc))
        else {
            return false;
        };
        let qgen = self.queue.generation();
        let Some(run) = self.recs.radio.run.as_mut() else {
            return false;
        };
        run.title = v::RadioTitle::Mix(mt.clone());
        if run.queue_gen == Some(qgen) {
            self.queue.set_origin(v::QueueOrigin::Mix(mt));
        }
        true
    }
}

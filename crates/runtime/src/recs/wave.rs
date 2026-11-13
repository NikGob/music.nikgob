//! «Моя волна» in the runtime: the
//! radio of [`RadioSeed::Wave`] the home's card, the header's chip and the
//! cards of «Миксы для тебя» start; its filters from the queue's header;
//! «↻ Другой вайб» from the player.
//!
//! * The wave plays as any radio (the short live buffer, the tail picked
//!   again after skips, restored after a restart), but its picks come from
//!   [`nikgob_recs::wave`] over the open pool ([`super::open::open_pool`]),
//!   which is built on the radio's thread once for a snapshot and kept
//!   ([`WavePool`]): a chip changed or «Другой вайб» rebuilds the tail from
//!   the kept pool in milliseconds.
//! * Its novelty is its own («Новизна»: the last one chosen is kept in the
//!   prefs); «Новое в радио» does not apply. Under «Только скачанное» the
//!   pool is empty and the library plays alone.
//! * The scenes the header offers are the user's: the taste profiles as
//!   «Смешать с…» names them and the families of genres the library has
//!   enough songs of, the largest first ([`scenes`]).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Weak};

use nikgob_core::model::{EventContext, TrackId};
use nikgob_recs::open_world::{Character, OpenCandidate, Scene, WaveContext, WaveFilter};
use nikgob_recs::wave::{MIN_ARM_TRACKS, NOVELTY_BALANCE, NOVELTY_FAMILIAR, NOVELTY_MORE, NOVELTY_ONLY_NEW, WaveSeed};
use nikgob_recs::{Pick, RadioSeed, RadioState as Engine};
use nikgob_view as v;

use super::Snapshot;
use crate::Worker;

/// The wave's name in the queue.
pub(crate) const TITLE: &str = "Моя волна";
/// The scenes the header offers at most.
const SCENES_MAX: usize = 8;

/// The share of new music of a novelty.
pub(crate) fn novelty_share(n: v::WaveNovelty) -> f32 {
    match n {
        v::WaveNovelty::Familiar => NOVELTY_FAMILIAR,
        v::WaveNovelty::Balance => NOVELTY_BALANCE,
        v::WaveNovelty::More => NOVELTY_MORE,
        v::WaveNovelty::OnlyNew => NOVELTY_ONLY_NEW,
    }
}

/// The novelty nearest a share.
pub(crate) fn novelty_of(share: f32) -> v::WaveNovelty {
    *v::WaveNovelty::ALL
        .iter()
        .min_by(|a, b| (novelty_share(**a) - share).abs().total_cmp(&(novelty_share(**b) - share).abs()))
        .unwrap_or(&v::WaveNovelty::Balance)
}

fn character_of(c: v::WaveCharacter) -> Character {
    match c {
        v::WaveCharacter::Any => Character::Any,
        v::WaveCharacter::Lively => Character::Lively,
        v::WaveCharacter::Calm => Character::Calm,
        v::WaveCharacter::Dark => Character::Dark,
        v::WaveCharacter::Background => Character::Background,
    }
}

fn view_character(c: Character) -> v::WaveCharacter {
    match c {
        Character::Any => v::WaveCharacter::Any,
        Character::Lively => v::WaveCharacter::Lively,
        Character::Calm => v::WaveCharacter::Calm,
        Character::Dark => v::WaveCharacter::Dark,
        Character::Background => v::WaveCharacter::Background,
    }
}

fn scene_of(s: v::WaveScene) -> Scene {
    match s {
        v::WaveScene::All => Scene::All,
        v::WaveScene::Profile(k) => Scene::Profile(k),
        v::WaveScene::Kind(c) => Scene::Kind(nikgob_recs::mixes::lane_of_code(c)),
        v::WaveScene::Genre(f) => Scene::Genre(f),
    }
}

fn view_scene(s: Scene) -> v::WaveScene {
    match s {
        Scene::All => v::WaveScene::All,
        Scene::Profile(k) => v::WaveScene::Profile(k),
        Scene::Kind(l) => v::WaveScene::Kind(nikgob_recs::mixes::lane_code(l)),
        Scene::Genre(f) => v::WaveScene::Genre(f),
    }
}

/// The engine's filter of the view's.
pub(crate) fn filter_of(f: v::WaveFilter) -> WaveFilter {
    WaveFilter { novelty: novelty_share(f.novelty), character: character_of(f.character), scene: scene_of(f.scene) }
}

/// The view's filter of the engine's.
pub(crate) fn view_filter(f: &WaveFilter) -> v::WaveFilter {
    v::WaveFilter { novelty: novelty_of(f.novelty), character: view_character(f.character), scene: view_scene(f.scene) }
}

/// The wave's open pool as built for a snapshot ([`super::open::open_pool`]):
/// kept while the snapshot and the discoveries are the same.
#[derive(Clone)]
pub(crate) struct WavePool {
    snap: Weak<Snapshot>,
    discoveries: usize,
    pub cands: Arc<Vec<OpenCandidate>>,
}

impl WavePool {
    pub(crate) fn new(snap: &Arc<Snapshot>, discoveries: usize, cands: Vec<OpenCandidate>) -> Self {
        Self { snap: Arc::downgrade(snap), discoveries, cands: Arc::new(cands) }
    }

    /// Built for `snap` with `discoveries` kept.
    pub(crate) fn holds(&self, snap: &Arc<Snapshot>, discoveries: usize) -> bool {
        self.discoveries == discoveries && Weak::ptr_eq(&self.snap, &Arc::downgrade(snap))
    }
}

/// The next `n` of the wave of `state` over `snap` and the open `pool`
/// (on the radio's thread). `local_only`: «Только скачанное» as the prefs
/// say now.
pub(crate) fn extend(state: &mut Engine, snap: &Snapshot, pool: &[OpenCandidate], local_only: bool, n: usize, seed: u64) -> Vec<Pick> {
    let ok = |i: usize| {
        let t = &snap.catalog.tracks()[i];
        (t.local || !local_only) && !snap.model.taste.blocks(t)
    };
    let background = |i: usize| snap.background.get(i).copied().unwrap_or(false);
    let artist_tracks = |t: &nikgob_core::streaming::StreamTrack| super::lanes::artist_tracks(snap, t);
    let genre = |i: usize| snap.model.main_artist_at(i).and_then(|a| snap.genres.get(&a).copied());
    let ctx = WaveContext { model: &snap.model, ok: &ok, genre: Some(&genre), background: &background, artist_tracks: &artist_tracks, index: Some(&snap.index) };
    let pool: &[OpenCandidate] = if local_only { &[] } else { pool };
    state.extend_wave(&ctx, pool, n, seed)
}

/// The user's scenes over `snap` (see the module docs): the taste profiles
/// named as «Смешать с…» names them (`profiles`), with at least a cluster's
/// worth of library tracks, then the families of genres, each name once,
/// the largest first, [`SCENES_MAX`] at most.
pub(crate) fn scenes(snap: &Snapshot, profiles: &[v::BlendProfile]) -> Vec<v::WaveSceneCard> {
    let m = &snap.model;
    let mut by_profile: HashMap<u32, usize> = HashMap::new();
    let mut by_genre: HashMap<u8, usize> = HashMap::new();
    for i in 0..m.len() {
        if let Some(p) = m.profile_at(i).and_then(|j| m.profiles.list.get(j)) {
            *by_profile.entry(p.key).or_default() += 1;
        }
        if let Some(f) = m.main_artist_at(i).and_then(|a| snap.genres.get(&a).copied()) {
            *by_genre.entry(f).or_default() += 1;
        }
    }
    let mut found: Vec<(usize, v::WaveSceneCard)> = Vec::new();
    for p in profiles {
        let n = by_profile.get(&p.key).copied().unwrap_or(0);
        if n >= MIN_ARM_TRACKS {
            found.push((n, v::WaveSceneCard { scene: v::WaveScene::Profile(p.key), title: p.title.clone() }));
        }
    }
    found.sort_by_key(|a| std::cmp::Reverse(a.0));
    let mut genres: Vec<(usize, v::WaveSceneCard)> = by_genre
        .into_iter()
        .filter(|(_, n)| *n >= MIN_ARM_TRACKS)
        .filter_map(|(f, n)| Some((n, v::WaveSceneCard { scene: v::WaveScene::Genre(f), title: nikgob_recs::mix_names::family_title(f)?.to_owned() })))
        .collect();
    genres.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.title.cmp(&b.1.title)));
    found.extend(genres);
    let mut seen: HashSet<String> = HashSet::new();
    found.into_iter().map(|x| x.1).filter(|c| seen.insert(c.title.to_lowercase())).take(SCENES_MAX).collect()
}

/// The name of a scene the list of [`scenes`] may lack.
fn scene_title(snap: &Snapshot, scene: Scene) -> Option<String> {
    use nikgob_recs::profiles::{Kind, LaneKey};
    match scene {
        Scene::All => None,
        Scene::Profile(k) => snap.model.profiles.list.iter().find(|p| p.key == k).map(|p| p.kind.title()),
        Scene::Kind(LaneKey::Instrumental) => Some(Kind::Instrumental.title()),
        Scene::Kind(LaneKey::Sung(l)) => Some(Kind::Sung(l).title()),
        Scene::Kind(LaneKey::Unknown) => Some(Kind::Songs.title()),
        Scene::Genre(f) => nikgob_recs::mix_names::family_title(f).map(str::to_owned),
    }
}

/// The taste profile most of `tracks` belong to (its key).
fn profile_of(snap: &Snapshot, tracks: &[TrackId]) -> Option<u32> {
    let mut n: HashMap<u32, usize> = HashMap::new();
    for t in tracks {
        if let Some(p) = snap.model.position(*t).and_then(|i| snap.model.profile_at(i)).and_then(|j| snap.model.profiles.list.get(j)) {
            *n.entry(p.key).or_default() += 1;
        }
    }
    n.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0))).map(|x| x.0)
}

/// The name a wave plays under: its card's mix (`card`), else «Моя волна».
fn wave_title(card: Option<String>) -> String {
    card.filter(|t| !t.trim().is_empty()).unwrap_or_else(|| TITLE.to_owned())
}

impl Worker {
    /// `Command::PlayWave`: «Моя волна» plain, or as the card `preset`
    /// has it; a card that is no wave plays its list.
    pub(crate) fn play_wave(&mut self, preset: Option<v::MixKey>) {
        let novelty = novelty_share(self.state.read().prefs.wave_novelty);
        let seed = match preset {
            None => Some(WaveSeed::plain(novelty)),
            // A named mix of the day: its cluster's taste profile.
            Some(k @ v::MixKey::Daily(n)) if n > 0 => {
                let snap = self.snapshot();
                let tracks = self.recs.home.as_ref().and_then(|h| h.lanes.iter().find(|l| l.key == k)).map(|l| l.tracks.clone()).unwrap_or_default();
                let mut s = WaveSeed::plain(novelty);
                if let Some(p) = snap.as_ref().and_then(|s| profile_of(s, &tracks)) {
                    s.filter.scene = Scene::Profile(p);
                }
                Some(s)
            }
            Some(k) => super::lanes::engine_key(k).and_then(|k| nikgob_recs::wave::preset_of(k, novelty)),
        };
        let Some(seed) = seed else {
            if let Some(k) = preset {
                self.play_mix(v::MixId::Lane(k));
            }
            return;
        };
        log::info!("wave: starts {:?}", seed);
        // A card's wave is named after its mix: «Играет из микса
        // «Хип-хоп микс»», the queue's header, the card that plays. What
        // the history keeps of it is the seed's (a wave) and the section.
        let title = wave_title(preset.and_then(|k| self.recs.home.as_ref()?.lanes.iter().find(|l| l.key == k).map(|l| l.title.clone())));
        self.start_radio(RadioSeed::Wave(seed), title, EventContext::HomeSection, None);
        let id = self.recs.radio.run_id();
        self.recs.radio.wave_preset = id.map(|id| (id, preset));
    }

    /// `Command::SetWave`: the playing wave plays `filter` from its next
    /// tracks; the novelty is kept for the next wave.
    pub(crate) fn set_wave(&mut self, filter: v::WaveFilter) {
        if self.state.read().prefs.wave_novelty != filter.novelty {
            self.publish(|s| s.prefs.wave_novelty = filter.novelty);
            self.save_prefs();
        }
        let Some(old) = self.wave_seed() else { return };
        let f = filter_of(filter);
        // A blend's profiles and a card's part of the day hold while its
        // scene does.
        let only = if f.scene == old.filter.scene { old.only.clone() } else { Vec::new() };
        let seed = WaveSeed { filter: f, only, part: old.part };
        if seed == old {
            return;
        }
        log::info!("wave: the filter is now {:?}", seed.filter);
        self.radio_wave_changed(Some(seed));
    }

    /// `Command::OtherVibe`: «↻ Другой вайб».
    pub(crate) fn other_vibe(&mut self) {
        if self.wave_seed().is_none() {
            return;
        }
        log::info!("wave: another vibe");
        self.radio_wave_changed(None);
    }

    /// The playing wave's filter.
    pub(crate) fn wave_seed(&mut self) -> Option<WaveSeed> {
        if !self.radio_alive() {
            return None;
        }
        self.recs.radio.engine().and_then(|e| e.wave_seed().cloned())
    }

    /// What the queue's header shows of the playing wave.
    pub(crate) fn wave_view(&mut self) -> Option<v::WaveView> {
        let seed = self.wave_seed()?;
        let snap = self.snapshot();
        // Found on the wave's thread with its picks (of this snapshot or,
        // until its next picks, of the one before).
        let mut scenes = self.recs.radio.scenes.as_ref().map(|(_, list)| list.clone()).unwrap_or_default();
        // A card's scene the list lacks (a kind of song, a small family)
        // is offered too, first.
        let current = view_scene(seed.filter.scene);
        if current != v::WaveScene::All
            && !scenes.iter().any(|c| c.scene == current)
            && let Some(title) = snap.as_ref().and_then(|s| scene_title(s, seed.filter.scene))
        {
            scenes.insert(0, v::WaveSceneCard { scene: current, title });
        }
        let run = self.recs.radio.run_id();
        let preset = self.recs.radio.wave_preset.filter(|(id, _)| Some(*id) == run).and_then(|(_, k)| k);
        Some(v::WaveView { filter: view_filter(&seed.filter), scenes, preset })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wave started from a mix's card plays under the mix's name, so the
    /// queue says «из микса «Хип-хоп микс»», not «Моя волна»; the plain
    /// wave (or a card without a name) is «Моя волна».
    #[test]
    fn a_cards_wave_is_named_after_its_mix() {
        assert_eq!(wave_title(Some("Хип-хоп микс".into())), "Хип-хоп микс");
        assert_eq!(super::super::radio::context_of(&wave_title(Some("Хип-хоп микс".into()))), "микса «Хип-хоп микс»");
        assert_eq!(wave_title(None), TITLE);
        assert_eq!(wave_title(Some(" ".into())), TITLE);
    }

    /// The view's filters and the engine's go both ways.
    #[test]
    fn filters_go_both_ways() {
        for novelty in v::WaveNovelty::ALL {
            for character in v::WaveCharacter::ALL {
                for scene in [v::WaveScene::All, v::WaveScene::Profile(7), v::WaveScene::Kind(0), v::WaveScene::Kind(2), v::WaveScene::Genre(3)] {
                    let f = v::WaveFilter { novelty, character, scene };
                    assert_eq!(view_filter(&filter_of(f)), f);
                }
            }
        }
        assert_eq!(novelty_of(0.0), v::WaveNovelty::Familiar);
        assert_eq!(novelty_of(1.0), v::WaveNovelty::OnlyNew);
        assert_eq!(novelty_of(0.25), v::WaveNovelty::Balance);
    }
}

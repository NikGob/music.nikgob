use std::collections::{HashMap, HashSet};

use nikgob_core::i18n::home_recs::{
    BlendWord, blend_adjectives_in, blend_songs_noun_in, blend_word_in, home_recs_in, join_adjectives_in,
};
use nikgob_core::i18n::{BackgroundVariant, Locale, MixTitle, day_part_title_in};

use crate::profiles::{Kind, LaneKey, lang_adjective_in};
use crate::songtype::Lang;

use super::*;

impl<'a> Builder<'a> {
    /// The main artists of `tracks`, by how many songs they have there
    /// (then by their first place).
    pub(super) fn artists(&self, tracks: &[usize]) -> Vec<ArtistId> {
        let mut count: HashMap<ArtistId, (usize, usize)> = HashMap::new();
        for (k, &i) in tracks.iter().enumerate() {
            if let Some(a) = self.recs.info(i).artists.first() {
                count.entry(*a).or_insert((0, k)).0 += 1;
            }
        }
        let mut ranked: Vec<(ArtistId, (usize, usize))> = count.into_iter().collect();
        ranked.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.1.1.cmp(&b.1.1)));
        ranked.into_iter().map(|x| x.0).take(12).collect()
    }

    /// What the mix of `key` with `tracks` is called (see the module docs).
    pub fn title(&self, key: MixKey, tracks: &[usize]) -> String {
        self.title_in(key, tracks, self.settings.locale)
    }

    /// What the mix of `key` with `tracks` is called in `loc` (see the module docs).
    pub fn title_in(&self, key: MixKey, tracks: &[usize], loc: Locale) -> String {
        let name = |k: u32| -> Option<String> {
            let j = self.profile_index(k)?;
            Some(self.model.profiles.name_in(j, self.settings.names, loc, &|_| None).0)
        };
        match key {
            MixKey::Profile(k) => name(k).unwrap_or_default(),
            MixKey::Everything => MixTitle::Everything.title_in(loc).into_owned(),
            MixKey::Calm | MixKey::Lively => {
                side_title_in(key == MixKey::Calm, tracks.iter().map(|&i| LaneKey::of(self.model.song_type_at(i))), loc)
            }
            MixKey::Background { card, cards, .. } => match (cards, card) {
                (2, 0) | (3, 0) => BackgroundVariant::Quiet.title_in(loc).into(),
                (2, _) | (3, 2) => BackgroundVariant::Lively.title_in(loc).into(),
                _ => BackgroundVariant::Plain.title_in(loc).into(),
            },
            MixKey::Blend(keys) => {
                let names: Vec<String> = keys.iter().filter(|k| **k != 0).filter_map(|k| name(*k)).collect();
                if self.settings.names == NameStyle::Kind {
                    blend_title_in(names, loc)
                } else {
                    names.join(" + ")
                }
            }
            MixKey::Kind(c) => lane_kind(lane_of_code(c)).title_in(loc),
            // «Любимые» is the playlist of every liked song; this is a mix
            // of them (the row does not show it).
            MixKey::Liked => MixTitle::Liked.title_in(loc).into_owned(),
            MixKey::Part(p) => part_title_in(p, loc).into(),
            MixKey::Daily(0) => home_recs_in(loc).daily_mix.into(),
            MixKey::Daily(_) => self.cluster_title_in(tracks, loc),
            MixKey::Genre(f) => crate::mix_names::family_title_in(f, loc).unwrap_or_default().into(),
        }
    }

    /// Shorter forms of a blend's name by its profiles' kinds (see
    /// [`blend_short_in`]); none for the others, or by the artists.
    pub fn short_titles(&self, key: MixKey) -> Vec<String> {
        self.short_titles_in(key, self.settings.locale)
    }

    /// Shorter forms of a blend's name in `loc`.
    pub fn short_titles_in(&self, key: MixKey, loc: Locale) -> Vec<String> {
        let MixKey::Blend(keys) = key else { return Vec::new() };
        if self.settings.names != NameStyle::Kind {
            return Vec::new();
        }
        let kinds: Option<Vec<Kind>> = keys.iter().filter(|k| **k != 0).map(|k| Some(self.model.profiles.list[self.profile_index(*k)?].kind)).collect();
        kinds.map_or_else(Vec::new, |k| blend_short_in(&k, loc))
    }

    /// The names that need the artists' (`artist` names one): a profile's
    /// by its artists, a blend's of them; then two cards of one name tell
    /// themselves apart by their main artist.
    pub fn named(&self, mixes: &mut [Mix], artist: &dyn Fn(ArtistId) -> Option<String>) {
        self.named_in(mixes, self.settings.locale, artist);
    }

    /// [`Self::named`] rendering titles in `loc`.
    pub fn named_in(&self, mixes: &mut [Mix], loc: Locale, artist: &dyn Fn(ArtistId) -> Option<String>) {
        let name = |k: u32| -> Option<String> {
            let j = self.profile_index(k)?;
            Some(self.model.profiles.name_in(j, self.settings.names, loc, artist).0)
        };
        for m in mixes.iter_mut() {
            match m.key {
                MixKey::Profile(k) => m.title = name(k).unwrap_or_default(),
                MixKey::Blend(keys) => {
                    let parts: Vec<String> = keys
                        .iter()
                        .filter(|k| **k != 0)
                        .filter_map(|k| match self.settings.names {
                            NameStyle::Kind => name(*k),
                            NameStyle::Artists => {
                                let j = self.profile_index(*k)?;
                                self.model.profiles.list[j].stats.artists.first().and_then(|a| artist(*a))
                            }
                        })
                        .collect();
                    m.title = if self.settings.names == NameStyle::Kind { blend_title_in(parts, loc) } else { parts.join(" + ") };
                }
                _ => {}
            }
        }
        for k in 1..mixes.len() {
            if mixes[..k].iter().any(|m| m.title == mixes[k].title) {
                let who = mixes[k].artists.iter().filter_map(|a| artist(*a)).find(|n| !mixes[..k].iter().any(|m| m.title.ends_with(n.as_str())));
                if let Some(who) = who {
                    mixes[k].title = format!("{} · {who}", mixes[k].title);
                }
            }
        }
    }

    /// How well `mix` fits now (see the module docs).
    pub fn context(&self, mix: &Mix) -> f32 {
        let part = self.part.index();
        let (mut here, mut all) = (0u32, 0u32);
        for &i in &mix.tracks {
            let p = &self.model.tracks[i].part;
            here += u32::from(p[part]);
            all += p.iter().map(|x| u32::from(*x)).sum::<u32>();
        }
        let expect = all as f32 * self.model.part_share(part);
        let mut s = ((here as f32 + 1.0) / (expect + 1.0)).ln();
        // Its songs' fit to this part of this weekday ([`crate::context`]).
        if !mix.tracks.is_empty() {
            let slot = self.model.slot().with_part(self.part);
            let fit: f32 = mix.tracks.iter().map(|&i| self.model.time_bonus_at(i, slot) + self.model.sound_fit_at(i, slot)).sum();
            s += MOMENT_FIT * fit / mix.tracks.len() as f32;
        }
        let worlds: HashSet<u64> = mix.tracks.iter().filter_map(|&i| self.recs.info(i).world).collect();
        if !worlds.is_empty() {
            let now = self.model.now();
            let hot: f32 = worlds.iter().filter_map(|w| self.model.worlds.get(w)).map(|w| w.hot(now)).sum::<f32>() / worlds.len() as f32;
            s += 0.8 * hot.ln_1p();
        }
        if self.live.profile.is_some_and(|p| mix.profiles.contains(&p)) {
            s += 0.5;
        }
        if matches!(mix.key, MixKey::Background { .. }) {
            s += 0.8 * self.live.p_bg;
        }
        // Before anything is heard: at night the calm and the wordless first.
        if !self.history && self.part == DayPart::Night {
            let quiet = mix.key == MixKey::Calm || mix.key == MixKey::Kind(lane_code(LaneKey::Instrumental));
            s += if quiet { 0.5 } else { 0.0 };
        }
        s
    }
}

/// The name of the mix of the part of the day `p` ([`DayPart::index`]) in `loc`:
/// the moment itself, never taken for «Микс дня» / «Daily Mix».
pub fn part_title_in(p: u8, loc: Locale) -> &'static str {
    day_part_title_in(loc, p)
}

/// The row of «Миксы для тебя» (beside the «Любимые» tile before it and
/// «На повторе» after): «Микс дня», the mix of the part of the day, the
/// genre mixes, the named mixes of the day, the moments — of `daily` (as
/// [`Builder::daily`] and [`Builder::name_daily`] left it), `part` and
/// `moments` (the blends first, as [`Builder::lineup`] has them), each
/// group in its own order, [`ROW_MIXES`] at most. The named mixes only
/// fill it: [`ROW_NAMED`] of them, [`ROW_NAMED_FEW`] beside [`GENRE_MANY`]
/// genre mixes.
///
/// No two cards alike, the first winning — a card is left out when one
/// already in the row
///
/// * has its name;
/// * shares more than [`OVERLAP`] of the songs of the shorter of the two;
/// * is the mix of the genre it is of itself ([`Mix::genre`]: a named
///   mix, a profile's or a kind's with [`GENRE_SAME`] of its songs of that
///   genre — «Саундтреки без слов» beside «Микс саундтреков»);
/// * is of its kind of song, one of the two a side of everything loved
///   and the other not ([`Mix::kind`]: «Русские песни погромче» beside
///   «Русские песни»; a card of no kind stands for its name).
///
/// A blend is the user's own and always stays.
pub fn row(daily: Vec<Mix>, part: Option<Mix>, moments: Vec<Mix>) -> Vec<Mix> {
    row_upto(daily, part, moments, 0)
}

/// [`row`], then up to `more` of the cards it left out for want of room
/// (past [`ROW_MIXES`], the named mixes past their share), in their
/// order, none alike: the row scrolled to its end asks for them, and the
/// cards it shows stay where they are.
pub fn row_upto(daily: Vec<Mix>, part: Option<Mix>, moments: Vec<Mix>, more: usize) -> Vec<Mix> {
    let side = |m: &Mix| matches!(m.key, MixKey::Calm | MixKey::Lively);
    let name = |m: &Mix| m.title.trim().to_lowercase();
    let alike = |a: &Mix, b: &Mix| {
        if name(a) == name(b) {
            return true;
        }
        let set: HashSet<usize> = a.tracks.iter().copied().collect();
        let shared = b.tracks.iter().filter(|i| set.contains(i)).count();
        if shared as f32 > OVERLAP * a.tracks.len().min(b.tracks.len()) as f32 {
            return true;
        }
        let of_genre = matches!(b.key, MixKey::Profile(_) | MixKey::Kind(_)) || b.key.is_cluster();
        if a.key.is_genre() && of_genre && b.genre.is_some() && a.genre == b.genre {
            return true;
        }
        a.kind.is_some() && a.kind == b.kind && side(a) != side(b)
    };
    let (mut day, mut genres, mut named) = (Vec::new(), Vec::new(), Vec::new());
    for m in daily {
        match m.key {
            MixKey::Genre(_) => genres.push(m),
            k if k.is_cluster() => named.push(m),
            _ => day.push(m),
        }
    }
    let mut out: Vec<Mix> = Vec::new();
    let mut fill = 0;
    // What the row leaves out for want of room: the cards asked for past
    // it (`more`), in their order.
    let mut left: Vec<Mix> = Vec::new();
    // The mix of this part of the day right after «Микс дня»: on the
    // first screen beside «Моя волна».
    for m in day.into_iter().chain(part).chain(genres).chain(named).chain(moments) {
        if out.len() >= ROW_MIXES {
            left.push(m);
            continue;
        }
        if m.key.is_cluster() {
            let many = out.iter().filter(|c| c.key.is_genre()).count() >= GENRE_MANY;
            let room = if many { ROW_NAMED_FEW } else { ROW_NAMED };
            if fill >= room {
                left.push(m);
                continue;
            }
        }
        if !matches!(m.key, MixKey::Blend(_)) && out.iter().any(|c| alike(c, &m)) {
            continue;
        }
        fill += usize::from(m.key.is_cluster());
        out.push(m);
    }
    // The row as it was (it may be short of ten when the named mixes are
    // past their share), then at most `more` of what it left out.
    let end = out.len() + more;
    for m in left {
        if out.len() >= end {
            break;
        }
        if !matches!(m.key, MixKey::Blend(_)) && out.iter().any(|c| alike(c, &m)) {
            continue;
        }
        out.push(m);
    }
    out
}

/// The language a side keeps to: the one [`LANG_HOLD`] of `lanes` are
/// sung in, when its songs have a name.
pub(super) fn side_lang(lanes: &[LaneKey]) -> Option<Lang> {
    let mut langs: HashMap<Lang, usize> = HashMap::new();
    for l in lanes {
        if let LaneKey::Sung(x) = l {
            *langs.entry(*x).or_default() += 1;
        }
    }
    let (l, k) = langs.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))?;
    (k as f32 >= LANG_HOLD * lanes.len().max(1) as f32 && lang_adjective_in(l, Locale::Ru).is_some()).then_some(l)
}

/// Joins language adjectives in `loc`.
fn join_adjectives(adjectives: &[&str], loc: Locale) -> String {
    join_adjectives_in(loc, adjectives)
}

/// A blend's name in `loc`: its profiles' names joined by «+», the songs of
/// one word said once («Русские и японские песни + Саундтреки без слов» /
/// «Russian & Japanese Songs + Instrumental Soundtracks»).
pub(super) fn blend_title_in(names: Vec<String>, loc: Locale) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut kinds: Vec<(String, Vec<String>)> = Vec::new();
    for n in &names {
        // «Русские песни» / «Russian Songs»: one word before the last.
        let words: Vec<&str> = n.split(' ').collect();
        if words.len() == 2 && n.chars().next().is_some_and(char::is_uppercase) {
            match kinds.iter_mut().find(|k| k.0 == words[1]) {
                Some(k) => k.1.push(words[0].to_owned()),
                None => kinds.push((words[1].to_owned(), vec![words[0].to_owned()])),
            }
        }
    }
    let mut said: Vec<&str> = Vec::new();
    for n in &names {
        let words: Vec<&str> = n.split(' ').collect();
        let kind = kinds.iter().find(|k| words.len() == 2 && k.0 == words[1] && k.1.len() > 1);
        match kind {
            Some((noun, adjectives)) if !said.contains(&noun.as_str()) => {
                said.push(noun);
                let refs: Vec<&str> = adjectives.iter().map(String::as_str).collect();
                let combined = blend_adjectives_in(loc, &refs);
                parts.push(format!("{combined} {noun}"));
            }
            Some(_) => {}
            None => parts.push(n.clone()),
        }
    }
    parts.join(" + ")
}

/// Shorter forms of a blend's name in `loc`, the longest first, by its
/// profiles' kinds: «Русские и японские песни + без слов», then «Русские и
/// японские + без слов» (or English equivalents). None when two of its
/// profiles are of one kind (their names tell them apart by more than the kind).
pub(super) fn blend_short_in(kinds: &[Kind], loc: Locale) -> Vec<String> {
    let mut seen: Vec<Kind> = Vec::new();
    for k in kinds {
        if seen.contains(k) {
            return Vec::new();
        }
        seen.push(*k);
    }
    let sung: Vec<&str> = kinds
        .iter()
        .filter_map(|k| match k {
            Kind::Sung(l) => lang_adjective_in(*l, loc),
            _ => None,
        })
        .collect();
    let rest: Vec<&str> = kinds
        .iter()
        .filter_map(|k| match k {
            Kind::Sung(l) if lang_adjective_in(*l, loc).is_some() => None,
            Kind::Instrumental | Kind::InstrumentalSoundtracks => Some(blend_word_in(loc, BlendWord::Wordless)),
            Kind::Soundtracks => Some(blend_word_in(loc, BlendWord::Soundtracks)),
            Kind::Songs | Kind::Sung(_) => Some(blend_word_in(loc, BlendWord::Songs)),
            Kind::Calm => Some(blend_word_in(loc, BlendWord::Calm)),
            Kind::Lively => Some(blend_word_in(loc, BlendWord::Upbeat)),
        })
        .collect();
    let join = |head: String| -> String {
        let mut parts = vec![head];
        parts.extend(rest.iter().map(|r| (*r).to_owned()));
        parts.join(" + ")
    };
    let mut out = Vec::new();
    if sung.is_empty() {
        let mut first = rest.first().map(|r| (*r).to_owned()).unwrap_or_default();
        if let Some(c) = first.chars().next() {
            first = c.to_uppercase().collect::<String>() + &first[c.len_utf8()..];
        }
        out.push(std::iter::once(first).chain(rest.iter().skip(1).map(|r| (*r).to_owned())).collect::<Vec<_>>().join(" + "));
    } else {
        let adj = join_adjectives(&sung, loc);
        let songs_noun = blend_songs_noun_in(loc);
        out.push(join(format!("{adj} {songs_noun}")));
        if !rest.is_empty() {
            out.push(join(adj));
        }
    }
    out
}

/// Locale-neutral composition flavor of a Calm or Lively side mix.
pub type SideFlavor = nikgob_core::i18n::SideMixFlavor;

/// Computes the [`SideFlavor`] of raw track [`crate::songtype::SongType`]s.
pub fn side_flavor_of_types(types: impl Iterator<Item = crate::songtype::SongType>) -> SideFlavor {
    side_flavor(types.map(|t| LaneKey::of(&t)))
}

fn side_flavor(lanes: impl Iterator<Item = LaneKey>) -> SideFlavor {
    let lanes: Vec<LaneKey> = lanes.collect();
    let n = lanes.len().max(1) as f32;
    let inst = lanes.iter().filter(|l| **l == LaneKey::Instrumental).count() as f32 / n;
    let sung = lanes.iter().filter(|l| matches!(l, LaneKey::Sung(_))).count() as f32 / n;
    if let Some(l) = side_lang(&lanes) {
        match l {
            Lang::Russian => SideFlavor::Russian,
            Lang::Japanese => SideFlavor::Japanese,
            Lang::Korean => SideFlavor::Korean,
            Lang::English => SideFlavor::English,
            Lang::Spanish => SideFlavor::Spanish,
            Lang::French => SideFlavor::French,
            Lang::German => SideFlavor::German,
            Lang::Other => SideFlavor::OtherLang,
        }
    } else if inst >= KIND_SHARE {
        SideFlavor::Instrumental
    } else if sung >= KIND_SHARE {
        SideFlavor::Songs
    } else {
        SideFlavor::Plain
    }
}

/// Renders a Calm or Lively side mix title in `loc` from its [`SideFlavor`].
pub fn side_title_for_flavor(calm: bool, flavor: SideFlavor, loc: Locale) -> String {
    flavor.title_in(calm, loc)
}

/// The name of a side in `loc` by what it holds: of one language at
/// [`LANG_HOLD`], without words or of songs at [`KIND_SHARE`].
pub(super) fn side_title_in(calm: bool, lanes: impl Iterator<Item = LaneKey>, loc: Locale) -> String {
    side_title_for_flavor(calm, side_flavor(lanes), loc)
}

/// The typed [`Kind`] of a song lane before there are profiles.
pub(super) const fn lane_kind(l: LaneKey) -> Kind {
    match l {
        LaneKey::Instrumental => Kind::Instrumental,
        LaneKey::Sung(x) => Kind::Sung(x),
        LaneKey::Unknown => Kind::Songs,
    }
}


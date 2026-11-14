//! The 400 Discogs styles of Discogs-EffNet's other head, and what they
//! say together.
//!
//! The model gives a sigmoid per style on every patch ([`STYLES`], its own
//! label list, «Genre---Style», in the order of its output); the runtime
//! averages them over the windows of a track and this module keeps the
//! leading ones ([`Styles`]). Nothing here says what a style means: the
//! only structure read off the list is its own, the genre before «---»
//! ([`genre_index`]). What styles say of words is learnt from the
//! library's own labels ([`StyleProbe`]), and which family of genres they
//! are of from the artists the library's tags tell ([`StyleFamilies`]).
//! Nothing here knows a user, an artist or a language.

use std::collections::HashMap;

/// Outputs of the style head.
pub const STYLE_COUNT: usize = 400;
/// Leading styles kept of a track.
pub const TOP: usize = 4;
/// The styles' probabilities are scaled by this before the regression of
/// [`StyleProbe`] (its steps suit features of about unit size).
const PROBE_SCALE: f32 = 4.0;

/// The model's labels, «Genre---Style», in the order of its output
/// (`discogs-effnet-bsdynamic-1.json`, `classes`).
#[rustfmt::skip]
pub const STYLES: [&str; STYLE_COUNT] = [
    "Blues---Boogie Woogie", "Blues---Chicago Blues", "Blues---Country Blues", "Blues---Delta Blues",
    "Blues---Electric Blues", "Blues---Harmonica Blues", "Blues---Jump Blues", "Blues---Louisiana Blues",
    "Blues---Modern Electric Blues", "Blues---Piano Blues", "Blues---Rhythm & Blues", "Blues---Texas Blues",
    "Brass & Military---Brass Band", "Brass & Military---Marches", "Brass & Military---Military",
    "Children's---Educational", "Children's---Nursery Rhymes", "Children's---Story", "Classical---Baroque",
    "Classical---Choral", "Classical---Classical", "Classical---Contemporary", "Classical---Impressionist",
    "Classical---Medieval", "Classical---Modern", "Classical---Neo-Classical", "Classical---Neo-Romantic",
    "Classical---Opera", "Classical---Post-Modern", "Classical---Renaissance", "Classical---Romantic",
    "Electronic---Abstract", "Electronic---Acid", "Electronic---Acid House", "Electronic---Acid Jazz",
    "Electronic---Ambient", "Electronic---Bassline", "Electronic---Beatdown", "Electronic---Berlin-School",
    "Electronic---Big Beat", "Electronic---Bleep", "Electronic---Breakbeat", "Electronic---Breakcore",
    "Electronic---Breaks", "Electronic---Broken Beat", "Electronic---Chillwave", "Electronic---Chiptune",
    "Electronic---Dance-pop", "Electronic---Dark Ambient", "Electronic---Darkwave", "Electronic---Deep House",
    "Electronic---Deep Techno", "Electronic---Disco", "Electronic---Disco Polo", "Electronic---Donk",
    "Electronic---Downtempo", "Electronic---Drone", "Electronic---Drum n Bass", "Electronic---Dub",
    "Electronic---Dub Techno", "Electronic---Dubstep", "Electronic---Dungeon Synth", "Electronic---EBM",
    "Electronic---Electro", "Electronic---Electro House", "Electronic---Electroclash", "Electronic---Euro House",
    "Electronic---Euro-Disco", "Electronic---Eurobeat", "Electronic---Eurodance", "Electronic---Experimental",
    "Electronic---Freestyle", "Electronic---Future Jazz", "Electronic---Gabber", "Electronic---Garage House",
    "Electronic---Ghetto", "Electronic---Ghetto House", "Electronic---Glitch", "Electronic---Goa Trance",
    "Electronic---Grime", "Electronic---Halftime", "Electronic---Hands Up", "Electronic---Happy Hardcore",
    "Electronic---Hard House", "Electronic---Hard Techno", "Electronic---Hard Trance", "Electronic---Hardcore",
    "Electronic---Hardstyle", "Electronic---Hi NRG", "Electronic---Hip Hop", "Electronic---Hip-House",
    "Electronic---House", "Electronic---IDM", "Electronic---Illbient", "Electronic---Industrial",
    "Electronic---Italo House", "Electronic---Italo-Disco", "Electronic---Italodance", "Electronic---Jazzdance",
    "Electronic---Juke", "Electronic---Jumpstyle", "Electronic---Jungle", "Electronic---Latin",
    "Electronic---Leftfield", "Electronic---Makina", "Electronic---Minimal", "Electronic---Minimal Techno",
    "Electronic---Modern Classical", "Electronic---Musique Concrète", "Electronic---Neofolk", "Electronic---New Age",
    "Electronic---New Beat", "Electronic---New Wave", "Electronic---Noise", "Electronic---Nu-Disco",
    "Electronic---Power Electronics", "Electronic---Progressive Breaks", "Electronic---Progressive House",
    "Electronic---Progressive Trance", "Electronic---Psy-Trance", "Electronic---Rhythmic Noise",
    "Electronic---Schranz", "Electronic---Sound Collage", "Electronic---Speed Garage", "Electronic---Speedcore",
    "Electronic---Synth-pop", "Electronic---Synthwave", "Electronic---Tech House", "Electronic---Tech Trance",
    "Electronic---Techno", "Electronic---Trance", "Electronic---Tribal", "Electronic---Tribal House",
    "Electronic---Trip Hop", "Electronic---Tropical House", "Electronic---UK Garage", "Electronic---Vaporwave",
    "Folk, World, & Country---African", "Folk, World, & Country---Bluegrass", "Folk, World, & Country---Cajun",
    "Folk, World, & Country---Canzone Napoletana", "Folk, World, & Country---Catalan Music",
    "Folk, World, & Country---Celtic", "Folk, World, & Country---Country", "Folk, World, & Country---Fado",
    "Folk, World, & Country---Flamenco", "Folk, World, & Country---Folk", "Folk, World, & Country---Gospel",
    "Folk, World, & Country---Highlife", "Folk, World, & Country---Hillbilly", "Folk, World, & Country---Hindustani",
    "Folk, World, & Country---Honky Tonk", "Folk, World, & Country---Indian Classical",
    "Folk, World, & Country---Laïkó", "Folk, World, & Country---Nordic", "Folk, World, & Country---Pacific",
    "Folk, World, & Country---Polka", "Folk, World, & Country---Raï", "Folk, World, & Country---Romani",
    "Folk, World, & Country---Soukous", "Folk, World, & Country---Séga", "Folk, World, & Country---Volksmusik",
    "Folk, World, & Country---Zouk", "Folk, World, & Country---Éntekhno", "Funk / Soul---Afrobeat",
    "Funk / Soul---Boogie", "Funk / Soul---Contemporary R&B", "Funk / Soul---Disco", "Funk / Soul---Free Funk",
    "Funk / Soul---Funk", "Funk / Soul---Gospel", "Funk / Soul---Neo Soul", "Funk / Soul---New Jack Swing",
    "Funk / Soul---P.Funk", "Funk / Soul---Psychedelic", "Funk / Soul---Rhythm & Blues", "Funk / Soul---Soul",
    "Funk / Soul---Swingbeat", "Funk / Soul---UK Street Soul", "Hip Hop---Bass Music", "Hip Hop---Boom Bap",
    "Hip Hop---Bounce", "Hip Hop---Britcore", "Hip Hop---Cloud Rap", "Hip Hop---Conscious", "Hip Hop---Crunk",
    "Hip Hop---Cut-up/DJ", "Hip Hop---DJ Battle Tool", "Hip Hop---Electro", "Hip Hop---G-Funk", "Hip Hop---Gangsta",
    "Hip Hop---Grime", "Hip Hop---Hardcore Hip-Hop", "Hip Hop---Horrorcore", "Hip Hop---Instrumental",
    "Hip Hop---Jazzy Hip-Hop", "Hip Hop---Miami Bass", "Hip Hop---Pop Rap", "Hip Hop---Ragga HipHop",
    "Hip Hop---RnB/Swing", "Hip Hop---Screw", "Hip Hop---Thug Rap", "Hip Hop---Trap", "Hip Hop---Trip Hop",
    "Hip Hop---Turntablism", "Jazz---Afro-Cuban Jazz", "Jazz---Afrobeat", "Jazz---Avant-garde Jazz",
    "Jazz---Big Band", "Jazz---Bop", "Jazz---Bossa Nova", "Jazz---Contemporary Jazz", "Jazz---Cool Jazz",
    "Jazz---Dixieland", "Jazz---Easy Listening", "Jazz---Free Improvisation", "Jazz---Free Jazz", "Jazz---Fusion",
    "Jazz---Gypsy Jazz", "Jazz---Hard Bop", "Jazz---Jazz-Funk", "Jazz---Jazz-Rock", "Jazz---Latin Jazz",
    "Jazz---Modal", "Jazz---Post Bop", "Jazz---Ragtime", "Jazz---Smooth Jazz", "Jazz---Soul-Jazz", "Jazz---Space-Age",
    "Jazz---Swing", "Latin---Afro-Cuban", "Latin---Baião", "Latin---Batucada", "Latin---Beguine", "Latin---Bolero",
    "Latin---Boogaloo", "Latin---Bossanova", "Latin---Cha-Cha", "Latin---Charanga", "Latin---Compas",
    "Latin---Cubano", "Latin---Cumbia", "Latin---Descarga", "Latin---Forró", "Latin---Guaguancó", "Latin---Guajira",
    "Latin---Guaracha", "Latin---MPB", "Latin---Mambo", "Latin---Mariachi", "Latin---Merengue", "Latin---Norteño",
    "Latin---Nueva Cancion", "Latin---Pachanga", "Latin---Porro", "Latin---Ranchera", "Latin---Reggaeton",
    "Latin---Rumba", "Latin---Salsa", "Latin---Samba", "Latin---Son", "Latin---Son Montuno", "Latin---Tango",
    "Latin---Tejano", "Latin---Vallenato", "Non-Music---Audiobook", "Non-Music---Comedy", "Non-Music---Dialogue",
    "Non-Music---Education", "Non-Music---Field Recording", "Non-Music---Interview", "Non-Music---Monolog",
    "Non-Music---Poetry", "Non-Music---Political", "Non-Music---Promotional", "Non-Music---Radioplay",
    "Non-Music---Religious", "Non-Music---Spoken Word", "Pop---Ballad", "Pop---Bollywood", "Pop---Bubblegum",
    "Pop---Chanson", "Pop---City Pop", "Pop---Europop", "Pop---Indie Pop", "Pop---J-pop", "Pop---K-pop",
    "Pop---Kayōkyoku", "Pop---Light Music", "Pop---Music Hall", "Pop---Novelty", "Pop---Parody", "Pop---Schlager",
    "Pop---Vocal", "Reggae---Calypso", "Reggae---Dancehall", "Reggae---Dub", "Reggae---Lovers Rock", "Reggae---Ragga",
    "Reggae---Reggae", "Reggae---Reggae-Pop", "Reggae---Rocksteady", "Reggae---Roots Reggae", "Reggae---Ska",
    "Reggae---Soca", "Rock---AOR", "Rock---Acid Rock", "Rock---Acoustic", "Rock---Alternative Rock",
    "Rock---Arena Rock", "Rock---Art Rock", "Rock---Atmospheric Black Metal", "Rock---Avantgarde", "Rock---Beat",
    "Rock---Black Metal", "Rock---Blues Rock", "Rock---Brit Pop", "Rock---Classic Rock", "Rock---Coldwave",
    "Rock---Country Rock", "Rock---Crust", "Rock---Death Metal", "Rock---Deathcore", "Rock---Deathrock",
    "Rock---Depressive Black Metal", "Rock---Doo Wop", "Rock---Doom Metal", "Rock---Dream Pop", "Rock---Emo",
    "Rock---Ethereal", "Rock---Experimental", "Rock---Folk Metal", "Rock---Folk Rock", "Rock---Funeral Doom Metal",
    "Rock---Funk Metal", "Rock---Garage Rock", "Rock---Glam", "Rock---Goregrind", "Rock---Goth Rock",
    "Rock---Gothic Metal", "Rock---Grindcore", "Rock---Grunge", "Rock---Hard Rock", "Rock---Hardcore",
    "Rock---Heavy Metal", "Rock---Indie Rock", "Rock---Industrial", "Rock---Krautrock", "Rock---Lo-Fi",
    "Rock---Lounge", "Rock---Math Rock", "Rock---Melodic Death Metal", "Rock---Melodic Hardcore", "Rock---Metalcore",
    "Rock---Mod", "Rock---Neofolk", "Rock---New Wave", "Rock---No Wave", "Rock---Noise", "Rock---Noisecore",
    "Rock---Nu Metal", "Rock---Oi", "Rock---Parody", "Rock---Pop Punk", "Rock---Pop Rock", "Rock---Pornogrind",
    "Rock---Post Rock", "Rock---Post-Hardcore", "Rock---Post-Metal", "Rock---Post-Punk", "Rock---Power Metal",
    "Rock---Power Pop", "Rock---Power Violence", "Rock---Prog Rock", "Rock---Progressive Metal",
    "Rock---Psychedelic Rock", "Rock---Psychobilly", "Rock---Pub Rock", "Rock---Punk", "Rock---Rock & Roll",
    "Rock---Rockabilly", "Rock---Shoegaze", "Rock---Ska", "Rock---Sludge Metal", "Rock---Soft Rock",
    "Rock---Southern Rock", "Rock---Space Rock", "Rock---Speed Metal", "Rock---Stoner Rock", "Rock---Surf",
    "Rock---Symphonic Rock", "Rock---Technical Death Metal", "Rock---Thrash", "Rock---Twist", "Rock---Viking Metal",
    "Rock---Yé-Yé", "Stage & Screen---Musical", "Stage & Screen---Score", "Stage & Screen---Soundtrack",
    "Stage & Screen---Theme",
];

/// The genre of a style's label (the part before «---»).
pub fn genre_of(label: &str) -> &str {
    label.split("---").next().unwrap_or(label)
}

/// The style of a label (the part after «---»).
pub fn style_of(label: &str) -> &str {
    label.split("---").nth(1).unwrap_or(label)
}

/// The label of output `i`.
pub fn label(i: usize) -> Option<&'static str> {
    STYLES.get(i).copied()
}

/// The genres of the label list, in the order they first appear in it.
pub fn genres() -> &'static [&'static str] {
    static GENRES: std::sync::OnceLock<Vec<&'static str>> = std::sync::OnceLock::new();
    GENRES.get_or_init(|| {
        let mut out: Vec<&'static str> = Vec::new();
        for l in STYLES {
            let g = genre_of(l);
            if !out.contains(&g) {
                out.push(g);
            }
        }
        out
    })
}

/// The place of output `i`'s genre in [`genres`].
pub fn genre_index(i: usize) -> Option<usize> {
    let g = genre_of(label(i)?);
    genres().iter().position(|x| *x == g)
}

/// A track's leading styles (see the module docs).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Styles {
    /// The leading styles with their mean probability, the most likely
    /// first; `(u16::MAX, 0)` fills a place left empty.
    pub top: [(u16, f32); TOP],
    /// The mass of the leading styles (how sure the model is of any).
    pub sure: f32,
}

impl Styles {
    /// The summary of the mean probabilities of the 400 styles; `None`
    /// for a vector of another length or not numbers.
    pub fn of(mean: &[f32]) -> Option<Self> {
        if mean.len() != STYLE_COUNT || mean.iter().any(|x| !x.is_finite()) {
            return None;
        }
        let mut order: Vec<usize> = (0..STYLE_COUNT).collect();
        order.sort_by(|a, b| mean[*b].total_cmp(&mean[*a]).then(a.cmp(b)));
        let mut top = [(u16::MAX, 0.0); TOP];
        for (slot, &i) in top.iter_mut().zip(&order) {
            *slot = (i as u16, mean[i].clamp(0.0, 1.0));
        }
        Some(Self { top, sure: top.iter().map(|t| t.1).sum::<f32>().min(1.0) })
    }

    /// The leading styles that are there, with their labels.
    pub fn leading(&self) -> impl Iterator<Item = (&'static str, f32)> + '_ {
        self.top.iter().filter_map(|&(i, p)| Some((label(usize::from(i))?, p)))
    }

    /// The features of [`StyleProbe`] and [`StyleFamilies`]: each leading
    /// style and its genre ([`genre_index`], after the 400 styles) with its
    /// probability.
    fn features(&self) -> Vec<(usize, f32)> {
        let mut out: Vec<(usize, f32)> = Vec::with_capacity(2 * TOP);
        for &(i, p) in self.top.iter().filter(|t| t.1 > 0.0) {
            let i = usize::from(i);
            let Some(g) = genre_index(i) else { continue };
            for k in [i, STYLE_COUNT + g] {
                match out.iter_mut().find(|o| o.0 == k) {
                    Some(o) => o.1 += p,
                    None => out.push((k, p)),
                }
            }
        }
        out
    }
}

/// Sung or instrumental by a track's leading styles: a logistic regression
/// on them and their genres ([`Styles::features`]), trained on the
/// library's own labels as [`crate::songtype::EmbedProbe`] is, for tracks
/// whose sketch has styles and no vector is loaded. No style is told
/// voiced or wordless by hand.
#[derive(Clone, Debug, PartialEq)]
pub struct StyleProbe {
    /// The bias first.
    w: Vec<f32>,
}

impl StyleProbe {
    /// Trained on `rows` (styles, sung); `None` with fewer than
    /// [`crate::songtype::MIN_PROBE_LABELS`] of either kind.
    pub fn train(rows: &[(Styles, bool)]) -> Option<Self> {
        let sung = rows.iter().filter(|r| r.1).count();
        let min = crate::songtype::MIN_PROBE_LABELS;
        if sung < min || rows.len() - sung < min {
            return None;
        }
        let x: Vec<(Vec<(usize, f32)>, f32)> = rows.iter().map(|(s, y)| (s.features(), f32::from(u8::from(*y)))).collect();
        let n = x.len() as f32;
        let dim = STYLE_COUNT + genres().len();
        let mut w = vec![0.0f32; dim + 1];
        let mut g = vec![0.0f32; dim + 1];
        for _ in 0..crate::songtype::PROBE_EPOCHS {
            g.iter_mut().for_each(|v| *v = 0.0);
            for (f, y) in &x {
                let d = y - Self::at(&w, f);
                g[0] += d;
                for (k, v) in f {
                    g[k + 1] += d * v * PROBE_SCALE;
                }
            }
            for (wi, gi) in w.iter_mut().zip(&g) {
                *wi += crate::songtype::PROBE_LR * (gi / n - crate::songtype::PROBE_L2 * *wi);
            }
        }
        Some(Self { w })
    }

    fn at(w: &[f32], f: &[(usize, f32)]) -> f32 {
        crate::songtype::sigmoid(w[0] + f.iter().map(|(k, v)| w[k + 1] * v * PROBE_SCALE).sum::<f32>())
    }

    /// The chance a track of these styles is sung; `None` without any.
    pub fn p(&self, s: &Styles) -> Option<f32> {
        let f = s.features();
        (!f.is_empty()).then(|| Self::at(&self.w, &f))
    }
}

/// The least mass of family votes [`StyleFamilies::leading`] trusts (one
/// song whose leading style is surely of a family).
pub const MIN_FAMILY_MASS: f32 = 0.5;

/// Which family of genres ([`crate::mix_names`]) each style and genre of
/// the labels is of, learnt from the library itself: the songs of the
/// artists whose tags give them a family vote with their styles for it.
/// An artist no tag tells then gets the family their songs' styles lead
/// ([`StyleFamilies::leading`]). Nothing maps a style's name to a family.
#[derive(Clone, Debug, Default)]
pub struct StyleFamilies {
    /// For each feature ([`Styles::features`]), the families with their
    /// shares of it (summing to 1).
    of: HashMap<usize, Vec<(u8, f32)>>,
}

impl StyleFamilies {
    /// Learnt from `tagged`: the styles of a song and the family of its
    /// artist.
    pub fn learn<'a>(tagged: impl IntoIterator<Item = (&'a Styles, u8)>) -> Self {
        let mut mass: HashMap<usize, HashMap<u8, f32>> = HashMap::new();
        for (s, family) in tagged {
            for (k, p) in s.features() {
                *mass.entry(k).or_default().entry(family).or_default() += p;
            }
        }
        let of = mass
            .into_iter()
            .map(|(k, by)| {
                let total: f32 = by.values().sum();
                let mut v: Vec<(u8, f32)> = by.into_iter().map(|(f, m)| (f, m / total)).collect();
                v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
                (k, v)
            })
            .collect();
        Self { of }
    }

    /// The families a song of these styles votes for, by probability: each
    /// leading style by what it was learnt to be of, its genre when the
    /// style itself was never seen.
    pub fn votes(&self, s: &Styles) -> Vec<(u8, f32)> {
        let mut out: Vec<(u8, f32)> = Vec::new();
        for &(i, p) in &s.top {
            let i = usize::from(i);
            let Some(g) = genre_index(i) else { continue };
            let Some(shares) = self.of.get(&i).or_else(|| self.of.get(&(STYLE_COUNT + g))) else { continue };
            for &(f, share) in shares {
                match out.iter_mut().find(|o| o.0 == f) {
                    Some(o) => o.1 += p * share,
                    None => out.push((f, p * share)),
                }
            }
        }
        out.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        out
    }

    /// The family a set of songs' styles lead with
    /// [`crate::mix_names::LEAD_SHARE`] of their votes, from
    /// [`MIN_FAMILY_MASS`] of them on.
    pub fn leading<'a>(&self, styles: impl IntoIterator<Item = &'a Styles>) -> Option<u8> {
        let mut votes: HashMap<u8, f32> = HashMap::new();
        for s in styles {
            for (f, p) in self.votes(s) {
                *votes.entry(f).or_default() += p;
            }
        }
        let total: f32 = votes.values().sum();
        let (f, v) = votes.into_iter().max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)))?;
        (total >= MIN_FAMILY_MASS && v >= crate::mix_names::LEAD_SHARE * total).then_some(f)
    }
}

/// The running mean of the windows' style probabilities (weighted by the
/// audio each covers), as [`crate::embedding::EmbedSum`] does for vectors.
#[derive(Clone, Debug, Default)]
pub struct StyleSum(crate::embedding::EmbedSum);

impl StyleSum {
    pub fn add(&mut self, styles: &[f32], weight: f64) {
        if styles.len() == STYLE_COUNT {
            self.0.add(styles, weight);
        }
    }

    pub fn mean(&self) -> Option<Vec<f32>> {
        self.0.mean()
    }

    pub fn styles(&self) -> Option<Styles> {
        Styles::of(&self.mean()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(label: &str) -> usize {
        STYLES.iter().position(|l| *l == label).unwrap_or_else(|| panic!("no {label}"))
    }

    /// A track of a few styles over the noise of the rest.
    fn track(styles: &[(&str, f32)]) -> Vec<f32> {
        let mut v = vec![0.01; STYLE_COUNT];
        for (l, p) in styles {
            v[at(l)] = *p;
        }
        v
    }

    /// The labels are the model's, each once; their genres are read off
    /// the list itself.
    #[test]
    fn the_labels_and_their_genres() {
        let all: std::collections::HashSet<&str> = STYLES.iter().copied().collect();
        assert_eq!(all.len(), STYLE_COUNT, "labels are unique");
        assert_eq!(label(0), Some("Blues---Boogie Woogie"));
        assert_eq!(genre_of("Stage & Screen---Score"), "Stage & Screen");
        assert_eq!(style_of("Stage & Screen---Score"), "Score");
        assert_eq!(genres().len(), 15, "{:?}", genres());
        assert_eq!(genres()[genre_index(0).unwrap()], "Blues");
        assert_eq!(genre_index(at("Rock---Punk")), genre_index(at("Rock---Thrash")));
        assert_eq!(genre_index(400), None);
    }

    /// The leading styles over the noise of the rest.
    #[test]
    fn the_leading_styles() {
        let score = Styles::of(&track(&[("Stage & Screen---Score", 0.6), ("Classical---Modern", 0.3), ("Electronic---Ambient", 0.2)])).unwrap();
        assert_eq!(score.leading().next(), Some(("Stage & Screen---Score", 0.6)));
        assert_eq!(score.leading().count(), TOP);
        assert_eq!(score.sure, 1.0, "the mass is capped: {score:?}");
        assert!(Styles::of(&[0.5; 10]).is_none());
        assert!(Styles::of(&track(&[("Rock---Punk", f32::NAN)])).is_none());
    }

    fn lead(l: &str, p: f32) -> Styles {
        let mut top = [(u16::MAX, 0.0); TOP];
        top[0] = (at(l) as u16, p);
        Styles { top, sure: p }
    }

    /// What styles say of words is learnt from the library's labels,
    /// whichever way they go: here the library's scores are sung and its
    /// rap is not, and the probe follows; a style it never saw leans on
    /// its genre; too few labels train nothing.
    #[test]
    fn the_style_probe_learns_the_librarys_labels() {
        let mut rows = Vec::new();
        for k in 0..30 {
            let p = 0.5 + (k % 5) as f32 * 0.1;
            rows.push((lead("Stage & Screen---Score", p), true));
            rows.push((lead("Hip Hop---Trap", p), false));
        }
        let probe = StyleProbe::train(&rows).unwrap();
        assert!(probe.p(&lead("Stage & Screen---Score", 0.7)).unwrap() > 0.85);
        assert!(probe.p(&lead("Hip Hop---Trap", 0.7)).unwrap() < 0.15);
        // Unseen, but of the genre of the sung ones.
        assert!(probe.p(&lead("Stage & Screen---Musical", 0.7)).unwrap() > 0.5);
        assert_eq!(probe.p(&Styles::default()), None);
        assert!(StyleProbe::train(&rows[..30]).is_none(), "15 of each is too few");
    }

    /// The families are learnt from the tagged artists' songs: a style
    /// takes the family its tagged songs were of, an unseen style its
    /// genre's; a split or a faint vote names none.
    #[test]
    fn styles_learn_families_from_the_tagged() {
        let (ambient, jpop) = (15u8, 1u8);
        let a = lead("Electronic---Ambient", 0.8);
        let j = lead("Pop---J-pop", 0.8);
        let punk = lead("Rock---Punk", 0.8);
        let learnt = StyleFamilies::learn([(&a, ambient), (&a, ambient), (&j, jpop), (&punk, 9)]);
        assert_eq!(learnt.leading([&a, &a]), Some(ambient));
        // Never seen: its genre's family (Electronic's, from the ambient songs).
        let drone = lead("Electronic---Drone", 0.8);
        assert_eq!(learnt.leading([&drone]), Some(ambient));
        // Split between two families: none leads.
        assert_eq!(learnt.leading([&a, &j, &punk]), None);
        // Too little to say, or nothing learnt of it.
        assert_eq!(learnt.leading([&lead("Pop---J-pop", 0.2)]), None);
        assert_eq!(learnt.leading([&lead("Jazz---Bop", 0.9)]), None);
        assert_eq!(StyleFamilies::default().leading([&a]), None);
    }

    #[test]
    fn the_sum_averages_windows() {
        let mut s = StyleSum::default();
        assert!(s.styles().is_none());
        s.add(&track(&[("Rock---Punk", 0.9)]), 1.0);
        s.add(&track(&[("Rock---Punk", 0.3)]), 2.0);
        s.add(&[0.1; 3], 5.0);
        assert!((s.mean().unwrap()[at("Rock---Punk")] - 0.5).abs() < 1e-6);
    }
}

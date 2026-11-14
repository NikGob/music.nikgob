//! The 400 Discogs styles of Discogs-EffNet's other head, and what they
//! say together.
//!
//! The model gives a sigmoid per style on every patch ([`STYLES`], its own
//! label list, «Genre---Style», in the order of its output); the runtime
//! averages them over the windows of a track and this module turns that
//! into a few numbers ([`Styles`]): the leading styles, and coarse shares
//! read off the labels' own meaning — words or no words ([`VOICED`],
//! [`WORDLESS`]), electronic, acoustic, speech. Nothing here knows a user,
//! an artist or a language.
//!
//! The shares are of the styles' mass above a noise floor ([`FLOOR`]): the
//! 400 sigmoids each keep a little probability, and summed raw that long
//! tail drowns the few styles a track really is.

use std::collections::HashMap;

/// Outputs of the style head.
pub const STYLE_COUNT: usize = 400;
/// Leading styles kept of a track.
pub const TOP: usize = 4;
/// A style's mean probability counts from this much up (see the module docs).
pub const FLOOR: f32 = 0.02;
/// The steepness of the words-or-not prior on the balance of the styles
/// (−1 wordless … +1 voiced): ±0.5 of the mass on one side is p ≈ 0.92.
pub const VOICE_GAIN: f32 = 5.0;

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

/// Styles whose music is nearly always without words (by the label).
#[rustfmt::skip]
pub const WORDLESS: &[&str] = &[
    "Brass & Military---Brass Band", "Brass & Military---Marches", "Brass & Military---Military",
    "Classical---Baroque", "Classical---Classical", "Classical---Contemporary", "Classical---Impressionist",
    "Classical---Modern", "Classical---Neo-Classical", "Classical---Neo-Romantic", "Classical---Post-Modern",
    "Classical---Romantic",
    "Electronic---Abstract", "Electronic---Acid", "Electronic---Ambient", "Electronic---Berlin-School",
    "Electronic---Bleep", "Electronic---Breakcore", "Electronic---Chiptune", "Electronic---Dark Ambient",
    "Electronic---Deep Techno", "Electronic---Drone", "Electronic---Dub", "Electronic---Dub Techno",
    "Electronic---Dungeon Synth", "Electronic---Experimental", "Electronic---Glitch", "Electronic---Goa Trance",
    "Electronic---Hard Techno", "Electronic---IDM", "Electronic---Illbient", "Electronic---Minimal",
    "Electronic---Minimal Techno", "Electronic---Modern Classical", "Electronic---Musique Concrète",
    "Electronic---New Age", "Electronic---Noise", "Electronic---Psy-Trance", "Electronic---Rhythmic Noise",
    "Electronic---Schranz", "Electronic---Sound Collage", "Electronic---Synthwave", "Electronic---Tech House",
    "Electronic---Techno",
    "Hip Hop---Cut-up/DJ", "Hip Hop---DJ Battle Tool", "Hip Hop---Instrumental", "Hip Hop---Turntablism",
    "Jazz---Afro-Cuban Jazz", "Jazz---Avant-garde Jazz", "Jazz---Bop", "Jazz---Contemporary Jazz", "Jazz---Cool Jazz",
    "Jazz---Free Improvisation", "Jazz---Free Jazz", "Jazz---Fusion", "Jazz---Gypsy Jazz", "Jazz---Hard Bop",
    "Jazz---Jazz-Funk", "Jazz---Jazz-Rock", "Jazz---Latin Jazz", "Jazz---Modal", "Jazz---Post Bop", "Jazz---Ragtime",
    "Jazz---Smooth Jazz", "Jazz---Soul-Jazz",
    "Latin---Batucada", "Latin---Descarga",
    "Non-Music---Field Recording",
    "Pop---Light Music",
    "Reggae---Dub",
    "Rock---Math Rock", "Rock---Post Rock", "Rock---Surf",
    "Stage & Screen---Score", "Stage & Screen---Soundtrack",
];

/// Styles whose music nearly always has words, sung or spoken (by the label).
#[rustfmt::skip]
pub const VOICED: &[&str] = &[
    "Blues---Chicago Blues", "Blues---Country Blues", "Blues---Delta Blues", "Blues---Electric Blues",
    "Blues---Jump Blues", "Blues---Louisiana Blues", "Blues---Modern Electric Blues", "Blues---Rhythm & Blues",
    "Blues---Texas Blues",
    "Children's---Educational", "Children's---Nursery Rhymes", "Children's---Story",
    "Classical---Choral", "Classical---Opera",
    "Electronic---Dance-pop", "Electronic---Darkwave", "Electronic---Disco Polo", "Electronic---Electroclash",
    "Electronic---Euro House", "Electronic---Euro-Disco", "Electronic---Eurobeat", "Electronic---Eurodance",
    "Electronic---Freestyle", "Electronic---Ghetto", "Electronic---Grime", "Electronic---Hi NRG",
    "Electronic---Hip Hop", "Electronic---Hip-House", "Electronic---Italo-Disco", "Electronic---Italodance",
    "Electronic---New Wave", "Electronic---Synth-pop",
    "Folk, World, & Country---Cajun", "Folk, World, & Country---Canzone Napoletana", "Folk, World, & Country---Country",
    "Folk, World, & Country---Fado", "Folk, World, & Country---Folk", "Folk, World, & Country---Gospel",
    "Folk, World, & Country---Hillbilly", "Folk, World, & Country---Honky Tonk", "Folk, World, & Country---Laïkó",
    "Folk, World, & Country---Raï", "Folk, World, & Country---Séga", "Folk, World, & Country---Soukous",
    "Folk, World, & Country---Volksmusik", "Folk, World, & Country---Zouk", "Folk, World, & Country---Éntekhno",
    "Funk / Soul---Boogie", "Funk / Soul---Contemporary R&B", "Funk / Soul---Disco", "Funk / Soul---Gospel",
    "Funk / Soul---Neo Soul", "Funk / Soul---New Jack Swing", "Funk / Soul---Rhythm & Blues", "Funk / Soul---Soul",
    "Funk / Soul---Swingbeat", "Funk / Soul---UK Street Soul",
    "Hip Hop---Boom Bap", "Hip Hop---Bounce", "Hip Hop---Britcore", "Hip Hop---Cloud Rap", "Hip Hop---Conscious",
    "Hip Hop---Crunk", "Hip Hop---Electro", "Hip Hop---G-Funk", "Hip Hop---Gangsta", "Hip Hop---Grime",
    "Hip Hop---Hardcore Hip-Hop", "Hip Hop---Horrorcore", "Hip Hop---Jazzy Hip-Hop", "Hip Hop---Pop Rap",
    "Hip Hop---Ragga HipHop", "Hip Hop---RnB/Swing", "Hip Hop---Screw", "Hip Hop---Thug Rap", "Hip Hop---Trap",
    "Latin---Bolero", "Latin---Compas", "Latin---Cumbia", "Latin---Forró", "Latin---Guajira", "Latin---MPB",
    "Latin---Mariachi", "Latin---Merengue", "Latin---Norteño", "Latin---Nueva Cancion", "Latin---Ranchera",
    "Latin---Reggaeton", "Latin---Salsa", "Latin---Tejano", "Latin---Vallenato",
    "Non-Music---Audiobook", "Non-Music---Comedy", "Non-Music---Dialogue", "Non-Music---Education",
    "Non-Music---Interview", "Non-Music---Monolog", "Non-Music---Poetry", "Non-Music---Political",
    "Non-Music---Promotional", "Non-Music---Radioplay", "Non-Music---Spoken Word",
    "Pop---Ballad", "Pop---Bollywood", "Pop---Bubblegum", "Pop---Chanson", "Pop---City Pop", "Pop---Europop",
    "Pop---Indie Pop", "Pop---J-pop", "Pop---K-pop", "Pop---Kayōkyoku", "Pop---Music Hall", "Pop---Parody",
    "Pop---Schlager", "Pop---Vocal",
    "Reggae---Calypso", "Reggae---Dancehall", "Reggae---Lovers Rock", "Reggae---Ragga", "Reggae---Reggae",
    "Reggae---Reggae-Pop", "Reggae---Rocksteady", "Reggae---Roots Reggae", "Reggae---Soca",
    "Rock---AOR", "Rock---Alternative Rock", "Rock---Arena Rock", "Rock---Beat", "Rock---Black Metal",
    "Rock---Blues Rock", "Rock---Brit Pop", "Rock---Classic Rock", "Rock---Coldwave", "Rock---Country Rock",
    "Rock---Crust", "Rock---Death Metal", "Rock---Deathcore", "Rock---Deathrock", "Rock---Doo Wop", "Rock---Dream Pop",
    "Rock---Emo", "Rock---Folk Metal", "Rock---Folk Rock", "Rock---Funk Metal", "Rock---Garage Rock", "Rock---Glam",
    "Rock---Goregrind", "Rock---Goth Rock", "Rock---Gothic Metal", "Rock---Grindcore", "Rock---Grunge",
    "Rock---Hard Rock", "Rock---Hardcore", "Rock---Heavy Metal", "Rock---Indie Rock", "Rock---Melodic Death Metal",
    "Rock---Melodic Hardcore", "Rock---Metalcore", "Rock---Mod", "Rock---Neofolk", "Rock---New Wave", "Rock---Nu Metal",
    "Rock---Oi", "Rock---Parody", "Rock---Pop Punk", "Rock---Pop Rock", "Rock---Pornogrind", "Rock---Post-Hardcore",
    "Rock---Post-Punk", "Rock---Power Metal", "Rock---Power Pop", "Rock---Power Violence", "Rock---Psychobilly",
    "Rock---Pub Rock", "Rock---Punk", "Rock---Rock & Roll", "Rock---Rockabilly", "Rock---Soft Rock",
    "Rock---Southern Rock", "Rock---Speed Metal", "Rock---Technical Death Metal", "Rock---Thrash", "Rock---Twist",
    "Rock---Viking Metal", "Rock---Yé-Yé",
    "Stage & Screen---Musical",
];

/// Styles of speech rather than music.
#[rustfmt::skip]
pub const SPEECH: &[&str] = &[
    "Children's---Story", "Non-Music---Audiobook", "Non-Music---Comedy", "Non-Music---Dialogue",
    "Non-Music---Education", "Non-Music---Interview", "Non-Music---Monolog", "Non-Music---Poetry",
    "Non-Music---Political", "Non-Music---Radioplay", "Non-Music---Spoken Word",
];

/// Genres whose styles are played on acoustic instruments, and styles of
/// other genres that are.
const ACOUSTIC_GENRES: &[&str] = &["Blues", "Brass & Military", "Classical", "Folk, World, & Country", "Jazz", "Latin"];
#[rustfmt::skip]
const ACOUSTIC: &[&str] = &[
    "Electronic---Modern Classical", "Electronic---Neofolk", "Pop---Chanson", "Pop---Light Music",
    "Rock---Acoustic", "Rock---Folk Rock", "Rock---Neofolk", "Stage & Screen---Musical", "Stage & Screen---Score",
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

/// Each output's place in a group of labels.
fn members(group: &[&str]) -> Vec<bool> {
    let set: std::collections::HashSet<&str> = group.iter().copied().collect();
    STYLES.iter().map(|l| set.contains(l)).collect()
}

/// What a track's mean style probabilities say together (see the module
/// docs). Shares are 0…1.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Styles {
    /// The leading styles with their mean probability, the most likely
    /// first; `(u16::MAX, 0)` fills a place left empty.
    pub top: [(u16, f32); TOP],
    /// How likely the track has words (sung or spoken), from the labels
    /// alone.
    pub voice: f32,
    pub electronic: f32,
    pub acoustic: f32,
    pub speech: f32,
    /// The mass of the leading styles (how sure the model is of any).
    pub sure: f32,
}

/// The groups of the labels, made once.
struct Groups {
    wordless: Vec<bool>,
    voiced: Vec<bool>,
    speech: Vec<bool>,
    acoustic: Vec<bool>,
    electronic: Vec<bool>,
}

fn groups() -> &'static Groups {
    static GROUPS: std::sync::OnceLock<Groups> = std::sync::OnceLock::new();
    GROUPS.get_or_init(|| {
        let mut acoustic = members(ACOUSTIC);
        for (a, l) in acoustic.iter_mut().zip(STYLES) {
            *a |= ACOUSTIC_GENRES.contains(&genre_of(l));
        }
        let electronic = STYLES.iter().map(|l| genre_of(l) == "Electronic").collect();
        Groups { wordless: members(WORDLESS), voiced: members(VOICED), speech: members(SPEECH), acoustic, electronic }
    })
}

impl Styles {
    /// The summary of the mean probabilities of the 400 styles; `None`
    /// for a vector of another length or not numbers.
    pub fn of(mean: &[f32]) -> Option<Self> {
        if mean.len() != STYLE_COUNT || mean.iter().any(|x| !x.is_finite()) {
            return None;
        }
        let g = groups();
        let mass: Vec<f32> = mean.iter().map(|p| (p - FLOOR).max(0.0)).collect();
        let total: f32 = mass.iter().sum();
        let share = |of: &[bool]| if total > 0.0 { mass.iter().zip(of).filter(|(_, m)| **m).map(|(x, _)| *x).sum::<f32>() / total } else { 0.0 };
        let balance = share(&g.voiced) - share(&g.wordless);
        let mut order: Vec<usize> = (0..STYLE_COUNT).collect();
        order.sort_by(|a, b| mean[*b].total_cmp(&mean[*a]).then(a.cmp(b)));
        let mut top = [(u16::MAX, 0.0); TOP];
        for (slot, &i) in top.iter_mut().zip(&order) {
            *slot = (i as u16, mean[i].clamp(0.0, 1.0));
        }
        Some(Self {
            top,
            voice: if total > 0.0 { crate::songtype::sigmoid(VOICE_GAIN * balance) } else { 0.5 },
            electronic: share(&g.electronic),
            acoustic: share(&g.acoustic),
            speech: share(&g.speech),
            sure: top.iter().map(|t| t.1).sum::<f32>().min(1.0),
        })
    }

    /// The leading styles that are there, with their labels.
    pub fn leading(&self) -> impl Iterator<Item = (&'static str, f32)> + '_ {
        self.top.iter().filter_map(|&(i, p)| Some((label(usize::from(i))?, p)))
    }

    /// The families of genres ([`crate::mix_names`]) the leading styles
    /// vote for, by their probability: a style's own name first
    /// («Ambient», «J-pop»), its genre when the name is of none
    /// («Electronic---Bleep» is electronic).
    pub fn family_votes(&self) -> Vec<(u8, f32)> {
        let mut out: Vec<(u8, f32)> = Vec::new();
        for (l, p) in self.leading() {
            let family = crate::mix_names::family(style_of(l)).or_else(|| crate::mix_names::family(genre_of(l)));
            let Some(f) = family.and_then(crate::mix_names::family_id) else { continue };
            match out.iter_mut().find(|o| o.0 == f) {
                Some(o) => o.1 += p,
                None => out.push((f, p)),
            }
        }
        out.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        out
    }
}

/// The least mass of family votes [`leading_family`] trusts (one song
/// whose leading style is surely of a family).
pub const MIN_FAMILY_MASS: f32 = 0.5;

/// The family of genres a set of tracks' styles lead with
/// [`crate::mix_names::LEAD_SHARE`] of their votes, from [`MIN_FAMILY_MASS`]
/// of them on (an artist of no tags, by their songs' sound).
pub fn leading_family<'a>(styles: impl IntoIterator<Item = &'a Styles>) -> Option<u8> {
    let mut votes: HashMap<u8, f32> = HashMap::new();
    for s in styles {
        for (f, p) in s.family_votes() {
            *votes.entry(f).or_default() += p;
        }
    }
    let total: f32 = votes.values().sum();
    let (f, v) = votes.into_iter().max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)))?;
    (total >= MIN_FAMILY_MASS && v >= crate::mix_names::LEAD_SHARE * total).then_some(f)
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

    /// Every group names the model's own labels, each once, and no style is
    /// both voiced and wordless.
    #[test]
    fn the_groups_are_the_models_labels() {
        let all: std::collections::HashSet<&str> = STYLES.iter().copied().collect();
        assert_eq!(all.len(), STYLE_COUNT, "labels are unique");
        for group in [WORDLESS, VOICED, SPEECH, ACOUSTIC] {
            for l in group {
                assert!(all.contains(l), "{l} is not a label");
            }
        }
        assert!(WORDLESS.iter().all(|l| !VOICED.contains(l)));
        assert!(SPEECH.iter().all(|l| VOICED.contains(l)), "speech has words");
        assert_eq!(label(0), Some("Blues---Boogie Woogie"));
        assert_eq!(genre_of("Stage & Screen---Score"), "Stage & Screen");
        assert_eq!(style_of("Stage & Screen---Score"), "Score");
    }

    /// A score is wordless, rap has words, spoken word is speech; the
    /// noise floor keeps the long tail out of the shares.
    #[test]
    fn the_labels_tell_words_and_kinds() {
        let score = Styles::of(&track(&[("Stage & Screen---Score", 0.6), ("Classical---Modern", 0.3), ("Electronic---Ambient", 0.2)])).unwrap();
        assert!(score.voice < 0.1, "{score:?}");
        assert!(score.acoustic > 0.6 && score.speech == 0.0, "{score:?}");
        assert_eq!(score.leading().next(), Some(("Stage & Screen---Score", 0.6)));
        let rap = Styles::of(&track(&[("Hip Hop---Trap", 0.7), ("Hip Hop---Cloud Rap", 0.4)])).unwrap();
        assert!(rap.voice > 0.95, "{rap:?}");
        let techno = Styles::of(&track(&[("Electronic---Techno", 0.8), ("Electronic---Minimal", 0.3)])).unwrap();
        assert!(techno.voice < 0.1 && techno.electronic > 0.95, "{techno:?}");
        let talk = Styles::of(&track(&[("Non-Music---Spoken Word", 0.8), ("Non-Music---Poetry", 0.3)])).unwrap();
        assert!(talk.speech > 0.95 && talk.voice > 0.95, "{talk:?}");
        // Nothing above the floor: no opinion.
        let flat = Styles::of(&vec![0.015; STYLE_COUNT]).unwrap();
        assert_eq!(flat.voice, 0.5);
        assert!(Styles::of(&[0.5; 10]).is_none());
        assert!(Styles::of(&track(&[("Rock---Punk", f32::NAN)])).is_none());
    }

    /// The leading styles vote for the families of genres by their names,
    /// and an artist's songs agree on one.
    #[test]
    fn styles_vote_for_families() {
        use crate::mix_names::family_id;
        let ambient = Styles::of(&track(&[("Electronic---Ambient", 0.7), ("Electronic---Drone", 0.3)])).unwrap();
        assert_eq!(ambient.family_votes()[0].0, family_id("Эмбиент-микс").unwrap());
        let jpop = Styles::of(&track(&[("Pop---J-pop", 0.8), ("Rock---Pop Rock", 0.2)])).unwrap();
        assert_eq!(jpop.family_votes()[0].0, family_id("J-pop микс").unwrap());
        let score = Styles::of(&track(&[("Stage & Screen---Soundtrack", 0.8)])).unwrap();
        assert_eq!(leading_family([&score, &score]), family_id("Микс саундтреков"));
        // Split between two families: none leads.
        let drone = Styles::of(&track(&[("Electronic---Drone", 0.8)])).unwrap();
        let idol = Styles::of(&track(&[("Pop---J-pop", 0.8)])).unwrap();
        assert_eq!(leading_family([&drone, &idol]), None);
        // Too little to say.
        let faint = Styles::of(&track(&[("Pop---J-pop", 0.2)])).unwrap();
        assert_eq!(leading_family([&faint]), None);
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

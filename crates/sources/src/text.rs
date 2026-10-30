//! What to search for: the main artist without credit noise, the forms a
//! title goes by (original script and romaji, without «feat.» and upload
//! suffixes), an album without its "Original Soundtrack" words, and the
//! queries made of them, most specific first.
//!
//! Found in the wishes the indexer had nothing for (docs/stage4.5.md):
//! queries carried every credited artist («Con Fetti Pesto, David Münnich,
//! Alexx Grimm …») or the UI's «и ещё 1 исполнитель», a title in two
//! scripts at once («火星人 - Martian»), upload junk («//speed up//»), and
//! a soundtrack under its composer while trackers name the game.

use nikgob_core::source::{Want, WantScope};
use nikgob_metadata::text::{album_key, artist_key, search_artist, search_title, title_key};

/// The main artist as a search word: the first credited name, no "feat."
/// tail, no «и ещё N исполнитель», no "A, B & C" list.
pub fn main_artist(raw: &str) -> String {
    let mut s = search_artist(raw);
    for marker in [", и ещё", " и ещё ", ", and ", " and 1 more", " и другие"] {
        if let Some(at) = s.find(marker) {
            s.truncate(at);
        }
    }
    // A list of names: the first one ("Rosa Walton, Hallie Coggins").
    if let Some((first, _)) = s.split_once(", ") {
        s = first.to_owned();
    }
    s.trim().to_owned()
}

/// Upload junk a title gets on YouTube and SoundCloud that no release has.
const JUNK: &[&str] = &[
    "speed up", "sped up", "speedup", "slowed", "reverb", "nightcore", "8d audio", "bass boosted", "official audio",
    "official video", "official music video", "music video", "lyric video", "lyrics", "audio",
];

fn strip_junk(s: &str) -> String {
    // "//speed up//", "| speed up", "(sped up)": the marks go, then the words.
    let mut out = s.replace("//", " ").replace('|', " ");
    let lower = out.to_lowercase();
    for j in JUNK {
        if let Some(at) = lower.rfind(j) {
            // Only at the end: "Audio" inside a real title stays.
            if lower[at + j.len()..].trim().is_empty() && at > 0 {
                out.truncate(at);
            }
        }
    }
    out.trim_matches(|c: char| c.is_whitespace() || matches!(c, '-' | '–' | '—' | '/' | '|')).to_owned()
}

/// Has letters of a script other than Latin (kana, kanji, Cyrillic, Hangul).
fn native(s: &str) -> bool {
    s.chars().any(|c| c.is_alphabetic() && (c as u32) >= 0x250)
}

/// Has Latin letters.
fn latin(s: &str) -> bool {
    s.chars().any(|c| c.is_ascii_alphabetic())
}

/// The forms a title goes by, the fullest first: "火星人 - Martian" →
/// ["火星人 Martian", "火星人", "Martian"]; "Kan Saete Kuyashiiwa
/// (勘冴えて悔しいわ)" → its two scripts; kana also in romaji. No "feat."
/// tails, brackets of release noise, or upload junk.
pub fn title_forms(raw: &str) -> Vec<String> {
    fn push(out: &mut Vec<String>, s: String) {
        let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
        if s.chars().any(char::is_alphanumeric) && !out.iter().any(|o| o.eq_ignore_ascii_case(&s)) {
            out.push(s);
        }
    }
    let mut out: Vec<String> = Vec::new();
    let cleaned = strip_junk(raw);
    let base = search_title(&cleaned);
    push(&mut out, base.replace(" - ", " ").replace(" / ", " "));
    // A title in two scripts: "原題 - English", "原題 / Romaji", "Romaji (原題)".
    let mut parts: Vec<String> = Vec::new();
    for sep in [" - ", " – ", " — ", " / ", "／", " | "] {
        if let Some((a, b)) = base.split_once(sep) {
            parts.push(a.trim().to_owned());
            parts.push(b.trim().to_owned());
        }
    }
    if parts.is_empty() {
        // A bracket in another script than the rest (search_title kept it,
        // or dropped it as noise: look at the raw title).
        if let (Some(open), Some(close)) = (cleaned.find(['(', '（', '[', '「']), cleaned.rfind([')', '）', ']', '」']))
            && open < close
        {
            let inner = cleaned[open..close].trim_start_matches(['(', '（', '[', '「']).trim().to_owned();
            let outer = format!("{}{}", &cleaned[..open], cleaned[close..].trim_start_matches([')', '）', ']', '」']));
            if native(&inner) != native(&outer) && !inner.is_empty() {
                parts.push(outer.trim().to_owned());
                parts.push(inner);
            }
        }
    }
    let mixed = parts.len() >= 2 && parts.iter().any(|p| native(p)) && parts.iter().any(|p| latin(p) && !native(p));
    if mixed {
        for p in parts {
            push(&mut out, search_title(&p));
        }
    }
    let romaji: Vec<String> = out.iter().filter_map(|f| nikgob_core::names::kana_romaji(f)).collect();
    for r in romaji {
        push(&mut out, r);
    }
    if out.is_empty() {
        push(&mut out, raw.trim().to_owned());
    }
    out
}

/// Soundtrack words of an album title.
const OST_WORDS: &[&str] = &[
    "original soundtrack",
    "original game soundtrack",
    "original motion picture soundtrack",
    "original score",
    "official soundtrack",
    "soundtrack",
    "ost",
    "o.s.t.",
    "サウンドトラック",
    "саундтрек",
];

/// An album whose title says it is a soundtrack (of a game, an anime, a
/// film), or a song made for one ("Supraland Credits Song").
pub fn is_soundtrack(album: &str) -> bool {
    let l = album.to_lowercase();
    OST_WORDS.iter().any(|w| contains_word(&l, w)) || l.contains("credits song") || l.contains("theme song")
}

/// `w` in `hay` not glued to letters on either side.
fn contains_word(hay: &str, w: &str) -> bool {
    hay.match_indices(w).any(|(i, _)| {
        let before = hay[..i].chars().next_back();
        let after = hay[i + w.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    })
}

/// The album's own name for a search: no "(Original Soundtrack)", "OST",
/// "Deluxe Edition" or "Vol." noise: "Secrets of the Silent Witch Original
/// Soundtrack" → "Secrets of the Silent Witch", "Cyberpunk 2077: Radio,
/// Vol. 2 (Original Soundtrack)" → "Cyberpunk 2077 Radio Vol. 2".
pub fn album_core(raw: &str) -> String {
    let mut s = search_title(raw);
    let lower = s.to_lowercase();
    for w in OST_WORDS {
        if let Some(at) = lower.find(w)
            && contains_word(&lower, w)
        {
            let rest = &s[at + w.len()..];
            s = format!("{}{}", &s[..at], rest);
            break;
        }
    }
    let s = s.replace([':', ',', '(', ')', '[', ']'], " ");
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s = s.trim_matches(|c: char| matches!(c, '-' | '–' | '—' | ' ')).to_owned();
    if s.is_empty() { search_title(raw) } else { s }
}

/// The song a soundtrack track title names besides itself ("Das Nennt
/// Ihr Himmel (Supraland Six Inches Under Credits Song)" → "Supraland Six
/// Inches Under").
pub fn franchise_of(raw: &str) -> Option<String> {
    let open = raw.find('(')?;
    let close = raw.rfind(')')?;
    let inner = raw.get(open + 1..close)?;
    let lower = inner.to_lowercase();
    for w in ["credits song", "theme song", "main theme", "ending theme", "opening theme", "soundtrack"] {
        if let Some(at) = lower.find(w) {
            let name = inner[..at].trim().trim_start_matches("from ").trim_start_matches("from the ").trim();
            if name.chars().filter(|c| c.is_alphanumeric()).count() >= 3 {
                return Some(name.to_owned());
            }
        }
    }
    None
}

/// Queries for a torrent indexer, most specific first, at most `max`:
/// "artist album", "artist title" for each form of the title, the
/// soundtrack's own name, then the artist alone (a discography or a
/// collection the single is in).
pub fn torrent_queries(w: &Want, max: usize) -> Vec<String> {
    let artist = main_artist(w.artist());
    let mut out: Vec<String> = Vec::new();
    let mut push = |q: String| {
        let q = q.split_whitespace().collect::<Vec<_>>().join(" ");
        if !q.is_empty() && !out.iter().any(|o| o.eq_ignore_ascii_case(&q)) {
            out.push(q);
        }
    };
    let ost = w.album.as_deref().is_some_and(is_soundtrack)
        || w.title.as_deref().is_some_and(|t| franchise_of(t).is_some());
    let with_artist = |s: &str| if artist.is_empty() { s.to_owned() } else { format!("{artist} {s}") };
    if let Some(album) = w.album.as_deref().filter(|_| !w.is_single()) {
        push(with_artist(&album_core(album)));
        if ost {
            push(album_core(album));
        }
    }
    if w.scope == WantScope::Track
        && let Some(t) = &w.title
    {
        for f in title_forms(t).into_iter().take(3) {
            push(with_artist(&f));
        }
        if let Some(f) = franchise_of(t) {
            push(f);
        }
    } else if w.scope == WantScope::Album && w.album.is_none() && w.title.is_some() {
        push(with_artist(&album_core(w.title.as_deref().unwrap_or_default())));
    }
    // The artist alone finds the discographies; a name of one or two
    // letters would find everything.
    if artist.chars().filter(|c| c.is_alphanumeric()).count() >= 3 {
        push(artist.clone());
    }
    out.truncate(max);
    out
}

/// Queries for a catalogue service (tracks searched by name): "artist
/// title" for the forms of the title, or "artist album".
pub fn catalogue_queries(w: &Want) -> Vec<String> {
    let artist = main_artist(w.artist());
    let mut out: Vec<String> = Vec::new();
    match (w.scope, &w.title, &w.album) {
        (WantScope::Track, Some(t), _) => {
            for f in title_forms(t).into_iter().take(2) {
                out.push(format!("{artist} {f}").trim().to_owned());
            }
        }
        (_, _, Some(a)) => out.push(format!("{artist} {}", album_core(a)).trim().to_owned()),
        (_, Some(t), None) => out.push(format!("{artist} {}", album_core(t)).trim().to_owned()),
        _ => {}
    }
    out.dedup();
    out
}

/// Folded keys of the title's forms, for matching.
pub fn title_keys(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = title_forms(raw).iter().map(|f| title_key(f).base).filter(|k| !k.is_empty()).collect();
    let full = title_key(raw).base;
    if !full.is_empty() && !out.contains(&full) {
        out.insert(0, full);
    }
    out
}

/// Folded keys of every name of the main artist.
pub fn artist_keys(w: &Want) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for n in std::iter::once(main_artist(w.artist())).chain(w.aliases.iter().cloned()) {
        let k = artist_key(&n);
        if !k.is_empty() && !out.contains(&k) {
            out.push(k);
        }
    }
    out
}

/// The album's folded core.
pub fn album_keys(raw: &str) -> Vec<String> {
    let mut out = vec![album_key(raw)];
    let core = album_key(&album_core(raw));
    if !core.is_empty() && !out.contains(&core) {
        out.push(core);
    }
    out.retain(|k| !k.is_empty());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn want(artist: &str, title: Option<&str>, album: Option<&str>) -> Want {
        Want {
            artists: vec![artist.into()],
            title: title.map(Into::into),
            album: album.map(Into::into),
            ..Want::default()
        }
    }

    #[test]
    fn the_main_artist_is_the_first_name_without_noise() {
        assert_eq!(main_artist("MellSher, и ещё 1 исполнитель"), "MellSher");
        assert_eq!(main_artist("Con Fetti Pesto, David Münnich, Alexx Grimm"), "Con Fetti Pesto");
        assert_eq!(main_artist("MORGENSHTERN feat. Palc"), "MORGENSHTERN");
        assert_eq!(main_artist("Simon & Garfunkel"), "Simon & Garfunkel");
    }

    #[test]
    fn a_title_in_two_scripts_is_searched_in_each() {
        assert_eq!(title_forms("火星人 - Martian"), ["火星人 Martian", "火星人", "Martian"]);
        assert_eq!(
            title_forms("沈黙の魔女 - Chinmoku no Majo"),
            ["沈黙の魔女 Chinmoku no Majo", "沈黙の魔女", "Chinmoku no Majo"]
        );
        let kan = title_forms("Kan Saete Kuyashiiwa (勘冴えて悔しいわ)");
        assert!(kan.contains(&"勘冴えて悔しいわ".to_owned()), "{kan:?}");
        assert!(kan.contains(&"Kan Saete Kuyashiiwa".to_owned()), "{kan:?}");
        assert_eq!(title_forms("Wind's Anthem (feat. suis From Yorushika)"), ["Wind's Anthem"]);
        assert_eq!(title_forms("Комсомольск - Мозги & деньги //speed up//")[0], "Комсомольск Мозги & деньги");
        // One script: no split into halves.
        assert_eq!(title_forms("Зеленоглазые деффки!"), ["Зеленоглазые деффки!"]);
        let kana = title_forms("ヨフケノパレエド");
        assert!(kana.len() == 2 && kana[1].is_ascii(), "in romaji too: {kana:?}");
    }

    #[test]
    fn soundtracks_are_searched_by_their_own_name() {
        assert!(is_soundtrack("Secrets of the Silent Witch Original Soundtrack"));
        assert!(is_soundtrack("Cyberpunk 2077: Radio, Vol. 2 (Original Soundtrack)"));
        assert!(!is_soundtrack("Lost in the Post"), "no 'ost' inside a word");
        assert_eq!(album_core("Secrets of the Silent Witch Original Soundtrack"), "Secrets of the Silent Witch");
        assert_eq!(album_core("Cyberpunk 2077: Radio, Vol. 2 (Original Soundtrack)"), "Cyberpunk 2077 Radio Vol. 2");
        assert_eq!(
            franchise_of("Das Nennt Ihr Himmel (Supraland Six Inches Under Credits Song)").as_deref(),
            Some("Supraland Six Inches Under")
        );
    }

    #[test]
    fn torrent_queries_go_from_the_album_to_the_artist_alone() {
        let w = want("Yorushika", Some("火星人 - Martian"), Some("second person"));
        assert_eq!(
            torrent_queries(&w, 8),
            ["Yorushika second person", "Yorushika 火星人 Martian", "Yorushika 火星人", "Yorushika Martian", "Yorushika"]
        );
        // A single: the album is the title, said once.
        let w = want("Eve", Some("Underdog"), Some("Underdog"));
        assert_eq!(torrent_queries(&w, 8), ["Eve Underdog", "Eve"]);
        let w = want("Yoshihisa Kato", Some("沈黙の魔女 - Chinmoku no Majo"), Some("Secrets of the Silent Witch Original Soundtrack"));
        let q = torrent_queries(&w, 3);
        assert_eq!(q, ["Yoshihisa Kato Secrets of the Silent Witch", "Secrets of the Silent Witch", "Yoshihisa Kato 沈黙の魔女 Chinmoku no Majo"]);
        let w = want("MellSher, и ещё 1 исполнитель", Some("Разгром"), Some("Разгром"));
        assert_eq!(torrent_queries(&w, 8), ["MellSher Разгром", "MellSher"]);
    }

    #[test]
    fn catalogue_queries_name_the_track() {
        let w = want("Eve", Some("Wind's Anthem (feat. suis From Yorushika)"), Some("Wind's Anthem"));
        assert_eq!(catalogue_queries(&w), ["Eve Wind's Anthem"]);
        let mut w = want("nyan.mp3", None, Some("nyan.pop"));
        w.scope = WantScope::Album;
        assert_eq!(catalogue_queries(&w), ["nyan.mp3 nyan.pop"]);
    }
}

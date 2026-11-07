//! Cyrillic titles written in Latin letters.
//!
//! Services and taggers sometimes give a Russian title transliterated (GOST
//! / ISO 9 / BGN styles: «ChETYRE UKRAINKI», «LEGENDARNAJa PYL'», «Gruppa
//! krovi»). One song under both spellings is still one song
//! ([`same_translit`]), and the Cyrillic spelling is the one to show
//! ([`prefer_cyrillic`]).
//!
//! The Latin side is read back into Cyrillic (the longest letter group
//! first: «shch», «zh», «ja», «ts»…), then both sides lose what the
//! schemes disagree on (ё/э → е, й/ы → и, ь/ъ dropped).

/// Latin letter groups and the Cyrillic letter they stand for, longest
/// first.
const GROUPS: &[(&str, &str)] = &[
    ("shch", "щ"),
    ("shh", "щ"),
    ("sch", "щ"),
    ("zh", "ж"),
    ("kh", "х"),
    ("ts", "ц"),
    ("tc", "ц"),
    ("ch", "ч"),
    ("sh", "ш"),
    ("ya", "я"),
    ("ja", "я"),
    ("yu", "ю"),
    ("ju", "ю"),
    ("yo", "ё"),
    ("jo", "ё"),
    ("ye", "е"),
    ("je", "е"),
    ("a", "а"),
    ("b", "б"),
    ("v", "в"),
    ("w", "в"),
    ("g", "г"),
    ("d", "д"),
    ("e", "е"),
    ("z", "з"),
    ("i", "и"),
    ("j", "й"),
    ("k", "к"),
    ("q", "к"),
    ("l", "л"),
    ("m", "м"),
    ("n", "н"),
    ("o", "о"),
    ("p", "п"),
    ("r", "р"),
    ("s", "с"),
    ("t", "т"),
    ("u", "у"),
    ("f", "ф"),
    ("h", "х"),
    ("x", "х"),
    ("c", "ц"),
    ("y", "ы"),
];

fn is_cyrillic(c: char) -> bool {
    matches!(c, '\u{0400}'..='\u{04FF}')
}

/// Has Cyrillic letters.
pub fn has_cyrillic(s: &str) -> bool {
    s.chars().any(is_cyrillic)
}

/// Has letters, all of them Latin.
fn latin_only(s: &str) -> bool {
    s.chars().any(char::is_alphabetic) && s.chars().all(|c| !c.is_alphabetic() || c.is_ascii_alphabetic())
}

/// Latin letters read as Cyrillic (see [`GROUPS`]); anything else stays.
pub fn to_cyrillic(latin: &str) -> String {
    let lower = latin.to_lowercase();
    let mut out = String::with_capacity(lower.len() * 2);
    let mut rest = lower.as_str();
    while let Some(c) = rest.chars().next() {
        if matches!(c, '\'' | 'ʹ' | '’' | '`' | 'ʼ') {
            out.push('ь');
            rest = &rest[c.len_utf8()..];
            continue;
        }
        match GROUPS.iter().find(|(l, _)| rest.starts_with(l)) {
            Some((l, cy)) => {
                out.push_str(cy);
                rest = &rest[l.len()..];
            }
            None => {
                out.push(c);
                rest = &rest[c.len_utf8()..];
            }
        }
    }
    out
}

/// What is left of a Cyrillic text once the schemes agree: lower case,
/// ё/э → е, й/ы → и, no ь/ъ, only letters and digits.
fn skeleton(cyrillic: &str) -> String {
    cyrillic
        .chars()
        .flat_map(char::to_lowercase)
        .filter_map(|c| match c {
            'ё' | 'э' => Some('е'),
            'й' | 'ы' => Some('и'),
            'ь' | 'ъ' => None,
            c if c.is_alphanumeric() => Some(c),
            _ => None,
        })
        .collect()
}

/// One title in Cyrillic and the other in Latin letters that spell it
/// («ЧЕТЫРЕ УКРАИНКИ» and «ChETYRE UKRAINKI»). Two Latin or two Cyrillic
/// titles are for the caller to compare.
pub fn same_translit(a: &str, b: &str) -> bool {
    let (cy, la) = match (has_cyrillic(a), has_cyrillic(b)) {
        (true, false) => (a, b),
        (false, true) => (b, a),
        _ => return false,
    };
    if !latin_only(la) {
        return false;
    }
    let k = skeleton(cy);
    !k.is_empty() && k == skeleton(&to_cyrillic(la))
}

/// The spelling to show of two spellings of one title: the Cyrillic one
/// when the other is its transliteration, else `current`.
pub fn prefer_cyrillic<'a>(current: &'a str, other: &'a str) -> &'a str {
    if !has_cyrillic(current) && has_cyrillic(other) && same_translit(current, other) {
        other
    } else {
        current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transliterations_are_their_cyrillic_both_ways() {
        for (cy, la) in [
            ("ЧЕТЫРЕ УКРАИНКИ", "ChETYRE UKRAINKI"),
            ("ЛЕГЕНДАРНАЯ ПЫЛЬ", "LEGENDARNAJa PYL'"),
            ("Легендарная пыль", "Legendarnaya pyl"),
            ("Группа крови", "Gruppa krovi"),
            ("Щука", "Shchuka"),
            ("Щука", "Schuka"),
            ("Жить", "Zhit'"),
            ("Хорошо", "Khorosho"),
            ("Хорошо", "Xorosho"),
            ("Цой", "Tsoj"),
            ("Юла", "Yula"),
            ("Юла", "Jula"),
            ("Ёлка", "Yolka"),
            ("Ёлка", "Elka"),
            ("Этой весной", "Etoj vesnoj"),
            ("Моргенштерн", "Morgenshtern"),
        ] {
            assert!(same_translit(cy, la), "{cy} / {la}");
            assert!(same_translit(la, cy), "{la} / {cy}");
        }
    }

    #[test]
    fn other_words_and_same_scripts_are_not() {
        assert!(!same_translit("ЧЕТЫРЕ УКРАИНКИ", "TRI UKRAINKI"));
        assert!(!same_translit("Группа крови", "Blood Type"));
        assert!(!same_translit("Gruppa krovi", "Gruppa krovi"), "two Latin titles");
        assert!(!same_translit("Кино", "Кино"));
        assert!(!same_translit("夏子", "Natsuko"));
        assert!(!same_translit("Группа крови", ""));
    }

    #[test]
    fn the_cyrillic_spelling_is_shown() {
        assert_eq!(prefer_cyrillic("ChETYRE UKRAINKI", "ЧЕТЫРЕ УКРАИНКИ"), "ЧЕТЫРЕ УКРАИНКИ");
        assert_eq!(prefer_cyrillic("ЧЕТЫРЕ УКРАИНКИ", "ChETYRE UKRAINKI"), "ЧЕТЫРЕ УКРАИНКИ");
        assert_eq!(prefer_cyrillic("Blood Type", "Группа крови"), "Blood Type", "a translation stays");
    }
}

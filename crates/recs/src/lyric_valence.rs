//! A soft lyrical axis of a song's mood (docs/recommendations.md §6): the
//! sign of the feeling of its words, `lyric_valence ∈ [-1, +1]`, from a
//! small local lexicon, so a sad text over a fast dancing beat is told from
//! a really happy song.
//!
//! * Only for tracks whose text is in the cache already: a track without one
//!   (every new find from the services) is exactly neutral, 0.0, and so
//!   competes on equal terms. Never a filter, never a penalty.
//! * The lexicon is word stems of English and Russian; a text in another
//!   language finds too few of them and stays neutral too. A negation just
//!   before a word ("not", "never", «не», «нет», «без») turns it over.
//! * A text needs [`MIN_HITS`] feeling words to move off 0; then the value
//!   is the balance of the positive and the negative ones, shrunk towards 0
//!   while there are few.

/// Feeling words a text needs before its valence leaves 0.
pub const MIN_HITS: usize = 3;
/// Pseudo-count that shrinks the valence of a text with few feeling words.
const PRIOR: f32 = 4.0;

/// Stems of positive words (matched at a word's start).
const POSITIVE: &[&str] = &[
    // English.
    "love", "loving", "lovely", "happy", "happi", "joy", "smil", "laugh", "sunshine", "sunny", "beautiful", "shine", "shining",
    "dance", "dancing", "celebrat", "free", "freedom", "hope", "dream", "sweet", "kiss", "heaven", "alive", "glad", "together",
    "bright", "warm", "fun", "party", "good", "best", "wonderful", "magic", "delight", "bliss", "paradise", "embrace",
    // Russian.
    "любл", "любов", "любим", "люби", "счаст", "радост", "радуе", "радуй", "улыб", "смех", "смеш", "солнц", "свет", "красив",
    "танц", "праздник", "свобод", "надежд", "мечт", "нежн", "поцел", "рай", "жив", "вместе", "тепл", "весел", "добр",
    "чудес", "волшеб", "обним", "восторг", "лучш", "сияе", "сиять", "хорош",
];

/// Stems of negative words.
const NEGATIVE: &[&str] = &[
    // English.
    "sad", "cry", "cried", "crying", "tear", "pain", "hurt", "broken", "lonely", "alone", "dark", "death", "dead", "die",
    "dying", "kill", "hate", "fear", "afraid", "lost", "lose", "losing", "goodbye", "cold", "empty", "grief", "sorrow",
    "miser", "blood", "grave", "suffer", "scream", "regret", "sorry", "wound", "bleed", "depress", "nightmare", "darkness",
    // Russian.
    "груст", "печал", "плач", "плак", "слез", "слёз", "боль", "больн", "разбит", "одинок", "тьм", "темн", "тёмн",
    "смерт", "мёртв", "мертв", "умир", "умер", "убий", "убит", "ненавид", "ненавист", "страх", "страш", "боюсь", "потер",
    "прощай", "холод", "пусто", "тоск", "кровь", "крови", "могил", "ад", "страда", "крик", "кричу", "жаль", "рана", "ран",
    "депресс", "кошмар",
];

/// Words that turn the next one over.
const NEGATIONS: &[&str] = &["not", "never", "no", "dont", "don't", "cant", "can't", "won't", "не", "нет", "без", "ни", "никогда"];

/// The valence of a cached text: −1 (sad, dark) to +1 (happy, bright), 0
/// with too few feeling words.
pub fn lyric_valence(text: &str) -> f32 {
    let (mut pos, mut neg) = (0usize, 0usize);
    let mut negated = false;
    for word in text.split(|c: char| !(c.is_alphabetic() || c == '\'')).filter(|w| !w.is_empty()) {
        let w = word.to_lowercase();
        if NEGATIONS.contains(&w.as_str()) {
            negated = true;
            continue;
        }
        // A short word only matches a whole stem («ад», "sad"), never
        // the start of another («адрес» is no hell).
        let hit = |stems: &[&str]| stems.iter().any(|s| if s.chars().count() <= 3 { w == *s } else { w.starts_with(s) });
        let sign = if hit(NEGATIVE) {
            -1
        } else if hit(POSITIVE) {
            1
        } else {
            negated = false;
            continue;
        };
        match (sign > 0) != negated {
            true => pos += 1,
            false => neg += 1,
        }
        negated = false;
    }
    let hits = pos + neg;
    if hits < MIN_HITS {
        return 0.0;
    }
    ((pos as f32 - neg as f32) / (hits as f32 + PRIOR)).clamp(-1.0, 1.0)
}

/// [`lyric_valence`] of a track's cached text, exactly 0.0 without one.
pub fn lyric_valence_of(text: Option<&str>) -> f32 {
    text.map_or(0.0, lyric_valence)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn happy_and_sad_texts_part() {
        let happy = "I love you, we dance in the sunshine, so happy together, sweet dreams and a smile";
        let sad = "I cry alone in the dark, the pain of a broken heart, lonely tears, goodbye";
        assert!(lyric_valence(happy) > 0.4, "{}", lyric_valence(happy));
        assert!(lyric_valence(sad) < -0.4, "{}", lyric_valence(sad));
        let ru_sad = "Я плачу одна, мне так больно и одиноко, слёзы и тоска, холодно и пусто";
        let ru_happy = "Я люблю тебя, мы танцуем, солнце светит, счастье и радость, улыбка";
        assert!(lyric_valence(ru_sad) < -0.4, "{}", lyric_valence(ru_sad));
        assert!(lyric_valence(ru_happy) > 0.4, "{}", lyric_valence(ru_happy));
    }

    /// No text, or too few feeling words, or a language the lexicon does
    /// not know: exactly neutral, so new music is not held back.
    #[test]
    fn without_enough_words_it_is_neutral() {
        assert_eq!(lyric_valence_of(None), 0.0);
        assert_eq!(lyric_valence(""), 0.0);
        assert_eq!(lyric_valence("love"), 0.0);
        assert_eq!(lyric_valence("Das ist ein Lied über den Sommer und die Stadt"), 0.0);
        assert_eq!(lyric_valence("адрес адрес адрес"), 0.0, "a short stem matches whole words only");
    }

    #[test]
    fn a_negation_turns_a_word_over() {
        let v = lyric_valence("I am not happy, not happy, never happy, I don't love");
        assert!(v < 0.0, "{v}");
        let v = lyric_valence("не грусти, не плачь, не бойся, всё будет хорошо");
        assert!(v > 0.0, "{v}");
        assert!((-1.0..=1.0).contains(&lyric_valence(&"love ".repeat(500))));
    }
}

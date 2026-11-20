    use super::*;
    use super::ends::{fair, no_overlap, spread_squeezed};
    use crate::text::can_hold;
    use std::time::Instant;
    use crate::text::Letters;

    /// A vocabulary of «а», «б», «в» and `|`, and emissions that hear the
    /// letters of `heard` (one per frame pair, blanks between).
    fn setup(heard: &str) -> (Vocab, Emissions) {
        let vocab = Vocab::from_json(r#"{"<pad>":0,"|":1,"а":2,"б":3,"в":4}"#, Letters::Native).unwrap();
        let id = |c: char| match c {
            'а' => 2,
            'б' => 3,
            'в' => 4,
            ' ' => 1,
            _ => 0,
        };
        let mut logp = Vec::new();
        let mut push = |best: usize| {
            for i in 0..5 {
                logp.push(if i == best { 0.9f32.ln() } else { 0.025f32.ln() });
            }
        };
        for c in heard.chars() {
            push(0);
            push(id(c));
            push(id(c));
        }
        push(0);
        (vocab, Emissions { logp, vocab: 5, fps: 10.0, blank: 0 })
    }

    fn line(s: f32, t: &str) -> LyricLine {
        LyricLine::new(Duration::from_secs_f32(s), t)
    }

    #[test]
    fn the_right_text_fits_and_gets_word_times() {
        let (v, em) = setup("аб ва ббб");
        let fit = fit_text(&em, &v, &[line(0.0, "аб ва"), line(1.8, "ббб")], false).unwrap();
        assert!(fit.fits(), "{}", fit.score);
        assert_eq!(fit.timing, Timing::None);
        let l = &fit.lines[0];
        assert_eq!(l.words.len(), 2);
        assert_eq!(l.words[0].text, "аб ");
        assert!(l.words[0].start < l.words[1].start);
        assert!(fit.lines[1].start > l.words[1].start);
    }

    #[test]
    fn another_text_does_not_fit() {
        let (v, em) = setup("аааааааааааа");
        let fit = fit_text(&em, &v, &[line(0.0, "бв вб"), line(1.0, "вбв")], false).unwrap();
        assert!(!fit.fits(), "{}", fit.score);
    }

    #[test]
    fn line_times_shifted_by_a_constant_are_found() {
        // Lines heard at 0.1, 1.3 and 2.5 s; the source says 0.6, 1.8, 3.0 s.
        let (v, em) = setup("ааа ббб ввв");
        let lines = [line(0.6, "ааа"), line(1.8, "ббб"), line(3.0, "ввв")];
        let fit = fit_text(&em, &v, &lines, true).unwrap();
        assert_eq!(fit.timing, Timing::Shifted, "{fit:?}");
        assert!((fit.offset - 0.5).abs() < 0.15, "{}", fit.offset);
        // Times all over the place: another version.
        let lines = [line(0.0, "ааа"), line(3.0, "ббб"), line(1.0, "ввв")];
        assert_eq!(fit_text(&em, &v, &lines, true).unwrap().timing, Timing::Elsewhere);
    }

    /// A clip's video plays 41 s of intro before the song: a text timed for
    /// the record is one steady shift of -41 s off, found as such (the
    /// lines then move by it), not as another version.
    #[test]
    fn a_clip_intro_of_41_seconds_is_one_steady_shift() {
        let (v, short) = setup("ааа ббб ввв");
        // 410 frames (41 s at 10 a second) of the intro, then the song.
        let intro = 410 * 5;
        let mut logp = Vec::with_capacity(intro + short.logp.len());
        for _ in 0..410 {
            for i in 0..5 {
                logp.push(if i == 0 { 0.9f32.ln() } else { 0.025f32.ln() });
            }
        }
        logp.extend_from_slice(&short.logp);
        let em = Emissions { logp, vocab: 5, fps: 10.0, blank: 0 };
        // The record's times: heard at 41.1, 42.3 and 43.5 s in the video.
        let lines = [line(0.1, "ааа"), line(1.3, "ббб"), line(2.5, "ввв")];
        let fit = fit_text(&em, &v, &lines, true).unwrap();
        assert!(fit.fits(), "{}", fit.score);
        assert_eq!(fit.timing, Timing::Shifted, "{fit:?}");
        assert!((fit.offset + 41.0).abs() < 0.15, "{}", fit.offset);
    }

    /// Emissions over a 5-letter vocabulary (0 blank) from the best letter
    /// of each frame, at 10 frames a second.
    fn frames(best: &[usize]) -> Emissions {
        let mut logp = Vec::new();
        for &b in best {
            for i in 0..5 {
                logp.push(if i == b { 0.9f32.ln() } else { 0.025f32.ln() });
            }
        }
        Emissions { logp, vocab: 5, fps: 10.0, blank: 0 }
    }

    #[test]
    fn a_foreign_line_is_placed_by_its_own_aligner() {
        // The text's aligner hears «а», an ad-lib it takes for letters
        // (frames 4–5), nothing in the foreign line, then «б»; the other
        // aligner hears «xyz» at frames 10–12.
        let (v, _) = setup("");
        let main = frames(&[0, 2, 0, 0, 4, 4, 0, 0, 0, 0, 0, 0, 0, 0, 3, 3, 0]);
        let other_vocab = Vocab::from_json(r#"{"<pad>":0,"|":1,"x":2,"y":3,"z":4}"#, Letters::Native).unwrap();
        let other = frames(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 3, 4, 0, 0, 0, 0]);
        let lines = [line(0.0, "а"), line(0.0, "xyz"), line(0.0, "б")];
        let alone = fit_text(&main, &v, &lines, false).unwrap();
        assert!(alone.lines[1].start.as_secs_f32() < 0.7, "{:?}", alone.lines[1]);
        let f = Foreign { em: &other, vocab: &other_vocab, lines: &[1] };
        let both = fit_text_with(&main, &v, &[f], &lines, false, None, &Tuning::default()).unwrap();
        let start = both.lines[1].start.as_secs_f32();
        assert!((start - (1.0 - LATE)).abs() < 0.05, "{:?}", both.lines[1]);
        // The verdict stays the text's aligner's.
        assert_eq!(both.score, alone.score);
        assert!(both.lines[2].start > both.lines[1].start);
    }

    /// «M» heard at once, «-iley» dumped at the end of the held stretch:
    /// the dumped letters are spread over it, vowels longer.
    #[test]
    fn collapsed_letters_are_spread_over_the_voiced_stretch() {
        let mut letters = vec![(0, 3), (40, 40), (41, 41), (42, 42), (43, 43)];
        let vowel = [false, true, false, true, true];
        let voiced = vec![true; 50];
        spread_collapsed(&mut letters, &vowel, &voiced);
        assert_eq!(letters[0], (0, 3));
        assert_eq!(letters[1].0, 4);
        assert_eq!(letters[4].1, 43);
        for w in letters.windows(2) {
            assert!(w[1].0 > w[0].1 && w[1].0 == w[0].1 + 1, "{letters:?}");
        }
        // The vowel «i» takes about twice the consonant «l».
        let len = |s: (usize, usize)| s.1 + 1 - s.0;
        assert!(len(letters[1]) >= 2 * len(letters[2]) - 1, "{letters:?}");
        // Over silence nothing moves (the word's end is the aligner's).
        let mut quiet = vec![(0, 3), (40, 40), (41, 41)];
        spread_collapsed(&mut quiet, &[false, true, false], &[false; 50]);
        assert_eq!(quiet, [(0, 3), (40, 40), (41, 41)]);
    }

    /// «Хитрый Юра» four times: a line the text has twice while the song
    /// sings it three times takes the repeats its source times point to,
    /// the one between goes to no word, and the lines never overlap.
    #[test]
    fn each_repeat_of_a_line_takes_its_own_occurrence() {
        let (v, _) = setup("");
        let mut best = vec![0; 40];
        for (t, l) in [(1, [4, 2]), (10, [2, 3]), (18, [2, 3]), (26, [2, 3]), (34, [4, 2])] {
            best[t..t + 2].copy_from_slice(&l);
        }
        let em = frames(&best);
        let lines = [line(0.1, "ва"), line(1.0, "аб"), line(2.6, "аб"), line(3.4, "ва")];
        let fit = fit_text(&em, &v, &lines, true).unwrap();
        assert_eq!(fit.timing, Timing::Fits, "{fit:?}");
        let starts: Vec<f32> = fit.lines.iter().map(|l| l.start.as_secs_f32() + LATE).collect();
        assert!((starts[1] - 1.0).abs() < 0.05 && (starts[2] - 2.6).abs() < 0.05, "{starts:?}");
        for w in fit.lines.windows(2) {
            assert!(w[0].end.unwrap() <= w[1].start, "{:?}", fit.lines);
        }
    }

    #[test]
    fn hums_and_held_vowels_are_told_from_words() {
        for hum in ["м", "М, ", "мм", "мгм", "угу, ", "а-а", "о-о-о", "Mmm", "hmm"] {
            assert!(is_hum(hum), "{hum}");
        }
        for word in ["а", "о ", "мама", "ура", "a", "oh", "ага", "ой", "ん", "ああ"] {
            assert!(!is_hum(word), "{word}");
        }
    }

    /// Lines end by the time the next one starts.
    #[test]
    fn a_line_never_runs_into_the_next() {
        let w = |a: f32, b: f32| LyricWord { start: Duration::from_secs_f32(a), end: Duration::from_secs_f32(b), text: "x".into() };
        let mut a = line(1.0, "x");
        a.words = vec![w(1.0, 1.5), w(1.5, 3.2)];
        let mut b = line(3.0, "x");
        b.words = vec![w(3.0, 3.5)];
        let mut lines = vec![a, b];
        no_overlap(&mut lines);
        assert_eq!(lines[0].words[1].end, Duration::from_secs(3));
        assert_eq!(lines[0].end, Some(Duration::from_secs(3)));
    }

    /// «В этой траве»: the last line's word ends in the song's silent
    /// tail when that tail is heard as letters; the source's empty end
    /// stamp holds it.
    #[test]
    fn a_last_word_ends_where_the_source_ends_its_line() {
        let (v, _) = setup("");
        // «ав» at 0.2 s, «ба» at 0.8 s, «в» at 1.4 s, silence, then a
        // phantom «а» at 4.1 s.
        let mut best = vec![0, 0, 2, 0, 4, 1, 0, 0, 3, 0, 2, 1, 0, 0, 4];
        best.extend([0; 26]);
        best.extend([2, 2, 0]);
        let em = frames(&best);
        let mut lines = vec![line(0.2, "ав"), line(0.8, "ба"), line(1.4, "ва")];
        let alone = fit_text(&em, &v, &lines, true).unwrap();
        assert!(matches!(alone.timing, Timing::Fits | Timing::Shifted), "{alone:?}");
        assert!(alone.lines[2].words[0].end.as_secs_f32() > 4.0, "without an end it runs to the tail: {:?}", alone.lines[2]);
        // The empty end stamp at 1.7 s.
        lines.push(line(1.7, ""));
        let held = fit_text(&em, &v, &lines, true).unwrap();
        let end = held.lines[2].words[0].end.as_secs_f32();
        assert!(end <= 1.7 + OVERRUN + 0.01, "{:?}", held.lines[2]);
        assert_eq!(held.lines[2].end, held.lines[2].words.last().map(|w| w.end));
        // A line's own end holds it just the same.
        let mut own = lines[..3].to_vec();
        own[2].end = Some(Duration::from_secs_f32(1.8));
        assert!(fit_text(&em, &v, &own, true).unwrap().lines[2].words[0].end.as_secs_f32() <= 1.8 + OVERRUN + 0.01);
        // A plain text has no times of its own: nothing holds it here.
        let plain: Vec<LyricLine> = lines[..3].iter().map(|l| line(0.0, &l.text)).collect();
        assert_eq!(fit_text(&em, &v, &plain, false).unwrap().timing, Timing::None);
    }

    #[test]
    fn source_ends_take_the_line_s_end_or_the_next_start() {
        let mut ls = vec![line(1.0, "а"), line(3.0, "б"), line(5.0, "в"), line(9.0, "")];
        ls[1].end = Some(Duration::from_secs(4));
        let mut bg = line(3.5, "о");
        bg.background = true;
        ls.insert(2, bg);
        let ends = source_ends(&ls, 0.5);
        assert_eq!(ends, [Some(2.5), Some(3.5), None, Some(8.5), None]);
    }

    /// Where the voice ends, a word ends: a sung held note stays long, a
    /// word that runs on into silence stops a moment after the voice.
    #[test]
    fn a_word_ends_with_the_voice_but_a_held_note_stays() {
        let w = |a: f32, b: f32, t: &str| LyricWord { start: Duration::from_secs_f32(a), end: Duration::from_secs_f32(b), text: t.into() };
        let mut held = line(1.0, "о-о-о");
        held.words = vec![w(1.0, 5.0, "о-о-о")];
        let mut tail = line(6.0, "ха");
        tail.words = vec![w(6.0, 6.2, "х"), w(6.2, 16.0, "а")];
        let mut short = line(17.0, "да");
        short.words = vec![w(17.0, 17.9, "да")];
        let mut lines = vec![held, tail, short];
        // Voice from 1 to 5 s, 6 to 6.8 s, 17 to 17.5 s (tenths).
        let voiced: Vec<bool> = (0..200).map(|i| (10..50).contains(&i) || (60..68).contains(&i) || (170..175).contains(&i)).collect();
        let v = VoiceEnds { voiced: &voiced, step: 0.1, tail: TAIL, level: None, tuning: Tuning::align4() };
        end_with_voice(&mut lines, v, &[None; 3]);
        assert!((lines[0].words[0].end.as_secs_f32() - 5.0).abs() < 1e-3, "held: {:?}", lines[0].words);
        let end = lines[1].words[1].end.as_secs_f32();
        assert!((end - 6.9).abs() < 0.01, "{end}");
        assert_eq!(lines[1].end, Some(lines[1].words[1].end));
        // 0.4 s of no voice at its end is the vowel going on: kept.
        assert!((lines[2].words[0].end.as_secs_f32() - 17.9).abs() < 1e-3);
    }

    /// «осенний листопад»: «беда» is sung held for seconds, the aligner
    /// hears its letters only at the start (79.88 → 80.16) and the line's
    /// source end is 85.29. The word goes on while the voice holds it near
    /// the line's level, a short dip bridged, and stops where the voice
    /// stops or falls to a reverb tail; never past the source's end of the
    /// line or into the next line.
    #[test]
    fn a_held_last_word_lasts_while_the_voice_holds_it() {
        let w = |a: f32, b: f32, t: &str| LyricWord { start: Duration::from_secs_f32(a), end: Duration::from_secs_f32(b), text: t.into() };
        let line_of = |words: Vec<LyricWord>| {
            let mut l = line(words[0].start.as_secs_f32(), "x");
            l.end = words.last().map(|w| w.end);
            l.words = words;
            l
        };
        let lines = vec![
            line_of(vec![w(78.6, 79.4, "Знаешь, "), w(79.4, 79.88, "не "), w(79.88, 80.16, "беда")]),
            line_of(vec![w(85.3, 86.0, "Если")]),
        ];
        // Sung at 0.9 from 78.6 to 83.4 s with a dip at 81.0 s, a reverb
        // tail at 0.4 to 84.5 s, the next line from 85.3 s.
        let level: Vec<f32> = (0..900)
            .map(|i| match i {
                810 => 0.3,
                786..834 => 0.9,
                834..845 => 0.4,
                853.. => 0.9,
                _ => 0.0,
            })
            .collect();
        let voiced: Vec<bool> = level.iter().map(|e| *e > 0.05).collect();
        let v = VoiceEnds { voiced: &voiced, step: 0.1, tail: TAIL, level: Some(&level), tuning: Tuning::align4() };
        let mut held = lines.clone();
        end_with_voice(&mut held, v, &[Some(85.29), None]);
        let end = held[0].words[2].end.as_secs_f32();
        assert!((end - 83.4).abs() < 0.01, "{end}");
        assert_eq!(held[0].end, Some(held[0].words[2].end), "the line ends with the held word");
        assert!((held[0].words[0].end.as_secs_f32() - 79.4).abs() < 1e-3, "a word followed by the next keeps its end");
        // Sung on to the next line: the source's end of the line holds it.
        let on: Vec<f32> = (0..900).map(|i| if i >= 786 { 0.9 } else { 0.0 }).collect();
        let v_on = VoiceEnds { level: Some(&on), ..v };
        let mut long = lines.clone();
        end_with_voice(&mut long, v_on, &[Some(85.0), None]);
        assert!((long[0].words[2].end.as_secs_f32() - 85.0).abs() < 0.01);
        // … else the next line's start.
        let mut next = lines.clone();
        end_with_voice(&mut next, v_on, &[Some(90.0), None]);
        assert!((next[0].words[2].end.as_secs_f32() - 85.3).abs() < 0.01);
        // On the mix nothing is held.
        let mut mix = lines.clone();
        end_with_voice(&mut mix, VoiceEnds { level: None, ..v }, &[Some(85.29), None]);
        assert!((mix[0].words[2].end.as_secs_f32() - 80.16).abs() < 1e-3);
        // Without a source end, at most HOLD longer (and the last line too).
        let mut last = vec![lines[0].clone()];
        end_with_voice(&mut last, v_on, &[None]);
        assert!((last[0].words[2].end.as_secs_f32() - (80.16 + HOLD)).abs() < 0.01);
    }

    /// «Miley Cyrus»: the last word of a line squeezed to nothing where the
    /// next line starts takes a fair share from the word before it.
    #[test]
    fn a_squeezed_word_gets_a_fair_share() {
        let w = |a: f32, b: f32, t: &str| LyricWord { start: Duration::from_secs_f32(a), end: Duration::from_secs_f32(b), text: t.into() };
        let mut a = line(123.71, "Miley Cyrus");
        a.words = vec![w(123.71, 124.50, "Miley "), w(125.76, 125.76, "Cyrus")];
        let mut b = line(125.74, "Слышишь");
        b.words = vec![w(125.74, 126.44, "Слышишь")];
        let mut lines = vec![a, b];
        spread_squeezed(&mut lines);
        let cyrus = &lines[0].words[1];
        assert!((cyrus.end.as_secs_f32() - 125.74).abs() < 0.01, "{cyrus:?}");
        assert!((cyrus.end - cyrus.start).as_secs_f32() >= fair("Cyrus") - 0.01, "{cyrus:?}");
        assert!(lines[0].words[0].end <= cyrus.start);
        // A word of fair length stays.
        let mut c = line(1.0, "да нет");
        c.words = vec![w(1.0, 1.5, "да "), w(1.5, 1.9, "нет")];
        c.end = Some(Duration::from_secs_f32(1.9));
        let mut keep = vec![c.clone()];
        spread_squeezed(&mut keep);
        assert_eq!(keep[0], c);
        // The player's timing does it.
        assert!(Tuning::default().squeezed);
    }

    /// A held note is a vowel: a word that ends in a stop is not held on.
    #[test]
    fn a_word_ending_in_a_stop_is_not_held() {
        let w = |a: f32, b: f32, t: &str| LyricWord { start: Duration::from_secs_f32(a), end: Duration::from_secs_f32(b), text: t.into() };
        let mut l = line(1.0, "хватит");
        l.words = vec![w(1.0, 1.6, "хватит")];
        l.end = Some(Duration::from_secs_f32(1.6));
        let level: Vec<f32> = (0..60).map(|i| if (10..50).contains(&i) { 0.9 } else { 0.0 }).collect();
        let voiced: Vec<bool> = level.iter().map(|e| *e > 0.05).collect();
        let v = VoiceEnds { voiced: &voiced, step: 0.1, tail: TAIL, level: Some(&level), tuning: Tuning::align4() };
        let mut held = vec![l.clone()];
        end_with_voice(&mut held, v, &[Some(5.0)]);
        assert!((held[0].words[0].end.as_secs_f32() - 5.0).abs() < 0.01);
        let mut stopped = vec![l];
        end_with_voice(&mut stopped, VoiceEnds { tuning: Tuning { stop_hold: 0.2, ..Tuning::align4() }, ..v }, &[Some(5.0)]);
        assert!((stopped[0].words[0].end.as_secs_f32() - 1.8).abs() < 0.01, "{:?}", stopped[0].words);
        assert!(can_hold("Sykes?!") && !can_hold("хватит") && !can_hold("crack,"));
    }

    /// «Дави на газ, [pause] дави на газ»: a word before a comma ends with
    /// its voice after a shorter silence than elsewhere; a word without one,
    /// and the last word of a line, keep the usual tail.
    /// «…oh, that's fine» and a long tail of the voice at a third of the
    /// line's level under the instrumental: with `end_level` the last word
    /// ends where the voice falls under half the line's level; a word
    /// inside the line keeps the usual rule.
    #[test]
    fn a_last_word_ends_where_its_voice_falls_to_a_tail() {
        let w = |a: f32, b: f32, t: &str| LyricWord { start: Duration::from_secs_f32(a), end: Duration::from_secs_f32(b), text: t.into() };
        let mut l = line(1.0, "that's fine");
        l.words = vec![w(1.0, 1.5, "that's "), w(1.5, 3.0, "fine")];
        // The line at 1.0 from 1.0 to 2.0 s, then 0.3 to 3.0 s, then nothing.
        let level: Vec<f32> = (0..40).map(|i| if (10..20).contains(&i) { 1.0 } else if (20..30).contains(&i) { 0.3 } else { 0.0 }).collect();
        let voiced: Vec<bool> = level.iter().map(|e| *e > 0.05).collect();
        let ends = |tuning: Tuning| {
            let mut lines = vec![l.clone()];
            end_with_voice(&mut lines, VoiceEnds { voiced: &voiced, step: 0.1, tail: TAIL, level: Some(&level), tuning }, &[Some(3.0)]);
            lines[0].words.iter().map(|w| w.end.as_secs_f32()).collect::<Vec<_>>()
        };
        assert!((ends(Tuning::align4())[1] - 3.0).abs() < 0.01);
        let e = ends(Tuning { end_level: 0.5, ..Tuning::align4() });
        assert!((e[1] - 2.1).abs() < 0.01, "{e:?}");
        assert!((e[0] - 1.5).abs() < 0.01, "{e:?}");
    }

    #[test]
    fn a_word_before_a_comma_ends_with_its_breath() {
        let w = |a: f32, b: f32, t: &str| LyricWord { start: Duration::from_secs_f32(a), end: Duration::from_secs_f32(b), text: t.into() };
        let mut l = line(1.0, "газ, дави газ газ,");
        l.words = vec![w(1.0, 1.9, "газ, "), w(1.9, 2.3, "дави "), w(3.0, 3.9, "газ "), w(4.0, 4.9, "газ,")];
        // Voice at 1.0–1.4, 1.9–2.3, 3.0–3.4 and 4.0–4.4 s (tenths).
        let voiced: Vec<bool> = (0..60).map(|i| [10, 19, 30, 40].iter().any(|a| (*a..*a + 4).contains(&i))).collect();
        let tuning = Tuning { comma_ends: true, ..Tuning::align4() };
        let mut lines = vec![l.clone()];
        end_with_voice(&mut lines, VoiceEnds { voiced: &voiced, step: 0.1, tail: TAIL, level: None, tuning }, &[None]);
        let ends: Vec<f32> = lines[0].words.iter().map(|w| w.end.as_secs_f32()).collect();
        assert!((ends[0] - 1.5).abs() < 0.01, "{ends:?}");
        assert!((ends[2] - 3.9).abs() < 0.01, "{ends:?}");
        assert!((ends[3] - 4.9).abs() < 0.01, "{ends:?}");
        let mut lines = vec![l];
        end_with_voice(&mut lines, VoiceEnds { voiced: &voiced, step: 0.1, tail: TAIL, level: None, tuning: Tuning::align4() }, &[None]);
        assert!((lines[0].words[0].end.as_secs_f32() - 1.9).abs() < 0.01);
    }

    #[test]
    fn a_lone_latin_word_inside_a_cyrillic_line_is_found() {
        let inside = |t: &str| latin_inside(&pieces(t), true, false);
        assert_eq!(inside("Кто раздаёт вам swag? (Ла-ла-ла-ла)"), [false, false, false, true, false]);
        assert_eq!(inside("Я будто тролль warlord"), [false; 4]);
        assert_eq!(inside("Джинсовка Levi's, йоу!"), [false, true, false]);
        assert_eq!(inside("Totally Spies, йоу!"), [false; 3]);
        let runs = |t: &str| latin_inside(&pieces(t), false, true);
        assert_eq!(runs("Грёбаный сукин сын Oliver Sykes?!"), [false, false, false, true, true]);
        assert_eq!(runs("Я будто тролль warlord"), [false; 4]);
        assert_eq!(runs("Кто раздаёт вам swag? (Ла-ла-ла-ла)"), [false; 5]);
        // A word in syllables is one word.
        let glued: Vec<String> = ["Я ", "будто ", "тролль ", "war", "lord"].iter().map(|s| s.to_string()).collect();
        assert_eq!(latin_inside(&glued, true, true), [false; 5]);
    }

    /// «Это нон-стоп» then twelve seconds of instrumental: the next line's
    /// start is no end of this one.
    #[test]
    fn a_line_ends_at_the_next_start_only_without_its_own_end() {
        let mut ls = vec![line(1.0, "а"), line(3.0, "б"), line(15.0, "в")];
        ls[1].end = Some(Duration::from_secs(4));
        assert_eq!(ends_from_next(&ls), [true, false, true]);
    }

    /// Eight «Miley Cyrus» in a row: one taking two sung repeats pulls each
    /// towards an even share of the stretch they took.
    #[test]
    fn lines_sung_alike_are_pulled_to_even_shares() {
        use crate::ctc::{Path, Tok, TokenSpan};
        let lines: Vec<LyricLine> = (0..4).map(|i| line(i as f32, "Miley Cyrus")).collect();
        // One token a line: lines at frames 0, 10, 30 (two repeats), 35.
        let placed: Vec<Placed> = (0..4).map(|li| Placed { line: li, text: "x".into(), say: "x".into(), span: None, peak: 0.0 }).collect();
        let mut path = Path::default();
        for _ in 0..4 {
            path.push(Tok::Letter(1), None);
        }
        let heads: Vec<(usize, usize)> = (0..4).map(|i| (i, i)).collect();
        let owner: Vec<Option<usize>> = (0..4).map(Some).collect();
        let span = |a: usize, b: usize| Some(TokenSpan { first: a, last: b, peak: 1.0 });
        let spans = vec![span(0, 8), span(10, 28), span(30, 33), span(35, 39)];
        assert!(even_repeats(&lines, &heads, &owner, &placed, &spans, &mut path, 3.0, false));
        assert_eq!(path.at, [Some(0), Some(10), Some(20), Some(30)]);
        assert_eq!(path.pull[2], 3.0);
        // Even already, or kept by source times: left alone.
        let even = vec![span(0, 8), span(10, 18), span(20, 28), span(30, 39)];
        let mut p2 = path.clone();
        p2.at = vec![None; 4];
        assert!(!even_repeats(&lines, &heads, &owner, &placed, &even, &mut p2, 3.0, false));
        assert!(!even_repeats(&lines, &heads, &owner, &placed, &spans, &mut p2, 3.0, true));
    }

    #[test]
    fn the_repeat_search_out_of_time_keeps_what_it_has() {
        // A line heard twice, written once: the second time is voice no
        // word covers. With no time left the search stops at once.
        let (v, em) = setup("абвабвабв абвабвабв");
        let lines = [line(0.0, "абвабвабв")];
        let placed = vec![Placed { line: 0, text: "абвабвабв".into(), say: "абвабвабв".into(), span: Some((0.0, 2.8)), peak: 0.9 }];
        let voiced = vec![true; em.frames()];
        let started = Instant::now();
        let out = repeats(&em, &v, &lines, &placed, &voiced, &[false], Duration::ZERO);
        assert!(out.is_empty());
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    /// Every pass of the fit over `lines`: timed or not, with the voice
    /// (the repeat search, voice-held words) and without.
    fn fit_all_ways(em: &Emissions, v: &Vocab, lines: &[LyricLine]) -> Vec<Option<Fit>> {
        let voiced = vec![true; em.frames()];
        let voice = Voice { voiced: &voiced, ..Default::default() };
        let mut out = Vec::new();
        for timed in [false, true] {
            out.push(fit_text(em, v, lines, timed));
            out.push(fit_text_with(em, v, &[], lines, timed, Some(voice), &Tuning::default()));
        }
        out
    }

    #[test]
    fn lyrics_longer_than_the_audio_do_not_fit_and_do_not_panic() {
        let (v, em) = setup("аб");
        let lines: Vec<LyricLine> = (0..12).map(|i| line(i as f32 * 0.5, "абв вба ббб")).collect();
        for fit in fit_all_ways(&em, &v, &lines) {
            assert!(fit.as_ref().is_none_or(|f| !f.fits()), "{fit:?}");
        }
        // No audio at all.
        let none = Emissions { logp: Vec::new(), vocab: 5, fps: 10.0, blank: 0 };
        for fit in fit_all_ways(&none, &v, &lines) {
            assert!(fit.as_ref().is_none_or(|f| !f.fits()), "{fit:?}");
        }
    }

    #[test]
    fn an_empty_line_keeps_its_place_without_words() {
        let (v, em) = setup("ааа ббб ввв ааа");
        let lines = [line(0.1, "ааа"), line(0.6, ""), line(1.3, "ббб"), line(2.5, "ввв"), line(2.9, "  "), line(3.7, "ааа")];
        for fit in fit_all_ways(&em, &v, &lines) {
            let fit = fit.expect("the text fits");
            assert!(fit.lines.len() >= lines.len(), "{:?}", fit.lines);
            assert!(fit.lines.iter().filter(|l| l.text.trim().is_empty()).all(|l| l.words.is_empty()), "{:?}", fit.lines);
        }
    }

    #[test]
    fn a_line_with_no_audio_under_it_does_not_panic() {
        // The last line's source time is long past the recording's end, and
        // its letters are heard nowhere.
        let (v, em) = setup("ааа ббб ввв");
        let lines = [line(0.1, "ааа"), line(1.3, "ббб"), line(2.5, "ввв"), line(60.0, "вв"), line(61.0, "б")];
        for fit in fit_all_ways(&em, &v, &lines) {
            let fit = fit.expect("a fit");
            assert!(fit.lines.len() >= lines.len());
            for l in &fit.lines {
                for w in &l.words {
                    assert!(w.start <= w.end || w.end.is_zero(), "{l:?}");
                }
            }
        }
    }

    /// Letters out of order (the run ends before the letter before it) are
    /// left as they are.
    #[test]
    fn letters_out_of_order_are_not_spread() {
        let mut letters = vec![(0, 30), (45, 45), (2, 2)];
        spread_collapsed(&mut letters, &[false, true, false], &[true; 50]);
        assert_eq!(letters, [(0, 30), (45, 45), (2, 2)]);
    }

    #[test]
    fn plain_text_lines_drop_labels() {
        let l = plain_lines("[Куплет 1]\nРаз два\n\n  три  \n[Припев]");
        assert_eq!(l.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["Раз два", "три"]);
    }

    use std::time::Duration;

    use super::*;

    fn line(ms: u64, text: &str) -> LyricLine {
        LyricLine::new(Duration::from_millis(ms), text)
    }

    fn worded() -> Text {
        let mut l = line(1000, "a");
        l.words.push(nikgob_core::lyrics::LyricWord {
            start: Duration::from_millis(1000),
            end: Duration::from_millis(1500),
            text: "a".into(),
        });
        Text { lines: vec![l], plain: None, weak: false }
    }

    #[test]
    fn eviction_drops_the_oldest_idle_lyrics_down_to_the_cap() {
        let ids: Vec<TrackId> = (0..=6).map(|_| TrackId::new()).collect();
        let id = |n: usize| ids[n];
        let mut order: VecDeque<TrackId> = (1..=6).map(id).collect();
        // 1 and 3 are busy (in use, a lookup on its way): they stay.
        let gone = evict(&mut order, 3, |t| t == id(1) || t == id(3));
        assert_eq!(gone, vec![id(2), id(4), id(5)]);
        assert_eq!(order, VecDeque::from(vec![id(1), id(3), id(6)]));
        // Everything busy: over the cap rather than losing a result.
        let gone = evict(&mut order, 1, |_| true);
        assert!(gone.is_empty());
        assert_eq!(order.len(), 3);
        assert!(evict(&mut order, 5, |_| false).is_empty());
    }

    /// Lyrics shown again are the last to go: the cache forgets the least
    /// recently used, not the least recently stored.
    #[test]
    fn lyrics_used_again_are_forgotten_last() {
        let ids: Vec<TrackId> = (0..4).map(|_| TrackId::new()).collect();
        let mut order: VecDeque<TrackId> = ids.iter().copied().collect();
        touch(&mut order, ids[0]);
        assert_eq!(order, VecDeque::from(vec![ids[1], ids[2], ids[3], ids[0]]));
        assert_eq!(evict(&mut order, 2, |_| false), vec![ids[1], ids[2]]);
        touch(&mut order, ids[1]);
        assert_eq!(order, VecDeque::from(vec![ids[3], ids[0], ids[1]]));
    }

    #[test]
    fn an_entry_is_counted_by_its_texts_and_words() {
        let empty = TrackLyrics::default().approx_bytes();
        let mut tl = TrackLyrics::default();
        tl.insert(LyricsSource::Lrclib, Text::from_lrc(Some("[00:01.00]one line\n[00:02.00]another line"), None));
        let lines = tl.approx_bytes();
        assert!(lines > empty + 2 * std::mem::size_of::<LyricLine>(), "{empty} → {lines}");
        let mut words = tl.clone();
        let text = words.texts.get_mut(&LyricsSource::Lrclib).unwrap();
        text.lines[0].words = vec![nikgob_core::lyrics::LyricWord { text: "one ".into(), ..Default::default() }; 50];
        assert!(words.approx_bytes() > lines + 50 * 4);
        let cache = HashMap::from([(TrackId::new(), tl), (TrackId::new(), words)]);
        note_memory(&cache);
    }

    #[test]
    fn best_text_wins_unless_picked() {
        let id = TrackId::new();
        let mut tl = TrackLyrics::default();
        tl.pending.insert(LyricsSource::LyricsPlus);
        tl.insert(LyricsSource::Lrclib, Text::from_lrc(Some("[00:01.00]a"), None));
        let v = tl.view(id);
        assert_eq!(v.source, Some(LyricsSource::Lrclib));
        assert!(!v.loading);
        assert_eq!(v.sources.len(), 2);

        tl.insert(LyricsSource::LyricsPlus, Some(worded()));
        assert_eq!(tl.view(id).source, Some(LyricsSource::LyricsPlus), "word sync beats line sync");

        tl.pick = Some(LyricsSource::Lrclib);
        let v = tl.view(id);
        assert_eq!(v.source, Some(LyricsSource::Lrclib));
        assert!(v.picked);

        // A pick whose text is gone falls back to the automatic choice.
        tl.pick = Some(LyricsSource::File);
        assert_eq!(tl.view(id).source, Some(LyricsSource::LyricsPlus));
    }

    /// «Take Me to the Beach»: LyricsPlus' first line is its songwriters.
    #[test]
    fn a_songwriters_line_is_not_shown_as_a_lyric() {
        let mut tl = TrackLyrics { names: vec!["Imagine Dragons".into()], ..TrackLyrics::default() };
        let credits = "Ben McKee/Dan Reynolds/Mattias Larsson/Robin Lennart Fredriksson/Wayne Sermon/Matteo Professione";
        let text = Text::from_lines(vec![line(2246, credits), line(9435, "People pleasing planet"), line(11436, "Got a million people saying how to plan it")]);
        tl.insert(LyricsSource::LyricsPlus, text);
        let shown = &tl.texts[&LyricsSource::LyricsPlus];
        assert_eq!(shown.lines.len(), 2);
        assert_eq!(shown.lines[0].text, "People pleasing planet");
        // A Genius text's credits line too.
        tl.insert(LyricsSource::Genius, Text::from_plain("Written by: Dan Reynolds
People pleasing planet"));
        assert_eq!(tl.texts[&LyricsSource::Genius].plain.as_deref(), Some("People pleasing planet"));
    }

    /// The user's own text: shown first over a finer found one, never
    /// replaced by a source that answers again (a refetch), never sent to
    /// the aligner by itself, and nothing else is saved over it; a pick
    /// still shows another text, and timing made of it on asking is shown
    /// and kept as theirs.
    #[test]
    fn the_own_text_comes_first_and_a_refetch_never_replaces_it() {
        use nikgob_align::fit::Timing;
        let id = TrackId::new();
        let mut tl = TrackLyrics { place: Place::File, ..TrackLyrics::default() };
        tl.insert(LyricsSource::LyricsPlus, Some(worded()));
        tl.insert(LyricsSource::Own, Text::from_lrc(Some("[00:02.00]Своя строка\n[00:04.00]Written by: me"), None));
        assert_eq!(tl.chosen(), Some(LyricsSource::Own), "over a word-synced source");
        assert_eq!(tl.texts_of(LyricsSource::Own).unwrap().lines.len(), 2, "kept as written");
        tl.search_again();
        tl.found(LyricsSource::LyricsPlus, Some(worded()));
        tl.found(LyricsSource::LrcRed, None);
        let v = tl.view(id);
        assert_eq!(v.source, Some(LyricsSource::Own));
        assert_eq!(v.lines[0].text, "Своя строка");
        assert!(!v.picked, "chosen, not picked");
        assert_eq!(v.home, v::LyricsHome::Computer);
        assert!(!v.savable, "on the computer already");
        assert!(v.sources.iter().any(|o| o.source == LyricsSource::Own));
        assert!(tl.unchecked().iter().all(|t| t.source != LyricsSource::Own), "never checked by itself");
        // Another source picked: shown, but never saved over the own text.
        tl.pick = Some(LyricsSource::LyricsPlus);
        let v = tl.view(id);
        assert_eq!(v.source, Some(LyricsSource::LyricsPlus));
        assert!(!v.savable);
        tl.pick = None;
        // Word timing made of it on asking: shown, and kept as theirs.
        let hash = tl.texts_of(LyricsSource::Own).unwrap().hash();
        tl.set_aligned(
            LyricsSource::Own,
            Aligned {
                hash,
                fits: true,
                timing: Timing::None,
                offset: Duration::ZERO,
                offset_early: false,
                covered: 1.0,
                lines: worded().lines,
                saved: true,
                current: true,
            },
        );
        let v = tl.view(id);
        assert!(v.made && v.savable);
        assert_eq!(v.source, Some(LyricsSource::Own));
        // «Вернуть найденный»: the found texts again.
        tl.pick = Some(LyricsSource::Own);
        tl.forget(LyricsSource::Own);
        assert_eq!(tl.picked(), None);
        assert_eq!(tl.view(id).source, Some(LyricsSource::LyricsPlus));
        assert!(!tl.view(id).sources.iter().any(|o| o.source == LyricsSource::Own));
    }

    #[test]
    fn syllable_sync_beats_word_sync_even_at_a_lower_source_priority() {
        let mut tl = TrackLyrics::default();
        tl.insert(LyricsSource::LyricsPlus, Some(worded()));
        let mut text = worded();
        text.lines[0].text = "ab".into();
        let mut piece = text.lines[0].words[0].clone();
        piece.text = "b".into();
        text.lines[0].words.push(piece);
        tl.insert(LyricsSource::Lrclib, Some(text));
        let view = tl.view(TrackId::new());
        assert_eq!(view.source, Some(LyricsSource::Lrclib));
        assert!(view.sources.iter().any(|o| o.state == v::LyricsAvail::Ready(LyricsSync::Syllable)));
    }

    #[test]
    fn written_lrc_is_not_a_second_source_and_misses_are_listed() {
        let id = TrackId::new();
        let mut tl = TrackLyrics::default();
        tl.insert(LyricsSource::File, Text::from_lrc(Some("[00:01.00]a"), None));
        tl.insert(LyricsSource::Lrclib, Text::from_lrc(Some("[00:01.00]a"), None));
        tl.insert(LyricsSource::LyricsPlus, None);
        let v = tl.view(id);
        assert_eq!(v.source, Some(LyricsSource::Lrclib));
        assert_eq!(
            v.sources,
            vec![
                v::LyricsOption {
                    source: LyricsSource::LyricsPlus,
                    state: v::LyricsAvail::Missing,
                    fits: None,
                    made: false,
                },
                v::LyricsOption {
                    source: LyricsSource::Lrclib,
                    state: v::LyricsAvail::Ready(LyricsSync::Line),
                    fits: None,
                    made: false,
                },
            ]
        );
    }

    #[test]
    fn plain_text_and_loading() {
        let id = TrackId::new();
        let mut tl = TrackLyrics::default();
        tl.pending.insert(LyricsSource::Lrclib);
        assert!(tl.view(id).loading);
        tl.insert(LyricsSource::Lrclib, Text::from_lrc(None, Some("a\nb")));
        let v = tl.view(id);
        assert_eq!(v.plain.as_deref(), Some("a\nb"));
        assert!(v.lines.is_empty());
        assert!(Text::from_lrc(None, Some("  ")).is_none());
    }

    #[test]
    fn synced_file_wins_plain_file_loses_and_online_ties_follow_priority() {
        let mut tl = TrackLyrics::default();
        tl.insert(LyricsSource::File, Text::from_lrc(None, Some("local")));
        for source in [LyricsSource::Lrclib, LyricsSource::BetterLyrics, LyricsSource::LrcRed, LyricsSource::LyricsPlus] {
            tl.insert(source, Text::from_lrc(Some("[00:02.00]online"), None));
            assert_eq!(tl.chosen(), Some(source));
        }
        tl.insert(LyricsSource::Lrclib, Some(worded()));
        assert_eq!(tl.chosen(), Some(LyricsSource::Lrclib), "word beats line regardless of source");
        tl.insert(LyricsSource::File, Text::from_lrc(Some("[00:03.00]local"), None));
        assert_eq!(tl.chosen(), Some(LyricsSource::File), "synced file beats word-synced online");
        tl.pick = Some(LyricsSource::LyricsPlus);
        assert_eq!(tl.chosen(), tl.pick);
        assert_eq!(LyricsSource::from_key("netease"), None);
        assert_eq!(LyricsSource::from_key("qq"), None);
    }

    #[test]
    fn better_lyrics_requires_two_confirmed_misses_and_lrclib_is_last() {
        let mut tl = TrackLyrics::default();
        tl.pending.extend(LyricsSource::ALL.into_iter().filter(|s| *s != LyricsSource::File));
        assert_eq!(tl.next_lookups(), [LyricsSource::LyricsPlus, LyricsSource::LrcRed]);
        tl.insert(LyricsSource::LyricsPlus, None);
        assert_eq!(tl.next_lookups(), [LyricsSource::LrcRed]);
        tl.insert(LyricsSource::LrcRed, None);
        assert_eq!(tl.next_lookups(), [LyricsSource::BetterLyrics]);
        tl.insert(LyricsSource::BetterLyrics, None);
        assert_eq!(tl.next_lookups(), [LyricsSource::Lrclib]);

        for got in [Some(worded()), None] {
            let mut tl = TrackLyrics::default();
            tl.pending.extend([LyricsSource::BetterLyrics, LyricsSource::Lrclib]);
            tl.insert(LyricsSource::LyricsPlus, got);
            // lrc.red failed, so it is neither pending nor a confirmed miss.
            tl.failed = true;
            assert_eq!(tl.next_lookups(), [LyricsSource::Lrclib]);
            assert!(!tl.pending.contains(&LyricsSource::BetterLyrics));
        }
    }

    #[test]
    fn embedded_text_deduplicates_every_online_source_but_manual_file_still_works() {
        for source in LyricsSource::ALL.into_iter().filter(|s| *s != LyricsSource::File) {
            let mut tl = TrackLyrics::default();
            let text = worded();
            tl.insert(LyricsSource::File, Text::from_lrc(Some(&text.serialized()), None));
            tl.insert(source, Some(text));
            assert_eq!(tl.chosen(), Some(source));
            assert!(!tl.view(TrackId::new()).sources.iter().any(|s| s.source == LyricsSource::File));
            tl.pick = Some(LyricsSource::File);
            assert_eq!(tl.chosen(), Some(LyricsSource::File));
        }
    }

    #[test]
    fn background_groups_deduplicate_after_lrc_sorts_lines() {
        let mut online = worded();
        let mut background = line(5000, "oh");
        background.background = true;
        online.lines.extend([background, line(3000, "next")]);
        let mut tl = TrackLyrics::default();
        tl.insert(LyricsSource::File, Text::from_lrc(Some(&online.serialized()), None));
        tl.insert(LyricsSource::LyricsPlus, Some(online));
        assert!(tl.duplicate_file());
        assert_eq!(tl.chosen(), Some(LyricsSource::LyricsPlus));
    }

    #[test]
    fn identical_tag_is_not_rewritten() {
        use nikgob_import::{EncodeOptions, Encoder, RopusEncoder};
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("in.wav");
        let path = dir.path().join("song.opus");
        let mut writer = hound::WavWriter::create(&wav, hound::WavSpec {
            channels: 1, sample_rate: 48_000, bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        }).unwrap();
        for _ in 0..48_000 { writer.write_sample(0i16).unwrap(); }
        writer.finalize().unwrap();
        RopusEncoder.encode_to_opus(&wav, &path, &EncodeOptions { bitrate_kbps: 160 }).unwrap();
        let text = worded().serialized();
        embed_text(&path, &text).unwrap();
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        embed_text(&path, &text).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), modified);
    }

    #[test]
    fn a_weak_line_sync_loses_to_any_other_line_sync_but_not_to_plain_text() {
        let lrc = |t: &str| Text::from_lrc(Some(t), None);
        let mut weak = lrc("[00:01.00]a").unwrap();
        weak.weak = true;
        let mut tl = TrackLyrics::default();
        tl.insert(LyricsSource::LyricsPlus, Some(weak.clone()));
        tl.insert(LyricsSource::Lrclib, Text::from_lrc(None, Some("a")));
        assert_eq!(tl.chosen(), Some(LyricsSource::LyricsPlus), "lines beat plain text");
        for other in [LyricsSource::Lrclib, LyricsSource::BetterLyrics, LyricsSource::LrcRed] {
            let mut tl = tl.clone();
            tl.insert(other, lrc("[00:01.50]a"));
            assert_eq!(tl.chosen(), Some(other), "an accurate line sync beats the weak one");
            // Real word sync of LyricsPlus wins again.
            tl.insert(LyricsSource::LyricsPlus, Some(worded()));
            assert_eq!(tl.chosen(), Some(LyricsSource::LyricsPlus));
        }
        // Cached and read back as weak; a normal text stays a plain list.
        let back = stored_lines(&weak.body().unwrap()).unwrap();
        assert!(back.weak);
        assert_eq!(back.lines, weak.lines);
        let normal = worded();
        assert!(normal.body().unwrap().starts_with('['));
        assert!(!stored_lines(&normal.body().unwrap()).unwrap().weak);
    }

    fn made(lines: &[(u64, &str)]) -> Vec<LyricLine> {
        lines
            .iter()
            .map(|(ms, t)| {
                let mut l = line(*ms, t);
                l.words.push(nikgob_core::lyrics::LyricWord {
                    start: Duration::from_millis(*ms),
                    end: Duration::from_millis(ms + 400),
                    text: (*t).to_owned(),
                });
                l.end = Some(Duration::from_millis(ms + 400));
                l
            })
            .collect()
    }

    fn aligned(text: &Text, fits: bool, timing: nikgob_align::fit::Timing, offset_ms: i64, lines: Vec<LyricLine>) -> Aligned {
        Aligned {
            hash: text.hash(),
            fits,
            timing,
            offset: Duration::from_millis(offset_ms.unsigned_abs()),
            offset_early: offset_ms < 0,
            covered: 0.9,
            lines,
            saved: false,
            current: true,
        }
    }

    /// A clip that stays one: its lyrics are asked by its song's title with
    /// no length; the found timing shows no highlighting until the check
    /// heard it, then moves by the steady shift of the video's intro.
    #[test]
    fn a_clip_asks_by_its_song_title_and_waits_for_the_check() {
        use nikgob_align::fit::Timing;
        let clip = TrackRow {
            id: TrackId::new(),
            title: "Starboy ft. Daft Punk (Official Video) (feat. Daft Punk)".into(),
            artist: "The Weeknd".into(),
            album: None,
            release_id: None,
            cover_ref: None,
            disc_no: 0,
            track_no: 0,
            duration_ms: Some(274_000),
            quality: Some(nikgob_core::model::score::STREAM_YOUTUBE_CLIP),
            codec: None,
            active_kind: Some(nikgob_core::model::SourceKind::Youtube),
            has_local: false,
            liked: false,
            in_library: false,
            credit: Default::default(),
            youtube_id: Some("34Na4j8AVgA".into()),
            check: None,
        };
        let asked = lookup_row(clip.clone());
        assert_eq!(asked.title, "Starboy ft. Daft Punk");
        assert_eq!(asked.duration_ms, None, "the video's length is not the song's");
        let song = TrackRow { quality: Some(nikgob_core::model::score::STREAM_YOUTUBE), ..clip.clone() };
        assert_eq!(lookup_row(song.clone()), song, "a song is asked as it is");

        let mut tl = TrackLyrics { clip: true, ..TrackLyrics::default() };
        let lrc = Text::from_lrc(Some("[00:10.00]I'm tryna put you in the worst mood\n[00:14.00]P1 cleaner than your church shoes"), None).unwrap();
        tl.insert(LyricsSource::Lrclib, Some(lrc.clone()));
        let v = tl.view(TrackId::new());
        assert!(v.lines.is_empty(), "no highlighting before the check");
        assert!(v.plain.as_deref().is_some_and(|p| p.contains("worst mood")), "the lines show as text");
        assert_eq!(tl.unchecked().len(), 1);
        // The check heard it 41 s later: shown moved by the intro.
        let heard = made(&[(51_000, "I'm tryna put you in the worst mood"), (55_000, "P1 cleaner than your church shoes")]);
        tl.set_aligned(LyricsSource::Lrclib, aligned(&lrc, true, Timing::Shifted, -41_000, heard));
        let v = tl.view(TrackId::new());
        assert_eq!(v.lines.first().map(|l| l.start), Some(Duration::from_millis(51_000)));
        // Not this song's text: still no highlighting.
        tl.set_aligned(LyricsSource::Lrclib, aligned(&lrc, false, Timing::Elsewhere, 0, Vec::new()));
        assert!(tl.view(TrackId::new()).lines.is_empty());
        // A song plays: its found timing shows at once.
        let mut tl = TrackLyrics::default();
        tl.insert(LyricsSource::Lrclib, Some(lrc));
        assert_eq!(tl.view(TrackId::new()).lines.len(), 2);
    }

    #[test]
    fn a_line_text_of_another_take_is_shown_with_the_timing_of_this_recording() {
        use nikgob_align::fit::Timing;
        // «космоцветок»: LRCLIB's lines are 2.14 s early for this recording.
        let mut tl = TrackLyrics::default();
        let lrc = Text::from_lrc(Some("[00:14.60]Слово за слово\n[00:18.32]Дальше"), None).unwrap();
        tl.insert(LyricsSource::Lrclib, Some(lrc.clone()));
        let v0 = tl.view(TrackId::new());
        assert!(!v0.made);
        assert_eq!(v0.lines[0].start, Duration::from_millis(14_600));
        let retimed = made(&[(16_740, "Слово за слово"), (20_460, "Дальше")]);
        tl.set_aligned(LyricsSource::Lrclib, aligned(&lrc, true, Timing::Shifted, -2140, retimed.clone()));
        let v = tl.view(TrackId::new());
        assert_eq!(v.source, Some(LyricsSource::Lrclib));
        assert!(v.made);
        assert_eq!(v.lines, retimed);
        assert_eq!(v.sources[0].fits, Some(true));
        assert!(v.sources[0].made);
        let t = v.texts.iter().find(|t| t.source == Some(LyricsSource::Lrclib)).unwrap();
        assert_eq!(t.lines, lrc.lines, "the sheet shows the source's own text too");
        assert_eq!(t.made.as_ref(), Some(&retimed));
        // A changed text is not covered by the old check.
        tl.insert(LyricsSource::Lrclib, Text::from_lrc(Some("[00:14.60]Другое"), None));
        assert!(!tl.view(TrackId::new()).made);
        assert_eq!(tl.unchecked().len(), 1);
    }

    #[test]
    fn a_text_that_does_not_fit_loses_to_one_that_does() {
        use nikgob_align::fit::Timing;
        let mut tl = TrackLyrics::default();
        let wrong = Text::from_lrc(Some("[00:01.00]чужие слова"), None).unwrap();
        let genius = Text::from_plain("свои слова").unwrap();
        tl.insert(LyricsSource::Lrclib, Some(wrong.clone()));
        tl.insert(LyricsSource::Genius, Some(genius.clone()));
        assert_eq!(tl.chosen(), Some(LyricsSource::Lrclib), "unchecked: lines beat plain");
        tl.set_aligned(LyricsSource::Lrclib, aligned(&wrong, false, Timing::Elsewhere, 0, Vec::new()));
        assert_eq!(tl.chosen(), Some(LyricsSource::Genius), "plain text that fits beats lines that do not");
        tl.set_aligned(LyricsSource::Genius, aligned(&genius, true, Timing::None, 0, made(&[(3000, "свои слова")])));
        let v = tl.view(TrackId::new());
        assert_eq!(v.source, Some(LyricsSource::Genius));
        assert!(v.made);
        assert_eq!(LyricsSync::of(&v.lines), Some(LyricsSync::Word));
        assert_eq!(v.sources.iter().find(|o| o.source == LyricsSource::Lrclib).unwrap().fits, Some(false));
        // A text heard in the recording loses to any text that fits, but
        // not to one that does not.
        let mut heard = TrackLyrics::default();
        heard.insert(LyricsSource::Recognized, Some(Text::from_lines(made(&[(500, "услышано")])).unwrap()));
        heard.insert(LyricsSource::Lrclib, Some(wrong.clone()));
        assert_eq!(heard.chosen(), Some(LyricsSource::Lrclib));
        heard.set_aligned(LyricsSource::Lrclib, aligned(&wrong, false, Timing::Elsewhere, 0, Vec::new()));
        assert_eq!(heard.chosen(), Some(LyricsSource::Recognized));
        assert!(heard.view(TrackId::new()).made);
    }

    /// Word timing off (the default): a line text keeps its lines (moved
    /// when one shift off, made from the recording when another version's);
    /// a plain text gets made lines, or none with that off too; timing the
    /// user asked for shows whatever the settings.
    #[test]
    fn without_word_timing_lines_stay_lines_and_a_plain_text_gets_made_lines() {
        use nikgob_align::fit::Timing;
        let off = Shown { words: false, lines: true, heard: true };
        let lrc = Text::from_lrc(Some("[00:14.60]Слово за слово\n[00:18.32]Дальше"), None).unwrap();
        let words = made(&[(16_740, "Слово за слово"), (20_460, "Дальше")]);
        for (timing, first, made_flag) in [(Timing::Fits, 14_600, false), (Timing::Shifted, 16_740, false), (Timing::Elsewhere, 16_740, true)] {
            let mut tl = TrackLyrics { show: off, ..Default::default() };
            tl.insert(LyricsSource::Lrclib, Some(lrc.clone()));
            tl.set_aligned(LyricsSource::Lrclib, aligned(&lrc, true, timing, -2140, words.clone()));
            let v = tl.view(TrackId::new());
            assert_eq!(LyricsSync::of(&v.lines), Some(LyricsSync::Line), "{timing:?}");
            assert_eq!(v.lines[0].start, Duration::from_millis(first), "{timing:?}");
            assert_eq!(v.made, made_flag, "{timing:?}");
        }
        let genius = Text::from_plain("Слово за слово\nДальше").unwrap();
        let mut tl = TrackLyrics { show: off, ..Default::default() };
        tl.insert(LyricsSource::Genius, Some(genius.clone()));
        tl.set_aligned(LyricsSource::Genius, aligned(&genius, true, Timing::None, 0, words.clone()));
        let v = tl.view(TrackId::new());
        assert!(v.made);
        assert_eq!(LyricsSync::of(&v.lines), Some(LyricsSync::Line));
        assert_eq!(v.lines[1].start, Duration::from_millis(20_460));
        tl.show.lines = false;
        let v = tl.view(TrackId::new());
        assert!(v.lines.is_empty() && v.plain.is_some() && !v.made);
        // Asked for by the user: shown as made, whatever the settings.
        tl.set_aligned(LyricsSource::Genius, Aligned { saved: true, ..aligned(&genius, true, Timing::None, 0, words.clone()) });
        let v = tl.view(TrackId::new());
        assert!(v.made);
        assert_eq!(v.lines, words);
        // With word timing on, the made words.
        let mut tl = TrackLyrics { show: Shown { words: true, lines: false, heard: true }, ..Default::default() };
        tl.insert(LyricsSource::Genius, Some(genius.clone()));
        tl.set_aligned(LyricsSource::Genius, aligned(&genius, true, Timing::None, 0, words.clone()));
        assert_eq!(tl.view(TrackId::new()).lines, words);
    }

    /// The shown text is seen by words, by lines or without timing as the
    /// user chose, as far as it (or the player's timing of it) goes; the
    /// timing kept and the place it lives do not change with it.
    #[test]
    fn a_text_is_seen_as_chosen_as_far_as_it_has_that_timing() {
        use nikgob_align::fit::Timing;
        let off = Shown { words: false, lines: true, heard: true };
        let lrc = Text::from_lrc(Some("[00:14.60]Слово за слово
[00:18.32]Дальше"), None).unwrap();
        let words = made(&[(14_600, "Слово за слово"), (18_320, "Дальше")]);
        let mut tl = TrackLyrics { show: off, ..Default::default() };
        tl.insert(LyricsSource::Lrclib, Some(lrc.clone()));
        let id = TrackId::new();
        // Lines only: by lines or without timing.
        let v = tl.view(id);
        assert_eq!(v.shows, [LyricsSync::Line, LyricsSync::Plain]);
        tl.display = Some(LyricsSync::Word);
        assert_eq!(tl.view(id).lines, lrc.lines, "no words: as fine as it goes");
        tl.display = Some(LyricsSync::Plain);
        let v = tl.view(id);
        assert!(v.lines.is_empty() && !v.made);
        assert_eq!(v.plain.as_deref(), Some("Слово за слово
Дальше"));
        // The player made words (word timing off in the settings): they
        // can be seen, and lines stay the source's own.
        tl.set_aligned(LyricsSource::Lrclib, aligned(&lrc, true, Timing::Fits, 0, words.clone()));
        tl.display = None;
        let v = tl.view(id);
        assert_eq!(v.lines, lrc.lines);
        assert_eq!(v.shows, [LyricsSync::Word, LyricsSync::Line, LyricsSync::Plain]);
        tl.display = Some(LyricsSync::Word);
        let v = tl.view(id);
        assert_eq!(v.lines, words);
        assert!(v.made);
        // Words by lines: the same lines without the words.
        let mut own = TrackLyrics::default();
        own.insert(LyricsSource::LyricsPlus, Some(worded()));
        own.display = Some(LyricsSync::Line);
        let v = own.view(id);
        assert_eq!(LyricsSync::of(&v.lines), Some(LyricsSync::Line));
        assert_eq!(v.lines[0].start, worded().lines[0].start);
        assert_eq!(v.shows, [LyricsSync::Word, LyricsSync::Line, LyricsSync::Plain]);
        // Made timing kept by the user stays savable whatever is seen.
        let genius = Text::from_plain("Слово за слово
Дальше").unwrap();
        let mut g = TrackLyrics { place: Place::File, ..Default::default() };
        g.insert(LyricsSource::Genius, Some(genius.clone()));
        g.set_aligned(LyricsSource::Genius, Aligned { saved: true, ..aligned(&genius, true, Timing::None, 0, words.clone()) });
        g.display = Some(LyricsSync::Plain);
        let v = g.view(id);
        assert!(v.lines.is_empty() && v.savable && v.home == v::LyricsHome::Library);
        assert_eq!(g.to_keep(LyricsSource::Genius), Some(crate::lyrics_keep::Keep::Lines(words.clone())), "saving keeps the made words");

        // No text: nothing to choose.
        assert!(TrackLyrics::default().view(id).shows.is_empty());
    }

    /// With recognition off, a text heard earlier is never shown: not
    /// chosen, not listed, not even when it was picked.
    #[test]
    fn a_heard_text_is_hidden_while_recognition_is_off() {
        let heard = Text::from_lines(made(&[(500, "услышано")])).unwrap();
        let mut tl = TrackLyrics { show: Shown { heard: false, ..Shown::default() }, ..Default::default() };
        tl.insert(LyricsSource::Recognized, Some(heard));
        tl.pick = Some(LyricsSource::Recognized);
        let v = tl.view(TrackId::new());
        assert_eq!(v.source, None);
        assert!(v.lines.is_empty() && v.sources.is_empty() && v.texts.is_empty());
        // A found text still shows.
        tl.insert(LyricsSource::Genius, Text::from_plain("свои слова"));
        assert_eq!(tl.view(TrackId::new()).source, Some(LyricsSource::Genius));
        // Recognition on again: the heard text is back.
        tl.show.heard = true;
        assert_eq!(tl.view(TrackId::new()).source, Some(LyricsSource::Recognized));
    }

    /// Genius is asked once: a text it gave is kept in the library and read
    /// back from there, never asked for again.
    #[test]
    fn a_stored_genius_text_is_not_asked_for_again() {
        use nikgob_core::model::{ArtistCredit, Track};
        let lib = Library::open_in_memory().unwrap();
        let track = lib.upsert_track(&Track::new("Замок", ArtistCredit::single("nyan.mp3", None))).unwrap().id;
        let fresh = cached_lyrics(&lib, track, None, None, true, None);
        assert!(fresh.pending.contains(&LyricsSource::Genius));
        let body = Text::from_plain("Я невменяемый (Ай)").unwrap().body();
        lib.put_lyrics_text(track, LyricsSource::Genius.key(), body.as_deref()).unwrap();
        let tl = cached_lyrics(&lib, track, None, None, true, None);
        assert!(!tl.pending.contains(&LyricsSource::Genius));
        assert_eq!(tl.texts_of(LyricsSource::Genius).and_then(|t| t.plain.as_deref()), Some("Я невменяемый (Ай)"));
    }

    /// What the library keeps answers for: a found text forever, a miss
    /// for a week; what was never looked up is asked. Keyed by the track's
    /// id, so a retag (same id) keeps it all.
    #[test]
    fn the_cache_answers_with_found_texts_forever_and_with_misses_for_a_week() {
        use nikgob_core::model::{ArtistCredit, Track};
        use nikgob_core::lyrics::LyricsSource::{BetterLyrics, Genius, LrcRed, Lrclib, LyricsPlus};
        let lib = Library::open_in_memory().unwrap();
        let track = lib.upsert_track(&Track::new("Song", ArtistCredit::single("Artist", None))).unwrap().id;
        // Nothing stored: every online source is asked (Genius when on).
        let tl = cached_lyrics(&lib, track, None, None, false, None);
        assert_eq!(tl.pending, HashSet::from([Lrclib, LyricsPlus, LrcRed, BetterLyrics]));
        assert!(cached_lyrics(&lib, track, None, None, true, None).pending.contains(&Genius));
        // LRCLIB and LyricsPlus found a text, lrc.red and Genius did not.
        lib.put_lyrics(track, Some("[00:01.00]a"), None, false).unwrap();
        lib.put_lyrics_text(track, LyricsPlus.key(), worded().body().as_deref()).unwrap();
        lib.put_lyrics_text(track, LrcRed.key(), None).unwrap();
        lib.put_lyrics_text(track, Genius.key(), None).unwrap();
        let now = now_ms();
        let tl = cached_lyrics_at(&lib, track, None, None, true, None, now);
        assert!(tl.texts.contains_key(&Lrclib) && tl.texts.contains_key(&LyricsPlus));
        assert_eq!(tl.missing, HashSet::from([LrcRed, Genius]));
        assert_eq!(tl.pending, HashSet::from([BetterLyrics]), "never looked up");
        // A week on: the misses are asked again, the found texts never.
        let later = now + LYRICS_RETRY_MS + 1;
        let tl = cached_lyrics_at(&lib, track, None, None, true, None, later);
        assert!(tl.texts.contains_key(&Lrclib) && tl.texts.contains_key(&LyricsPlus));
        assert!(tl.missing.is_empty());
        assert_eq!(tl.pending, HashSet::from([LrcRed, BetterLyrics, Genius]));
        // An LRCLIB miss holds the same week.
        let other = lib.upsert_track(&Track::new("Other", ArtistCredit::single("Artist", None))).unwrap().id;
        lib.put_lyrics(other, None, None, false).unwrap();
        let now = now_ms();
        assert!(cached_lyrics_at(&lib, other, None, None, false, None, now).missing.contains(&Lrclib));
        assert!(cached_lyrics_at(&lib, other, None, None, false, None, now + LYRICS_RETRY_MS + 1).pending.contains(&Lrclib));
    }

    /// «Искать текст заново»: every source is asked again in the usual
    /// order; a miss now keeps the text found before, a new text replaces
    /// it, and Genius keeps its gate.
    #[test]
    fn a_search_again_asks_every_source_and_a_miss_keeps_the_text_found_before() {
        use nikgob_core::lyrics::LyricsSource::{BetterLyrics, Genius, LrcRed, Lrclib, LyricsPlus};
        let lrc = Text::from_lrc(Some("[00:01.00]a"), None);
        let mut tl = TrackLyrics { genius: true, ..Default::default() };
        tl.insert(Lrclib, lrc.clone());
        for s in [LyricsPlus, LrcRed, BetterLyrics, Genius] {
            tl.insert(s, None);
        }
        tl.failed = true;
        tl.search_again();
        assert!(!tl.failed);
        assert_eq!(tl.next_lookups(), [LyricsPlus, LrcRed]);
        tl.found(LyricsPlus, None);
        tl.found(LrcRed, None);
        assert_eq!(tl.next_lookups(), [BetterLyrics]);
        tl.found(BetterLyrics, None);
        assert_eq!(tl.next_lookups(), [Lrclib]);
        assert_eq!(tl.view(TrackId::new()).source, Some(Lrclib), "shown while it is asked");
        tl.found(Lrclib, None);
        assert_eq!(tl.texts_of(Lrclib), lrc.as_ref());
        assert!(!tl.missing.contains(&Lrclib));
        // A timed text fits as it is: Genius is not asked.
        assert!(tl.next_lookups().is_empty());
        assert!(tl.pending.is_empty());
        // Asked again, a new text takes the old one's place.
        let newer = Text::from_lrc(Some("[00:02.00]b"), None);
        tl.search_again();
        tl.found(Lrclib, newer.clone());
        assert_eq!(tl.texts_of(Lrclib), newer.as_ref());
        // Off in the settings, Genius is not asked again either.
        let mut off = TrackLyrics::default();
        off.search_again();
        assert!(!off.pending.contains(&Genius));
    }

    /// A Genius miss stored before its current matching is asked again; a
    /// newer one, and any other source's, holds for the usual week.
    #[test]
    fn a_genius_miss_from_older_matching_is_asked_again() {
        let since = nikgob_metadata::genius::MATCHING_SINCE;
        let day = 86_400_000;
        assert!(!miss_holds(LyricsSource::Genius, since - day, since + day));
        assert!(miss_holds(LyricsSource::Genius, since, since + day));
        assert!(miss_holds(LyricsSource::LrcRed, since - day, since + day));
        assert!(!miss_holds(LyricsSource::Genius, since, since + LYRICS_RETRY_MS));
    }

    /// Genius is asked with every credited artist on its own, the main one
    /// first; without a credit, with the shown one.
    #[test]
    fn genius_is_asked_with_each_credited_artist() {
        use nikgob_core::model::{ArtistCredit, ArtistCreditPart, Track};
        let part = |name: &str, join: &str| ArtistCreditPart { artist_id: None, name: name.into(), join_phrase: join.into() };
        let lib = Library::open_in_memory().unwrap();
        let credit = ArtistCredit(vec![part("MellSher", ", "), part("и ещё 1 исполнитель", "")]);
        let track = lib.upsert_track(&Track::new("Разгром", credit)).unwrap().id;
        let mut row = lib.get_track_row(track).unwrap().unwrap();
        assert_eq!(genius_artists(&row), ["MellSher", "и ещё 1 исполнитель"]);
        row.credit = ArtistCredit(Vec::new());
        row.artist = "MellSher, и ещё 1 исполнитель".into();
        assert_eq!(genius_artists(&row), ["MellSher, и ещё 1 исполнитель"]);
    }

    #[test]
    fn own_word_timing_beats_made_timing_and_moves_when_shifted() {
        use nikgob_align::fit::Timing;
        let mut tl = TrackLyrics::default();
        let words = worded();
        tl.insert(LyricsSource::LyricsPlus, Some(words.clone()));
        let lines = Text::from_lrc(Some("[00:01.00]a"), None).unwrap();
        tl.insert(LyricsSource::Lrclib, Some(lines.clone()));
        tl.set_aligned(LyricsSource::Lrclib, aligned(&lines, true, Timing::Fits, 0, made(&[(1000, "a")])));
        assert_eq!(tl.chosen(), Some(LyricsSource::LyricsPlus), "a source's own words beat made ones");
        // One steady shift: the source's own words, moved (here 300 ms late).
        tl.set_aligned(LyricsSource::LyricsPlus, aligned(&words, true, Timing::Shifted, 300, made(&[(700, "a")])));
        let v = tl.view(TrackId::new());
        assert!(!v.made);
        assert_eq!(v.lines[0].words[0].start, Duration::from_millis(700));
        // Another version's timing: the made words instead.
        tl.set_aligned(LyricsSource::LyricsPlus, aligned(&words, true, Timing::Elsewhere, 0, made(&[(5000, "a")])));
        let v = tl.view(TrackId::new());
        assert!(v.made);
        assert_eq!(v.lines[0].start, Duration::from_millis(5000));
    }

    #[test]
    fn genius_is_asked_only_without_timed_text_that_fits() {
        use nikgob_align::fit::Timing;
        let mut tl = TrackLyrics { genius: true, ..Default::default() };
        tl.pending.extend([LyricsSource::Lrclib, LyricsSource::Genius]);
        assert_eq!(tl.next_lookups(), [LyricsSource::Lrclib]);
        let lrc = Text::from_lrc(Some("[00:01.00]a"), None).unwrap();
        tl.insert(LyricsSource::Lrclib, Some(lrc.clone()));
        assert!(tl.next_lookups().is_empty(), "a timed text is there");
        assert!(!tl.pending.contains(&LyricsSource::Genius));
        // The check says it is another song's: Genius is wanted now.
        tl.set_aligned(LyricsSource::Lrclib, aligned(&lrc, false, Timing::Elsewhere, 0, Vec::new()));
        assert!(tl.genius_wanted());
        tl.pending.insert(LyricsSource::Genius);
        assert_eq!(tl.next_lookups(), [LyricsSource::Genius]);
        // Its words fit, its times are another take's (two seconds off):
        // Genius is worth a look too.
        tl.pending.remove(&LyricsSource::Genius);
        tl.set_aligned(LyricsSource::Lrclib, aligned(&lrc, true, Timing::Shifted, -2140, Vec::new()));
        assert!(tl.genius_wanted());
        // The shown text fits only moved by a few tenths: still asked once,
        // so Genius' text is there to pick.
        tl.set_aligned(LyricsSource::Lrclib, aligned(&lrc, true, Timing::Shifted, -300, Vec::new()));
        assert!(tl.genius_wanted());
        tl.pending.insert(LyricsSource::Genius);
        assert_eq!(tl.next_lookups(), [LyricsSource::Genius]);
        // Its own times fit: no.
        tl.pending.remove(&LyricsSource::Genius);
        tl.set_aligned(LyricsSource::Lrclib, aligned(&lrc, true, Timing::Fits, 0, Vec::new()));
        assert!(!tl.genius_wanted());
        // Off in the settings: never.
        let mut off = TrackLyrics::default();
        off.pending.insert(LyricsSource::Genius);
        assert!(off.next_lookups().is_empty());
        // A plain text is cached and read back as plain.
        let g = Text::from_plain("раз\nдва").unwrap();
        let back = stored_lines(&g.body().unwrap()).unwrap();
        assert_eq!(back.plain.as_deref(), Some("раз\nдва"));
        assert!(back.lines.is_empty());
        let (lines, timed) = back.align_lines();
        assert!(!timed);
        assert_eq!(lines.len(), 2);
    }

    /// «космоцветок»: Genius is asked once when the shown text fits only
    /// moved, not when the moved text is not the shown one, and not again
    /// once it answered (a text or a miss).
    #[test]
    fn genius_is_asked_once_when_the_shown_text_was_moved_to_fit() {
        use nikgob_align::fit::Timing;
        let lrc = Text::from_lrc(Some("[00:14.60]Слово за слово"), None).unwrap();
        let mut tl = TrackLyrics { genius: true, ..Default::default() };
        tl.insert(LyricsSource::Lrclib, Some(lrc.clone()));
        tl.set_aligned(LyricsSource::Lrclib, aligned(&lrc, true, Timing::Shifted, -2140, made(&[(16_740, "Слово за слово")])));
        assert_eq!(tl.chosen(), Some(LyricsSource::Lrclib));
        assert!(tl.chosen_moved());
        assert!(tl.genius_wanted());
        // A word-synced text of its own that fits is shown instead: the
        // moved one is only in the menu, Genius is not needed.
        let mut words = tl.clone();
        let w = worded();
        words.insert(LyricsSource::LyricsPlus, Some(w.clone()));
        words.set_aligned(LyricsSource::LyricsPlus, aligned(&w, true, Timing::Fits, 0, Vec::new()));
        assert_eq!(words.chosen(), Some(LyricsSource::LyricsPlus));
        assert!(!words.genius_wanted());
        // Answered: never again, whether it found a text or not.
        let mut found = tl.clone();
        found.insert(LyricsSource::Genius, Text::from_plain("Слово за слово"));
        assert!(!found.genius_wanted());
        let mut miss = tl.clone();
        miss.insert(LyricsSource::Genius, None);
        assert!(!miss.genius_wanted());
        // Off in the settings: never.
        tl.genius = false;
        assert!(!tl.genius_wanted());
    }

    /// Sources sitting out are not asked: they count as failures and the
    /// ones after them come up.
    #[test]
    fn sources_sitting_out_are_skipped_as_failures() {
        use nikgob_metadata::{MetadataConfig, Verdict};
        let meta = Metadata::new(&MetadataConfig::new("tests@example.invalid")).unwrap();
        let now = std::time::Instant::now();
        let mut tl = TrackLyrics::default();
        tl.pending.extend([LyricsSource::LyricsPlus, LyricsSource::LrcRed, LyricsSource::BetterLyrics, LyricsSource::Lrclib]);
        assert_eq!(next_asked(&mut tl.clone(), &meta), [LyricsSource::LyricsPlus, LyricsSource::LrcRed]);
        meta.lyrics_pauses.note(LyricsSource::LyricsPlus.key(), Verdict::NeedsKey, now);
        let mut one = tl.clone();
        assert_eq!(next_asked(&mut one, &meta), [LyricsSource::LrcRed]);
        assert!(one.failed && !one.pending.contains(&LyricsSource::LyricsPlus));
        meta.lyrics_pauses.note(LyricsSource::LrcRed.key(), Verdict::NeedsKey, now);
        let mut both = tl.clone();
        assert_eq!(next_asked(&mut both, &meta), [LyricsSource::Lrclib]);
        assert!(both.failed);
        // A clone of the clients shares the pauses.
        meta.clone().lyrics_pauses.note(LyricsSource::Lrclib.key(), Verdict::NeedsKey, now);
        let mut all = tl.clone();
        assert!(next_asked(&mut all, &meta).is_empty());
        assert!(all.pending.is_empty() && all.failed);
    }

    /// «Summer 15/16»: LRCLIB's lines and lrc.red's text are in the
    /// library, LyricsPlus, Better Lyrics and Genius are not. Once
    /// LyricsPlus answers, nothing is left to ask: Genius must not stay
    /// «ищу…» with no lookup on its way.
    #[test]
    fn nothing_stays_pending_once_nothing_is_left_to_ask() {
        let lib = Library::open_in_memory().unwrap();
        let track = lib
            .upsert_track(&nikgob_core::model::Track::new("Summer 15/16", nikgob_core::model::ArtistCredit::single("LAZZY2WICE", None)))
            .unwrap()
            .id;
        lib.put_lyrics(track, Some("[00:01.00]a\n[00:02.00]b"), Some("a\nb"), false).unwrap();
        let lrcred = Text::from_lrc(Some("[00:01.00]a\n[00:02.00]b"), None).unwrap().body();
        lib.put_lyrics_text(track, LyricsSource::LrcRed.key(), lrcred.as_deref()).unwrap();
        let mut tl = cached_lyrics(&lib, track, None, None, true, None);
        assert!(tl.pending.contains(&LyricsSource::Genius));
        assert_eq!(tl.next_lookups(), [LyricsSource::LyricsPlus]);
        for got in [None, Some(worded())] {
            let mut tl = tl.clone();
            tl.insert(LyricsSource::LyricsPlus, got);
            assert!(tl.next_lookups().is_empty());
            assert!(tl.pending.is_empty(), "{:?}", tl.pending);
            let v = tl.view(track);
            assert!(!v.sources.iter().any(|o| o.state == v::LyricsAvail::Loading), "{:?}", v.sources);
        }
        // LyricsPlus failed (no network): still nothing left pending.
        tl.pending.remove(&LyricsSource::LyricsPlus);
        tl.failed = true;
        assert!(tl.next_lookups().is_empty());
        assert!(tl.pending.is_empty());
        // Without a timed text Genius is what is asked.
        let mut plain = TrackLyrics { genius: true, ..Default::default() };
        plain.insert(LyricsSource::LrcRed, Text::from_lrc(None, Some("a")));
        plain.pending.extend([LyricsSource::BetterLyrics, LyricsSource::Genius]);
        assert_eq!(plain.next_lookups(), [LyricsSource::Genius]);
        assert!(plain.pending.contains(&LyricsSource::Genius));
    }

    #[tokio::test]
    async fn lyricsplus_is_asked_with_the_full_credit_before_the_main_artist() {
        use nikgob_core::model::{ArtistCredit, Track};
        use nikgob_metadata::{LyricsPlus, MetadataConfig};
        use wiremock::matchers::{path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let mut meta = Metadata::new(&MetadataConfig::new("tests@example.invalid")).unwrap();
        meta.lyricsplus = LyricsPlus::with_base_url(nikgob_metadata::Http::new(&MetadataConfig::new("tests@example.invalid")).unwrap(), &server.uri()).unwrap();
        let lib = Library::open_in_memory().unwrap();
        let mut credit = ArtistCredit::single("WENARO", None);
        credit.push(", ", "LXNER", None);
        let t = lib.upsert_track(&Track::new("Лёд", credit)).unwrap();
        let row = lib.get_track_row(t.id).unwrap().unwrap();
        let apple = r#"{"type":"Line","metadata":{"source":"Apple"},"lyrics":[{"time":1000,"duration":2000,"text":"Ты мой лёд"}]}"#;
        let qq = r#"{"type":"Word","metadata":{"source":"QQ Music"},"lyrics":[{"time":0,"duration":500,"text":"Composed by：Еремин Василий Сергеевич"},{"time":1000,"duration":2000,"text":"Ты мой лёд","syllabus":[{"time":1000,"duration":500,"text":"Ты "},{"time":1500,"duration":500,"text":"мой "},{"time":2000,"duration":1000,"text":"лёд"}]}]}"#;
        let found = |body: &'static str| ResponseTemplate::new(200).set_body_raw(body, "application/json");

        // The full credit is found: one request, Apple's lines.
        Mock::given(path("/v2/lyrics/get")).and(query_param("artist", "WENARO, LXNER")).respond_with(found(apple)).mount(&server).await;
        Mock::given(path("/v2/lyrics/get")).and(query_param("artist", "WENARO")).respond_with(found(qq)).mount(&server).await;
        let text = fetch_online(meta.clone(), lib.clone(), row.clone(), LyricsSource::LyricsPlus).await.unwrap().unwrap();
        assert!(!text.weak);
        assert_eq!(text.lines[0].text, "Ты мой лёд");
        let asked: Vec<String> = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter_map(|r| r.url.query_pairs().find(|(k, _)| k == "artist").map(|(_, v)| v.into_owned()))
            .collect();
        assert_eq!(asked, ["WENARO, LXNER"]);

        // Not found under the full credit: the main artist next, whose QQ
        // text is a weak line sync without credits.
        server.reset().await;
        Mock::given(path("/v2/lyrics/get")).and(query_param("artist", "WENARO, LXNER")).respond_with(ResponseTemplate::new(404)).mount(&server).await;
        Mock::given(path("/v2/lyrics/get")).and(query_param("artist", "WENARO")).respond_with(found(qq)).mount(&server).await;
        let text = fetch_online(meta.clone(), lib.clone(), row.clone(), LyricsSource::LyricsPlus).await.unwrap().unwrap();
        assert!(text.weak);
        assert_eq!(text.sync(), LyricsSync::Line);
        assert_eq!(text.lines.len(), 1, "{:?}", text.lines);
        let asked: Vec<String> = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter_map(|r| r.url.query_pairs().find(|(k, _)| k == "artist").map(|(_, v)| v.into_owned()))
            .collect();
        assert_eq!(asked, ["WENARO, LXNER", "WENARO"]);
        // Cached weak.
        assert!(cached_lyrics(&lib, t.id, None, None, false, None).texts[&LyricsSource::LyricsPlus].weak);
    }

    #[tokio::test]
    async fn online_cache_keeps_misses_but_never_records_server_errors() {
        use nikgob_core::model::{ArtistCredit, Track};
        use nikgob_metadata::{LyricsPlus, MetadataConfig};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        use wiremock::matchers::path;
        let server = MockServer::start().await;
        let mut meta = Metadata::new(&MetadataConfig::new("tests@example.invalid")).unwrap();
        meta.lyricsplus = LyricsPlus::with_base_url(nikgob_metadata::Http::new(&MetadataConfig::new("tests@example.invalid")).unwrap(), &server.uri()).unwrap();
        let lib = Library::open_in_memory().unwrap();
        let t = lib.upsert_track(&Track::new("Song", ArtistCredit::single("Artist", None))).unwrap();
        let row = lib.get_track_row(t.id).unwrap().unwrap();
        Mock::given(path("/v2/lyrics/get")).respond_with(ResponseTemplate::new(404)).mount(&server).await;
        assert!(fetch_online(meta.clone(), lib.clone(), row.clone(), LyricsSource::LyricsPlus).await.unwrap().is_none());
        let miss = lib.lyrics_text(t.id, "lyricsplus").unwrap().unwrap();
        assert!(miss.0.is_none());
        assert!(cached_lyrics(&lib, t.id, None, None, false, None).missing.contains(&LyricsSource::LyricsPlus));
        server.reset().await;
        Mock::given(path("/v2/lyrics/get")).respond_with(ResponseTemplate::new(502)).mount(&server).await;
        assert!(fetch_online(meta.clone(), lib.clone(), row.clone(), LyricsSource::LyricsPlus).await.is_err());
        assert_eq!(lib.lyrics_text(t.id, "lyricsplus").unwrap().unwrap(), miss);
        // A text stored earlier stays when the source has nothing now.
        let body = worded().body();
        lib.put_lyrics_text(t.id, "lyricsplus", body.as_deref()).unwrap();
        server.reset().await;
        Mock::given(path("/v2/lyrics/get")).respond_with(ResponseTemplate::new(404)).mount(&server).await;
        assert!(fetch_online(meta, lib.clone(), row, LyricsSource::LyricsPlus).await.unwrap().is_none());
        assert_eq!(lib.lyrics_text(t.id, "lyricsplus").unwrap().unwrap().0, body);
    }

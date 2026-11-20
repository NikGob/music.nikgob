    use super::*;
    use crate::playback::{Follow, SLEW_MAX, follow, playback_error_toast, teaches_live_recs};
    use nikgob_core::model::PlaylistId;
    use nikgob_view::LIKED_PLAYLIST;

    /// An assistant's or a guest's queue teaches the live recommendations
    /// nothing; the owner's and an agent's do.
    #[test]
    fn only_the_owners_queues_teach_live_recs() {
        use nikgob_core::settings::ApiActor;
        assert!(teaches_live_recs(None));
        assert!(teaches_live_recs(Some(ApiActor::Owner)));
        assert!(teaches_live_recs(Some(ApiActor::Agent)));
        assert!(!teaches_live_recs(Some(ApiActor::Assistant)));
        assert!(!teaches_live_recs(Some(ApiActor::Guest)));
    }

    fn ms(v: u64) -> Duration {
        Duration::from_millis(v)
    }

    #[test]
    fn a_queue_started_from_an_album_a_playlist_or_an_artist_has_its_page() {
        use nikgob_view::Route;
        let (r, p, a) = (nikgob_core::model::ReleaseId::new(), PlaylistId::new(), nikgob_core::model::ArtistId::new());
        assert_eq!(context_page(&format!("release:{r}")), Some(Route::Release(r)));
        assert_eq!(context_page(&format!("playlist:{p}")), Some(Route::Playlist(p)));
        assert_eq!(context_page(&format!("playlist:{LIKED_PLAYLIST}")), Some(Route::Playlist(LIKED_PLAYLIST)));
        assert_eq!(context_page(&format!("artist:{a}")), Some(Route::Artist(a)));
        // A radio of an artist, a mix, a broken id: no page.
        assert_eq!(context_page(&format!("radio:artist:{a}")), None);
        assert_eq!(context_page("mix:Спокойное"), None);
        assert_eq!(context_page("release:nope"), None);
        assert_eq!(context_page("release"), None);
    }

    #[test]
    fn follow_slews_small_differences_and_jumps_far_ones() {
        assert_eq!(follow(ms(10_000), ms(10_002), false, 1.0), Follow::Keep);
        assert_eq!(follow(ms(10_000), ms(10_002), false, 1.1), Follow::Rate(1.0));
        // Behind: faster; ahead: slower, never backwards.
        let Follow::Rate(r) = follow(ms(10_000), ms(10_100), false, 1.0) else { panic!() };
        assert!((r - 1.2).abs() < 1e-9);
        let Follow::Rate(r) = follow(ms(10_100), ms(10_000), false, 1.0) else { panic!() };
        assert!((r - 0.8).abs() < 1e-9);
        let Follow::Rate(r) = follow(ms(10_290), ms(10_000), false, 1.0) else { panic!() };
        assert!((r - (1.0 - SLEW_MAX)).abs() < 1e-9);
        // A seek or a loop.
        assert_eq!(follow(ms(10_000), ms(500), false, 1.0), Follow::Snap);
        assert_eq!(follow(ms(10_000), ms(10_400), false, 1.0), Follow::Snap);
        // Stalled behind us: wait for it; stalled ahead: catch up.
        assert_eq!(follow(ms(10_200), ms(10_000), true, 1.0), Follow::Rate(0.0));
        assert!(matches!(follow(ms(10_000), ms(10_100), true, 1.0), Follow::Rate(r) if r > 1.0));
    }

    /// Our clock as the runtime keeps it, driven by simulated reports.
    struct Sim {
        position: f64,
        at: f64,
        rate: f64,
        heard: Option<Duration>,
        jumps: u32,
    }

    impl Sim {
        fn at(&self, t: f64) -> f64 {
            self.position + (t - self.at) * self.rate
        }

        fn report(&mut self, t: f64, heard: f64) {
            let h = Duration::from_secs_f64(heard);
            let stalled = self.heard == Some(h);
            self.heard = Some(h);
            let ours = self.at(t);
            match follow(Duration::from_secs_f64(ours), h, stalled, self.rate) {
                Follow::Keep => {}
                Follow::Snap => {
                    (self.position, self.at, self.rate) = (heard, t, if stalled { 0.0 } else { 1.0 });
                    self.jumps += 1;
                }
                Follow::Rate(r) => (self.position, self.at, self.rate) = (ours, t, r),
            }
        }
    }

    /// Plays `secs` of simulated time: the heard position from `heard(t)`,
    /// reports every 250 ms arriving 0–12 ms late, the view sampled every
    /// frame. Returns the largest |ours − heard| from `settle` on (s) and
    /// the jumps, and checks the view never went backwards without one.
    fn run(start: f64, secs: f64, settle: f64, heard: impl Fn(f64) -> f64) -> (f64, u32) {
        let mut sim = Sim { position: start, at: 0.0, rate: 1.0, heard: None, jumps: 0 };
        let (mut worst, mut last, mut next_report, mut k) = (0.0f64, f64::MIN, 0.25, 0u64);
        let mut jumps = 0;
        let frame = 1.0 / 120.0;
        let mut t = 0.0;
        while t < secs {
            if t >= next_report {
                // Pseudo-random delivery delay, the same every run.
                k = k.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
                let late = (k >> 33) as f64 / (1u64 << 31) as f64 * 0.012;
                sim.report(t, heard(t - late));
                next_report += 0.25;
            }
            let ours = sim.at(t);
            // Only a jump may move it back.
            assert!(ours >= last - 1e-9 || sim.jumps != jumps, "went back at {t}");
            (last, jumps) = (ours, sim.jumps);
            if t >= settle {
                worst = worst.max((ours - heard(t)).abs());
            }
            t += frame;
        }
        (worst, sim.jumps)
    }

    /// The numbers behind the clock: how far the lyrics and the progress
    /// can be from what is heard (`cargo test -p nikgob-runtime clock --
    /// --nocapture` prints them).
    #[test]
    fn the_clock_follows_what_is_heard() {
        // The sound clock runs 0.3 % off the system clock.
        let (drift, jumps) = run(0.0, 120.0, 1.0, |t| t * 1.003);
        assert_eq!(jumps, 0);
        // 200 ms off at the start (latency the old clock did not know).
        let (offset, jumps) = run(0.2, 30.0, 2.0, |t| t);
        assert_eq!(jumps, 0);
        let (offset_first, _) = run(0.2, 30.0, 0.0, |t| t);
        // A stream stalls for 0.8 s at 10 s, then plays on: told by the
        // second report that stands still, by then up to half a second
        // ahead, it waits or jumps back once.
        let stall_at = |t: f64| if t < 10.0 { t } else if t < 10.8 { 10.0 } else { t - 0.8 };
        let (stall, jumps) = run(0.0, 30.0, 1.0, stall_at);
        assert!(jumps <= 1, "{jumps}");
        let (after_stall, _) = run(0.0, 30.0, 12.5, stall_at);
        println!(
            "clock error: drift 0.3 % {:.1} ms; 200 ms off {:.0} ms at first, {:.1} ms after 2 s; \
             0.8 s stall {:.0} ms at most, {:.1} ms 1.7 s after it",
            drift * 1e3,
            offset_first * 1e3,
            offset * 1e3,
            stall * 1e3,
            after_stall * 1e3,
        );
        assert!(drift < 0.02, "{drift}");
        assert!(offset < 0.02, "{offset}");
        assert!(stall < 0.55, "{stall}");
        assert!(after_stall < 0.02, "{after_stall}");
    }

    #[test]
    fn playback_error_toast_localizes_all_branches() {
        use nikgob_core::i18n::{Locale, errors_in};
        use nikgob_view::{Link, SettingLink};

        assert_eq!(playback_error_toast(Locale::En, None, None), (errors_in(Locale::En).playback_failed.to_owned(), None));
        assert_eq!(playback_error_toast(Locale::Ru, None, None), (errors_in(Locale::Ru).playback_failed.to_owned(), None));
        assert_eq!(
            playback_error_toast(Locale::En, Some("Song".into()), None),
            ("Could not play the track".to_owned(), None),
        );
        assert_eq!(
            playback_error_toast(Locale::Ru, Some("Song".into()), None),
            ("Не получилось воспроизвести трек".to_owned(), None),
        );
        assert_eq!(
            playback_error_toast(Locale::En, Some("Song".into()), Some(("could not play".into(), None))),
            ("Song: could not play".to_owned(), None),
        );
        assert_eq!(
            playback_error_toast(
                Locale::En,
                Some("Song".into()),
                Some(("enable Spotify playback in settings".into(), Some(SettingLink::SpotifyPlayback))),
            ),
            (
                "Song: enable Spotify playback in settings".to_owned(),
                Some(("Sign in", Link::Setting(SettingLink::SpotifyPlayback))),
            ),
        );
    }

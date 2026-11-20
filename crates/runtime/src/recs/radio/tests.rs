use super::*;
use super::slots::{into_slot, ordinary_seeds};

fn entry(id: u64) -> Entry {
    Entry { id, track: TrackId::new() }
}

/// A new song taken out for its preview leaves its place to the radio's
/// next pick: the user's song after it stays after it, and so does the
/// radio's tail; the next song taken out stands in for it before the
/// user's song too.
#[test]
fn a_misfit_is_replaced_in_its_place() {
    let mut q = crate::queue::Queue::default();
    let tracks: Vec<TrackId> = (0..4).map(|_| TrackId::new()).collect();
    // Playing, the radio's misfit, the user's song, the radio's tail.
    q.set_path(&tracks, v::QueueOrigin::None, false);
    let up: Vec<Entry> = q.upcoming().to_vec();
    let (misfit, user, tail) = (up[0], up[1], up[2]);
    let slot = misfit_slot(&up, misfit.id).unwrap();
    assert_eq!(slot, (user.id, false));
    assert!(q.remove(misfit.id));
    let repl = q.enqueue(&[TrackId::new()])[0];
    assert!(into_slot(&mut q, repl.id, slot));
    let order: Vec<u64> = q.upcoming().iter().map(|e| e.id).collect();
    assert_eq!(order, vec![repl.id, user.id, tail.id]);
    // In the middle: after the entry before it.
    let up: Vec<Entry> = q.upcoming().to_vec();
    let slot = misfit_slot(&up, user.id).unwrap();
    assert_eq!(slot, (repl.id, true));
    assert!(q.remove(user.id));
    let next = q.enqueue(&[TrackId::new()])[0];
    assert!(into_slot(&mut q, next.id, slot));
    let order: Vec<u64> = q.upcoming().iter().map(|e| e.id).collect();
    assert_eq!(order, vec![repl.id, next.id, tail.id]);
    // Its anchor gone meanwhile: the pick stays where it was put.
    let late = q.enqueue(&[TrackId::new()])[0];
    assert!(!into_slot(&mut q, late.id, (misfit.id, true)));
    assert_eq!(q.upcoming().last().map(|e| e.id), Some(late.id));
    // Alone in the queue: no place to keep.
    assert_eq!(misfit_slot(&[misfit], misfit.id), None);
}

/// Two new songs taken out before the picks come (two previews heard
/// during one extension): each pick takes one place, in the queue's
/// order, around the user's song; two side by side, both before the
/// radio's tail; one pick for two places keeps the second waiting.
#[test]
fn two_misfits_are_replaced_each_in_its_place() {
    let run = |shape: &[&str]| {
        let mut q = crate::queue::Queue::default();
        let tracks: Vec<TrackId> = (0..=shape.len()).map(|_| TrackId::new()).collect();
        q.set_path(&tracks, v::QueueOrigin::None, false);
        let up: Vec<Entry> = q.upcoming().to_vec();
        let mut slots = Vec::new();
        for (k, name) in shape.iter().enumerate() {
            if name.starts_with('m') {
                let now: Vec<Entry> = q.upcoming().to_vec();
                reanchor(&mut slots, up[k].id, misfit_slot(&now, up[k].id));
                assert!(q.remove(up[k].id));
            }
        }
        (q, up, slots)
    };
    let (mut q, up, slots) = run(&["m1", "user", "m2", "tail"]);
    let picks: Vec<u64> = q.enqueue(&[TrackId::new(), TrackId::new()]).iter().map(|e| e.id).collect();
    assert!(into_slots(&mut q, &picks, slots).is_empty());
    let order: Vec<u64> = q.upcoming().iter().map(|e| e.id).collect();
    assert_eq!(order, vec![picks[0], up[1].id, picks[1], up[3].id]);
    let (mut q, up, slots) = run(&["m1", "m2", "tail"]);
    let picks: Vec<u64> = q.enqueue(&[TrackId::new(), TrackId::new()]).iter().map(|e| e.id).collect();
    assert!(into_slots(&mut q, &picks, slots).is_empty());
    let order: Vec<u64> = q.upcoming().iter().map(|e| e.id).collect();
    assert_eq!(order, vec![picks[0], picks[1], up[2].id]);
    let (mut q, up, slots) = run(&["m1", "user", "m2", "tail"]);
    let one: Vec<u64> = q.enqueue(&[TrackId::new()]).iter().map(|e| e.id).collect();
    let left = into_slots(&mut q, &one, slots);
    assert_eq!(left, vec![(up[1].id, true)], "the second place waits");
    let order: Vec<u64> = q.upcoming().iter().map(|e| e.id).collect();
    assert_eq!(order, vec![one[0], up[1].id, up[3].id]);
}

/// An ordinary radio's new finds are of its seed: a track's radio, its
/// track; a list's, its first songs; a mix's, none.
#[test]
fn an_ordinary_radios_finds_are_of_its_seed() {
    let (a, b) = (TrackId::new(), TrackId::new());
    assert_eq!(ordinary_seeds(&RadioSeed::Track(a)), vec![a]);
    assert_eq!(ordinary_seeds(&RadioSeed::Tracks(vec![a, b])), vec![a, b]);
    assert!(ordinary_seeds(&RadioSeed::Discover).is_empty());
}

/// What autoplay and a radio added comes back by its places after a
/// restart (new entry ids); a place that holds another track now does
/// not.
#[test]
fn added_entries_come_back_by_their_places() {
    let before: Vec<Entry> = (1..=5).map(entry).collect();
    let places = places_of(&before, &HashSet::from([2, 4, 5]));
    assert_eq!(places, vec![(1, before[1].track), (3, before[3].track), (4, before[4].track)]);
    // The next run: the same tracks under new ids, the last one moved.
    let mut after: Vec<Entry> = before.iter().map(|e| Entry { id: e.id + 100, track: e.track }).collect();
    after.swap(3, 4);
    assert_eq!(entries_at(&after, &places), HashSet::from([102]));
    // An older session (no places): nothing.
    assert!(entries_at(&after, &[]).is_empty());
}

/// A play dragged to its end (the runtime's «Дальше» at 90 % or later,
/// or the track run out) is judged by what was heard, by the session
/// and by the radio alike; with the bar untouched, or dragged from
/// where nobody knows, it is complete as always.
#[test]
fn a_play_dragged_to_its_end_is_judged_by_what_was_heard() {
    use nikgob_recs::session::{Ended, Scrub, judge};
    let t = TrackId::new();
    for (to, at, verdict) in [(190_000, 192_000, Verdict::Neutral), (195_000, 200_000, Verdict::Neutral), (30_000, 200_000, Verdict::Approved)] {
        let mut s = Session::default();
        s.start(t, 1_000, EventContext::Radio, None);
        s.seek(t, Some(2_000), to);
        s.end_at(t, Ended::Complete, at, Some(200_000), None);
        let p = s.last().unwrap();
        assert_eq!(judge(p.ended, p.position_ms, p.duration_ms, p.dragged()), verdict, "the session, to {to}");
        let o = outcome_of(true, at, Some(200_000), p.dragged());
        assert!(matches!(o, Outcome::Skip { .. }));
        assert_eq!(o.verdict(), verdict, "the radio, to {to}");
    }
    assert_eq!(outcome_of(true, 192_000, Some(200_000), None), Outcome::Complete);
    let blind = Some(Scrub { to_ms: 190_000, heard_ms: 0, blind: true });
    assert_eq!(outcome_of(true, 200_000, Some(200_000), blind), Outcome::Complete);
    assert_eq!(outcome_of(false, 50_000, Some(200_000), None), Outcome::Skip { position_ms: 50_000, duration_ms: Some(200_000), scrub: None });
}

/// A wave's card is saved with it, and a session from before it was
/// saved reads as the plain wave.
#[test]
fn a_wave_keeps_its_card_in_the_session() {
    let save = RadioSave {
        state: Engine::new(RadioSeed::Wave(nikgob_recs::wave::WaveSeed::plain(0.25))),
        title: "Моя волна".into(),
        kind: Some(v::RadioTitle::Wave),
        context: EventContext::HomeSection,
        seed: 1,
        streams: Vec::new(),
        pool: Vec::new(),
        autoplay: false,
        added_at: Vec::new(),
        wave_preset: Some(v::MixKey::Calm),
        mix_naming: None,
    };
    let json = serde_json::to_value(&save).unwrap();
    let back: RadioSave = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(back.wave_preset, Some(v::MixKey::Calm));
    assert_eq!(back.kind, Some(v::RadioTitle::Wave));
    let mut old = json;
    old.as_object_mut().unwrap().remove("wave_preset");
    old.as_object_mut().unwrap().remove("kind");
    let back: RadioSave = serde_json::from_value(old).unwrap();
    assert_eq!(back.wave_preset, None);
    assert_eq!(back.kind, None);
    assert_eq!(v::RadioTitle::from_legacy(&back.title), v::RadioTitle::Wave);
}

fn song(id: &str, artist: &str) -> StreamTrack {
    let mut t = StreamTrack::new(RemoteId::new(Service::Youtube, id), id);
    t.artists = vec![nikgob_core::streaming::ArtistRef::new(None, artist)];
    t
}

fn sound(x: f32) -> nikgob_recs::Vector {
    let mut v = [0.0; nikgob_recs::DIM];
    v[0] = x;
    v
}

/// A service's list the catalog lacks is blended whole: the mean sound
/// of every analysed song of it and every library artist it matches,
/// not its first song alone; a library list (or a service list the
/// catalog has whole) is its catalog tracks.
#[test]
fn a_service_list_radio_blends_the_whole_list() {
    let ids: Vec<TrackId> = (0..3).map(|_| TrackId::new()).collect();
    let streams = vec![song("a", "Кино"), song("b", "Аквариум"), song("c", "Кино")];
    let (kino, aquarium) = (ArtistId::new(), ArtistId::new());
    let artist = |name: &str| match name {
        "Кино" => Some(kino),
        "Аквариум" => Some(aquarium),
        _ => None,
    };
    let vector = |t: TrackId| (t != ids[0]).then(|| sound(if t == ids[1] { 1.0 } else { 3.0 }));
    let seed = list_seed(&ids, &streams, |_| false, vector, artist);
    let Some(RadioSeed::Stream { id, vector, artists }) = seed else { panic!("{seed:?}") };
    assert_eq!(id, streams[0].id);
    assert_eq!(vector.map(|v| v[0]), Some(2.0));
    assert_eq!(artists, vec![kino, aquarium]);
    // The catalog has some of it: still the whole list.
    let some = list_seed(&ids, &streams, |t| t == ids[1], |_| None, |_| None);
    assert!(matches!(some, Some(RadioSeed::Stream { .. })), "{some:?}");
    // The catalog has all of it, or it is the library's own.
    assert_eq!(list_seed(&ids, &streams, |_| true, |_| None, |_| None), Some(RadioSeed::Tracks(ids.clone())));
    assert_eq!(list_seed(&ids, &[], |t| t != ids[0], |_| None, |_| None), Some(RadioSeed::Tracks(ids[1..].to_vec())));
    assert_eq!(list_seed(&ids, &[], |_| false, |_| None, |_| None), None);
    assert_eq!(list_seed(&[], &[], |_| true, |_| None, |_| None), None);
}

/// A list's radio plays none of the list's own songs unasked: its
/// seed holds no track to play first, and a service list's first song
/// counts as served, so what plays is the radio's picks, which keep
/// out the disliked, the banned and (under «Миксы только из
/// скачанного») the not downloaded.
#[test]
fn a_list_radio_plays_the_picks_not_the_list() {
    let ids: Vec<TrackId> = (0..2).map(|_| TrackId::new()).collect();
    let streams = vec![song("a", "x"), song("b", "y")];
    let seed = list_seed(&ids, &streams, |_| false, |_| None, |_| None).expect("a seed");
    assert!(!matches!(seed, RadioSeed::Track(_)));
    let engine = Engine::new(seed);
    assert!(engine.served().any(|i| *i == Item::Stream(streams[0].id.clone())));
    assert!(!engine.served().any(|i| matches!(i, Item::Track(_))));
    let library = Engine::new(list_seed(&ids, &[], |_| true, |_| None, |_| None).expect("a seed"));
    assert_eq!(library.served().count(), 0);
}

#[test]
fn contexts_read_as_phrases() {
    assert_eq!(v::RadioTitle::Seed("Кино".into()).to_queue_origin().header_in(v::Locale::Ru, false).0, "Играет из радио «Кино»");
    assert_eq!(v::RadioTitle::Mix(v::MixTitle::Energy { calm: true }).to_queue_origin().header_in(v::Locale::Ru, false).0, "Играет из микса «Спокойный микс»");
    assert_eq!(v::RadioTitle::Album("nyan.pop".into()).to_queue_origin().header_in(v::Locale::Ru, false).0, "Играет из радио по альбому «nyan.pop»");
    assert_eq!(
        v::RadioTitle::Playlist { name: "P".into(), liked: false }.title_in(v::Locale::Ru),
        "Радио по плейлисту «P»"
    );
    assert_eq!(
        v::RadioTitle::Playlist { name: "Любимые".into(), liked: true }.to_queue_origin(),
        v::QueueOrigin::Radio(v::RadioOrigin::Liked)
    );
}

#[test]
fn active_mix_naming_side_relocalizes_and_round_trips() {
    let side = v::MixTitle::Side {
        calm: true,
        flavor: v::SideMixFlavor::Russian,
    };
    assert_eq!(side.title_in(v::Locale::Ru), "Русские песни потише");
    assert_eq!(side.title_in(v::Locale::En), "Calm Russian Songs");
    let rt = v::RadioTitle::Mix(side);
    let json = serde_json::to_string(&rt).unwrap();
    let back: v::RadioTitle = serde_json::from_str(&json).unwrap();
    assert_eq!(back, rt);

    // Legacy RadioSave JSON without `kind` or `mix_naming` migrates side mixes cleanly.
    let mut old = serde_json::to_value(RadioSave {
        state: Engine::new(RadioSeed::Wave(nikgob_recs::wave::WaveSeed::plain(0.25))),
        title: "Спокойное без слов".into(),
        kind: None,
        context: EventContext::HomeSection,
        seed: 1,
        streams: Vec::new(),
        pool: Vec::new(),
        autoplay: false,
        added_at: Vec::new(),
        wave_preset: Some(v::MixKey::Calm),
        mix_naming: None,
    })
    .unwrap();
    old.as_object_mut().unwrap().remove("kind");
    old.as_object_mut().unwrap().remove("mix_naming");
    let back: RadioSave = serde_json::from_value(old).unwrap();
    assert_eq!(
        v::RadioTitle::from_legacy(&back.title),
        v::RadioTitle::Mix(v::MixTitle::Side {
            calm: true,
            flavor: v::SideMixFlavor::Instrumental,
        })
    );
    assert_eq!(v::RadioTitle::from_legacy(&back.title).title_in(v::Locale::En), "Calm Instrumental");
}

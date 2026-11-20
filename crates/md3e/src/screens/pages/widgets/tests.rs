#[cfg(test)]
mod tests {
    use super::initials;

    /// The grid takes the same room whether its rows are laid out or not.
    #[test]
    fn grid_rows_out_of_sight_keep_their_room() {
        let height_with = |screen_h: f32| {
            let ctx = egui::Context::default();
            let mut h = 0.0;
            for i in 0..3 {
                let input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, screen_h))),
                    time: Some(f64::from(i) * 0.05),
                    ..Default::default()
                };
                let mut out = ctx.run_ui(input, |ui| {
                    let items: Vec<u32> = (0..203).collect();
                    let top = ui.cursor().top();
                    let mut drawn = 0;
                    super::grid(ui, &items, 160.0, |_, w| w + 56.0, |ui, _, w| {
                        drawn += 1;
                        ui.allocate_exact_size(egui::vec2(w, w + 56.0), egui::Sense::hover());
                        None
                    });
                    h = ui.cursor().top() - top;
                    let _ = drawn;
                });
                out.textures_delta.clear();
            }
            h
        };
        assert_eq!(height_with(400.0), height_with(100_000.0));
    }

    #[test]
    fn initials_skip_particles() {
        assert_eq!(initials("Boards of Canada"), "BC");
        assert_eq!(initials("Кино"), "К");
        assert_eq!(initials("Molchat Doma"), "MD");
        assert_eq!(initials("the xx"), "TX");
        assert_eq!(initials("Nick Cave & the Bad Seeds"), "NS");
        assert_eq!(initials(""), "");
    }
}

#[cfg(test)]
mod menu_tests {
    use super::{
        ArtistAct, PlaylistAct, PlaylistMenuOf, ReleaseAct, Row, artist_rows, card_badge, playlist_command, playlist_rows,
        progress_badge, release_apply, release_rows, row_acts,
    };
    use nikgob_view::{ArtistLinks, Command, PlaylistId, PlaylistSyncMode, ReleaseCard, ReleaseId, ReleaseType};

    fn labels<A>(rows: &[Row<A>]) -> Vec<String> {
        rows.iter()
            .filter_map(|r| match r {
                Row::Item(_, i) => Some(i.label.clone()),
                Row::Divider | Row::Label(_) | Row::Sub(..) => None,
            })
            .collect()
    }

    /// The playlist's right-click menu and its page's ⋮ are one list: the
    /// page leaves out only «Слушать» and «Перемешать» (its buttons), and
    /// both can play the playlist next and queue it.
    #[test]
    fn a_playlist_offers_the_same_actions_by_right_click_and_by_more() {
        let card = row_acts(&playlist_rows(PlaylistMenuOf { editable: true, ..Default::default() }));
        let page = row_acts(&playlist_rows(PlaylistMenuOf { on_page: true, editable: true, ..Default::default() }));
        let card_less_buttons: Vec<PlaylistAct> =
            card.iter().copied().filter(|a| !matches!(a, PlaylistAct::Play | PlaylistAct::Shuffle)).collect();
        assert_eq!(card_less_buttons, page);
        for acts in [&card, &page] {
            assert!(acts.contains(&PlaylistAct::Next) && acts.contains(&PlaylistAct::Queue));
            assert_eq!(acts.last(), Some(&PlaylistAct::Delete));
        }
        // The liked playlist: no edit, no delete, still next and queue.
        let liked = row_acts(&playlist_rows(PlaylistMenuOf::default()));
        assert_eq!(liked, [PlaylistAct::Play, PlaylistAct::Shuffle, PlaylistAct::Radio, PlaylistAct::Next, PlaylistAct::Queue]);
        assert_eq!(&labels(&playlist_rows(PlaylistMenuOf::default()))[2..], ["Радио по плейлисту", "Сыграть следующим", "В очередь"]);
        let id = PlaylistId::new();
        assert_eq!(playlist_command(PlaylistAct::Next, id), Some(Command::PlayPlaylistNext(id)));
        assert_eq!(playlist_command(PlaylistAct::Queue, id), Some(Command::EnqueuePlaylist(id)));
        assert_eq!(playlist_command(PlaylistAct::Radio, id), Some(Command::StartRadio { seed: nikgob_view::RadioSeed::Playlist(id) }));
    }

    /// A linked playlist's modes say what they do, and one stops the sync.
    #[test]
    fn a_linked_playlist_can_stop_syncing() {
        let rows = playlist_rows(PlaylistMenuOf {
            on_page: true,
            editable: true,
            sync: Some(("YouTube Music", PlaylistSyncMode::TwoWay)),
            can_link: false,
        });
        let l = labels(&rows);
        for want in ["Изменения в обе стороны", "Только получать изменения", "Выключить синхронизацию"] {
            assert!(l.iter().any(|x| x == want), "{want} missing from {l:?}");
        }
        assert!(rows.iter().any(|r| matches!(r, Row::Item(PlaylistAct::Sync(PlaylistSyncMode::TwoWay), i) if i.selected)));
        // Off: the two ways to turn it on, neither marked, and no way to
        // turn it off again.
        let off = playlist_rows(PlaylistMenuOf { sync: Some(("YouTube Music", PlaylistSyncMode::Once)), ..Default::default() });
        assert!(!labels(&off).iter().any(|x| x == "Выключить синхронизацию"));
        assert!(!off.iter().any(|r| matches!(r, Row::Item(PlaylistAct::Sync(_), i) if i.selected)));
        assert_eq!(off.iter().filter(|r| matches!(r, Row::Item(PlaylistAct::Sync(_), _))).count(), 2);
        assert!(rows.iter().any(|r| matches!(r, Row::Label(t) if t == "Синхронизация с YouTube Music")));
        let acts = row_acts(&rows);
        assert!(!acts.contains(&PlaylistAct::Link));
        let id = PlaylistId::new();
        assert_eq!(
            playlist_command(PlaylistAct::Sync(PlaylistSyncMode::Once), id),
            Some(Command::SetPlaylistSync { playlist: id, mode: PlaylistSyncMode::Once })
        );
        // Not linked: the way to link it instead of the modes, last with
        // the delete after their divider; asked first, so no command yet.
        let rows = playlist_rows(PlaylistMenuOf { on_page: true, editable: true, can_link: true, ..Default::default() });
        let unlinked = row_acts(&rows);
        assert_eq!(&unlinked[unlinked.len() - 2..], [PlaylistAct::Link, PlaylistAct::Delete]);
        assert!(matches!(rows[rows.len() - 3], Row::Divider));
        assert!(!unlinked.iter().any(|a| matches!(a, PlaylistAct::Sync(_))));
        assert_eq!(playlist_command(PlaylistAct::Link, id), None);
        assert_eq!(super::playlist_link_command(id), Command::SetPlaylistSync { playlist: id, mode: PlaylistSyncMode::TwoWay });
    }

    /// The album's right-click menu and its page's ⋮ are one list in one
    /// order: the page leaves out its buttons (play, shuffle, the like).
    #[test]
    fn an_album_offers_the_same_actions_by_right_click_and_by_more() {
        let r = ReleaseCard {
            id: ReleaseId::new(),
            title: "Album".into(),
            artist: "Artist".into(),
            artists: ArtistLinks::default(),
            year: None,
            release_type: ReleaseType::Album,
            cover: None,
            local_tracks: 1,
            total_tracks: Some(1),
            liked: false,
            added_at: 0,
            better: false,
            mine: None,
            mine_local: 1,
            whole: false,
        };
        let card = row_acts(&release_rows(&r, false));
        let page = row_acts(&release_rows(&r, true));
        let shared = |acts: &[ReleaseAct]| -> Vec<ReleaseAct> {
            acts.iter()
                .copied()
                .filter(|a| !matches!(a, ReleaseAct::Play | ReleaseAct::Shuffle | ReleaseAct::Like))
                .collect()
        };
        assert_eq!(shared(&card), shared(&page));
        assert_eq!(&card[..7], [ReleaseAct::Play, ReleaseAct::Shuffle, ReleaseAct::Radio, ReleaseAct::Next, ReleaseAct::Queue, ReleaseAct::Like, ReleaseAct::Playlist]);
        assert_eq!(&page[..4], [ReleaseAct::Radio, ReleaseAct::Next, ReleaseAct::Queue, ReleaseAct::Playlist]);
        assert_eq!(&labels(&release_rows(&r, false))[2..7], ["Радио по альбому", "Сыграть следующим", "В очередь", "Нравится", "В плейлист…"]);
        for acts in [&card, &page] {
            assert_eq!(&acts[acts.len() - 2..], [ReleaseAct::Trash, ReleaseAct::Purge]);
        }
        // Not all of it here: what it lacks can be downloaded from the card.
        let part = ReleaseCard { local_tracks: 3, total_tracks: Some(12), ..r.clone() };
        let acts = row_acts(&release_rows(&part, false));
        assert_eq!(&acts[acts.len() - 3..], [ReleaseAct::Download, ReleaseAct::Trash, ReleaseAct::Purge]);
        assert!(!row_acts(&release_rows(&part, true)).contains(&ReleaseAct::Download), "a button on its page");
        // A stream-only album has no files to delete for good, and an
        // album with no known artist no way to one.
        let stream = ReleaseCard { local_tracks: 0, ..part };
        let acts = row_acts(&release_rows(&stream, false));
        assert!(acts.contains(&ReleaseAct::Trash) && !acts.contains(&ReleaseAct::Purge) && !acts.contains(&ReleaseAct::Artist));
        // An album with a file that can get better: on its card and its page.
        let mp3 = ReleaseCard { better: true, ..r.clone() };
        for on_page in [false, true] {
            let acts = row_acts(&release_rows(&mp3, on_page));
            assert_eq!(&acts[acts.len() - 3..], [ReleaseAct::Better, ReleaseAct::Trash, ReleaseAct::Purge], "{on_page}");
        }
        assert!(!row_acts(&release_rows(&r, false)).contains(&ReleaseAct::Better));
        let gone = ReleaseCard { local_tracks: 0, ..mp3 };
        assert!(!row_acts(&release_rows(&gone, false)).contains(&ReleaseAct::Better));
        // Its cover is refreshed from its card and its page, a stream-only
        // album's too.
        for (card, on_page) in [(&r, false), (&r, true), (&gone, false)] {
            let acts = row_acts(&release_rows(card, on_page));
            let share = acts.iter().position(|a| *a == ReleaseAct::Share).unwrap();
            assert_eq!(acts[share + 1], ReleaseAct::RefreshCover);
        }
        let mut out = Vec::new();
        release_apply(&egui::Context::default(), ReleaseAct::RefreshCover, &r, &mut out);
        assert_eq!(out, [Command::RefreshReleaseCover(r.id)]);
    }

    /// An album only some songs of which are the user's: its card counts
    /// those songs (no «1 из 13»), its menus play it whole once, add it
    /// whole, download the user's songs it lacks or the whole of it.
    #[test]
    fn an_album_of_a_few_songs_of_the_users_owes_nothing_else() {
        let r = ReleaseCard {
            id: ReleaseId::new(),
            title: "OST".into(),
            artist: "Artist".into(),
            artists: ArtistLinks::default(),
            year: None,
            release_type: ReleaseType::Album,
            cover: None,
            local_tracks: 1,
            total_tracks: Some(13),
            liked: false,
            added_at: 0,
            better: false,
            mine: Some(1),
            mine_local: 1,
            whole: false,
        };
        assert_eq!(progress_badge(&r), None);
        assert_eq!(card_badge(&r), None);
        let streams = ReleaseCard { mine_local: 0, ..r.clone() };
        assert_eq!(card_badge(&streams).as_deref(), Some("стрим"));
        let acts = row_acts(&release_rows(&r, false));
        for a in [ReleaseAct::PlayWhole, ReleaseAct::AddWhole, ReleaseAct::DownloadWhole] {
            assert!(acts.contains(&a), "{a:?}");
        }
        assert!(!acts.contains(&ReleaseAct::Download), "nothing of the user's lacks a file");
        let lacking = ReleaseCard { mine: Some(2), ..r.clone() };
        assert!(row_acts(&release_rows(&lacking, false)).contains(&ReleaseAct::Download));
        let mut out = Vec::new();
        let ctx = egui::Context::default();
        release_apply(&ctx, ReleaseAct::PlayWhole, &r, &mut out);
        release_apply(&ctx, ReleaseAct::AddWhole, &r, &mut out);
        assert_eq!(
            out,
            [Command::PlayReleaseWhole { id: r.id, start: None, shuffle: false }, Command::AddWholeAlbum { id: r.id, on: true }]
        );
    }

    /// A switch on an album's page holds while the page stays: back on the
    /// page later it opens as the album does.
    #[test]
    fn an_album_page_view_lasts_one_visit() {
        use crate::screens::pages::widgets::{show_whole, visit_whole};
        let ctx = egui::Context::default();
        let id = ReleaseId::new();
        let page = |ctx: &egui::Context| {
            let mut seen = None;
            ctx.run_ui(Default::default(), |ui| seen = visit_whole(ui.ctx(), id)).textures_delta.clear();
            seen
        };
        let away = |ctx: &egui::Context| {
            for _ in 0..4 {
                ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
            }
        };
        assert_eq!(page(&ctx), None);
        show_whole(&ctx, id, true);
        assert_eq!(page(&ctx), Some(true));
        assert_eq!(page(&ctx), Some(true));
        away(&ctx);
        assert_eq!(page(&ctx), None, "a later visit opens as the album does");
    }

    /// The artist's right-click menu and its page's ⋮: one list, the page
    /// without its buttons (mix, shuffle, radio, «Подписаться»).
    #[test]
    fn an_artist_offers_the_same_actions_by_right_click_and_by_more() {
        let a = nikgob_view::ArtistCard {
            id: nikgob_view::ArtistId::new(),
            name: "Artist".into(),
            image: None,
            liked: false,
            track_count: 0,
            latin: None,
            character: false,
        };
        let marks = nikgob_view::DislikesView::default();
        let card = row_acts(&artist_rows(&a, false, &marks));
        let page = row_acts(&artist_rows(&a, true, &marks));
        let shared: Vec<ArtistAct> = card
            .iter()
            .copied()
            .filter(|x| !matches!(x, ArtistAct::Play | ArtistAct::Shuffle | ArtistAct::Radio | ArtistAct::Follow))
            .collect();
        assert_eq!(shared, page);
        assert_eq!(page, [ArtistAct::Next, ArtistAct::Queue, ArtistAct::Ban]);
        assert_eq!(&labels(&artist_rows(&a, true, &marks))[..2], ["Сыграть следующим", "В очередь"]);
    }

    fn track(has_local: bool) -> nikgob_view::TrackRow {
        nikgob_view::TrackRow {
            id: nikgob_view::TrackId::new(),
            title: "Song".into(),
            artist: "Artist".into(),
            album: Some("Album".into()),
            release_id: Some(ReleaseId::new()),
            artists: nikgob_view::Credit::single("Artist", Some(nikgob_view::ArtistId::new())),
            disc_no: 1,
            track_no: 1,
            duration: None,
            quality: None,
            source: None,
            has_local,
            liked: false,
            cover: None,
            clip: false,
            in_library: true,
        }
    }

    fn of(marks: &nikgob_view::DislikesView) -> crate::screens::pages::LibraryRowsOf<'_> {
        crate::screens::pages::LibraryRowsOf { marks, info: false, also: None, torrents: false, wished: None }
    }

    /// The track menus (a library row, a playlist row, a service's song)
    /// share one list: the same common actions in one order, then what
    /// only the place adds («Убрать из плейлиста», last).
    #[test]
    fn every_track_menu_has_the_common_actions_in_one_order() {
        use crate::screens::pages::{LibraryRowsOf, TrackAct as A, library_track_rows};
        let marks = nikgob_view::DislikesView::default();
        let t = track(true);
        let library = row_acts(&library_track_rows(&t, of(&marks)));
        let playlist = row_acts(&super::playlist_track_rows(&t, PlaylistId::new(), of(&marks)));
        assert_eq!(&playlist[..library.len()], &library[..]);
        assert_eq!(playlist.last(), Some(&A::Remove));
        let head = [A::Next, A::Queue, A::Radio, A::Playlist, A::Album, A::Artist, A::Share];
        assert_eq!(&library[..head.len()], head);
        assert_eq!(&library[library.len() - 2..], [A::Dislike, A::Ban]);
        // The service's song: the same head and tail around its downloads.
        let stream = row_acts(&crate::screens::pages::track_rows(crate::screens::pages::TrackMenuOf {
            playable: true,
            has_album: true,
            has_artists: true,
            has_file: false,
            also_on: None,
            info: false,
            fetch: Some(crate::components::MenuItem::new("Скачать")),
            better: false,
            pick_release: false,
            find_song: None,
            wrong_file: false,
            dislike: crate::components::MenuItem::new("Не нравится"),
            ban: Some(crate::components::MenuItem::new("Не рекомендовать исполнителя")),
            library: None,
        }));
        assert_eq!(&stream[..head.len()], head);
        assert_eq!(&stream[head.len()..], [A::Fetch, A::AlbumDownload, A::Dislike, A::Ban]);
        // The other album of the song sits right after «К альбому»; «Разные
        // песни» and «Убрать из плейлиста» close the menu in one group.
        let also = (ReleaseId::new(), "Other".to_owned());
        let with_also = LibraryRowsOf { also: Some(&also), ..of(&marks) };
        let rows = library_track_rows(&t, with_also);
        let acts = row_acts(&rows);
        assert_eq!(&acts[4..7], [A::Album, A::AlsoOn, A::Artist]);
        assert!(labels(&rows).iter().any(|l| l == "Есть и на альбоме «Other»"));
        let rows = super::playlist_track_rows(&t, PlaylistId::new(), with_also);
        assert!(matches!(&rows[rows.len() - 3..], [Row::Divider, Row::Item(A::Apart, _), Row::Item(A::Remove, _)]));
    }
}

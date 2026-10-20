//! The recommendations part of the view model (stage 4, docs/stage4.md §6):
//! the mix cards of the home page, what plays a mix or a radio, the
//! «Не рекомендовать» feedback and the radio line of the queue.
//!
//! The engine itself (`nikgob-recs`) stays behind the runtime: the view only
//! names things ([`Mood`], [`MixId`]) and shows what the runtime filled in.

use crate::{ArtistId, Cover, RemoteId, TrackId};

/// The six moods of «Под настроение» (the engine's own enum stays in the
/// runtime; this is its name in the view).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Mood {
    /// «Спокойное».
    Calm,
    /// «Энергичное».
    Energetic,
    /// «Мрачное».
    Dark,
    /// «Светлое».
    Bright,
    /// «Для фокуса».
    Focus,
    /// «Драйв».
    Drive,
}

impl Mood {
    pub const ALL: [Self; 6] = [Self::Calm, Self::Energetic, Self::Dark, Self::Bright, Self::Focus, Self::Drive];

    /// The card's title.
    pub const fn title(self) -> &'static str {
        match self {
            Self::Calm => "Спокойное",
            Self::Energetic => "Энергичное",
            Self::Dark => "Мрачное",
            Self::Bright => "Светлое",
            Self::Focus => "Для фокуса",
            Self::Drive => "Драйв",
        }
    }
}

/// Which mix a card plays ([`crate::Command::PlayMix`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MixId {
    /// «Твой микс»: the hero, the same all day (until «Обновить»).
    Daily,
    /// A mood card: endless (it plays as a radio).
    Mood(Mood),
    /// «Твои миксы»: a style mix by its place in [`crate::HomeView::style_mixes`]
    /// (endless).
    Style(u8),
    /// «Миксы исполнителей»: the artist's radio.
    Artist(ArtistId),
    /// «Новое для тебя» → «Слушать всё»: the discoveries, then a radio of
    /// new music.
    Discover,
}

/// One mix card of the home page.
#[derive(Clone, Debug, PartialEq)]
pub struct MixCard {
    pub id: MixId,
    /// «Твой микс», «Спокойное», «mzlff, MORGENSHTERN и другие».
    pub title: String,
    /// A quiet second line: «30 треков · обновлён сегодня»; empty when the
    /// card has nothing to add.
    pub subtitle: String,
    /// Up to four covers for a collage (the first alone for a single
    /// cover); files of the library or a service's `https://` urls.
    pub covers: Vec<Cover>,
}

/// What a radio grows from ([`crate::Command::StartRadio`]): «Радио по
/// треку» from a library track or a service's track, «Радио» on an artist
/// page.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RadioSeed {
    Track(TrackId),
    Stream(RemoteId),
    Artist(ArtistId),
}

/// What «Не рекомендовать» is about ([`crate::Command::RecFeedback`]).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RecTarget {
    /// A library track (a liked stream track too).
    Track(TrackId),
    /// A library artist: none of their tracks is picked.
    Artist(ArtistId),
    /// A track of a service (a discovery): that song, on every service.
    StreamTrack(RemoteId),
    /// An artist of a service, by the name shown: none of their songs is
    /// discovered.
    StreamArtist(String),
}

/// The radio the queue plays ([`crate::QueueView::radio`]): it extends
/// itself while it plays.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RadioView {
    /// «Радио · Группа крови», «Радио · Кино», «Спокойное», «Твой микс».
    pub title: String,
    /// Looking for the next tracks right now.
    pub extending: bool,
}

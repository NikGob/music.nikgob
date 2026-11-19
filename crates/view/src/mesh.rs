//! «Межсеть → Устройства»: this instance, the owner's other devices (paired
//! once, trusted by the ring's certificates), the unpaired ones seen nearby,
//! and the pairing flow. The runtime keeps what is saved (names given here,
//! the storage profile, a device being forgotten); the mesh service
//! reports what is live ([`MeshSnapshot`]: online, nearby, presence, the
//! pairing's state).

use serde::{Deserialize, Serialize};

pub use nikgob_core::settings::KeepMasters;

use nikgob_core::i18n::Locale;

use crate::PlaylistId;

/// What a device is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MeshKind {
    #[default]
    Pc,
    Pi,
    Phone,
    Tv,
    Server,
}

impl MeshKind {
    pub const ALL: [Self; 5] = [Self::Pc, Self::Pi, Self::Phone, Self::Tv, Self::Server];

    /// Its word in mDNS, certificates and configs (`pc`, `pi`…).
    pub const fn key(self) -> &'static str {
        match self {
            Self::Pc => "pc",
            Self::Pi => "pi",
            Self::Phone => "phone",
            Self::Tv => "tv",
            Self::Server => "server",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.key().eq_ignore_ascii_case(s.trim()))
    }
}

/// How far a device is trusted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MeshScope {
    /// «Свой»: everything.
    #[default]
    Owner,
    /// «Гость»: plays and reads, changes nothing of the owner's.
    Guest,
}

impl MeshScope {
    pub const fn key(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Guest => "guest",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "owner" => Some(Self::Owner),
            "guest" => Some(Self::Guest),
            _ => None,
        }
    }
}

/// What a device stores (the replica wave fills it in).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeshProfile {
    /// The library, the masters and the database.
    #[default]
    Full,
    /// The library without the masters.
    Library,
    /// What is pinned plus a cache up to `cap_bytes`.
    Favorites { cap_bytes: u64 },
    /// Nothing but a buffer.
    SoundOnly,
}

impl MeshProfile {
    /// Its word in the node table (`full`, `library`, `favorites:<bytes>`,
    /// `sound`).
    pub fn key(self) -> String {
        match self {
            Self::Full => "full".to_owned(),
            Self::Library => "library".to_owned(),
            Self::Favorites { cap_bytes } => format!("favorites:{cap_bytes}"),
            Self::SoundOnly => "sound".to_owned(),
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        match s {
            "full" => Some(Self::Full),
            "library" => Some(Self::Library),
            "sound" => Some(Self::SoundOnly),
            _ => s.strip_prefix("favorites:").and_then(|n| n.parse().ok()).map(|cap_bytes| Self::Favorites { cap_bytes }),
        }
    }

    /// The profile chip's word, in the active locale: «Полная копия»,
    /// «Библиотека», «Избранное», «Только звук».
    pub fn label(self) -> &'static str {
        let s = nikgob_core::i18n::mesh();
        match self {
            Self::Full => s.profile_full,
            Self::Library => s.profile_library,
            Self::Favorites { .. } => s.favorites,
            Self::SoundOnly => s.profile_sound_only,
        }
    }

    /// The device keeps files of the library (not «Только звук»).
    pub const fn stores(self) -> bool {
        !matches!(self, Self::SoundOnly)
    }

    /// The form its files take: the originals as they are for every
    /// profile now (the phone's Opus or MP3 come with the phone).
    pub const fn format(self) -> MeshFormat {
        MeshFormat::Orig
    }
}

/// The form a device keeps the library's files in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MeshFormat {
    /// The file as it is (lossy files and masters alike).
    #[default]
    Orig,
    /// Opus made by the device that sends it (not yet).
    Opus,
    /// MP3 made from the lossless master; lossy originals go as they are
    /// (not yet).
    Mp3,
}

impl MeshFormat {
    /// Its word in `?profile=` of the media route.
    pub const fn key(self) -> &'static str {
        match self {
            Self::Orig => "orig",
            Self::Opus => "opus",
            Self::Mp3 => "mp3",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        [Self::Orig, Self::Opus, Self::Mp3].into_iter().find(|f| f.key() == s.trim())
    }

    /// A device can send its files in this form now.
    pub const fn ready(self) -> bool {
        matches!(self, Self::Orig)
    }
}

/// «Избранное»: the cap a new choice of it starts with.
pub const FAVORITES_CAP_DEFAULT: u64 = 16 << 30;
/// The cap's slider, in GiB.
pub const FAVORITES_CAP_MIN_GB: u64 = 4;
pub const FAVORITES_CAP_MAX_GB: u64 = 256;

/// What a device with «Избранное» keeps pinned besides its cache.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MeshPins {
    /// «Любимые».
    pub liked: bool,
    pub playlists: Vec<PlaylistId>,
}

/// The library disk of a device.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeshStorage {
    /// Bytes the music takes.
    pub used: u64,
    /// Bytes left on its disk.
    pub free: u64,
}

/// What a device plays, with its clock: the position is `position_ms` at
/// `at_ms` (that device's unix ms) moving at `rate` (0 while paused).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MeshPlaying {
    pub title: String,
    pub artists: Vec<String>,
    pub paused: bool,
    pub position_ms: i64,
    pub at_ms: i64,
    pub rate: f32,
    #[serde(default)]
    pub duration_ms: Option<i64>,
}

/// A running transfer to a device (the replica wave).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeshTransfer {
    pub done: u64,
    pub total: u64,
    /// Paused here ([`crate::Command::MeshPauseTransfer`]).
    #[serde(default)]
    pub paused: bool,
    /// Bytes a second now (0 while paused or waiting).
    #[serde(default)]
    pub speed: u64,
}

impl MeshPlaying {
    /// Where the song is at `now_ms` (that device's clock, close enough to
    /// this one's on a home network), within its length.
    pub fn position_at(&self, now_ms: i64) -> i64 {
        let run = if self.paused { 0.0 } else { f64::from(self.rate) * (now_ms - self.at_ms).max(0) as f64 };
        let p = self.position_ms + run as i64;
        match self.duration_ms {
            Some(d) if d > 0 => p.clamp(0, d),
            _ => p.max(0),
        }
    }
}

/// One device.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshNodeView {
    /// Its node id (hex of its Ed25519 public key).
    pub id: String,
    pub name: String,
    pub kind: MeshKind,
    pub scope: MeshScope,
    pub online: bool,
    pub last_seen_ms: Option<i64>,
    /// `ip:port` it was last reached at.
    pub addr: Option<String>,
    pub version: Option<String>,
    pub storage: Option<MeshStorage>,
    pub playing: Option<MeshPlaying>,
    pub profile: MeshProfile,
    pub transfer: Option<MeshTransfer>,
    /// «Любимые» and the playlists it keeps with «Избранное» (chosen here).
    pub pins: MeshPins,
    /// The library's sync with it.
    pub sync: MeshSyncView,
}

/// «Забыть» of a device stopped by the check made at that moment: lossless
/// originals have their only verified copy there (none on another trusted
/// device, none left on this computer). The forget waits for an answer
/// ([`crate::Command::MeshForgetAnswer`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MeshForgetAsk {
    pub node_id: String,
    pub name: String,
    /// How many; `None` when the check failed (asked as if some would go).
    pub originals: Option<u64>,
}

/// The library's sync with a device: its switch (`MeshSetSync`), a pull
/// running and how far it is (in the device's change numbers: a first sync
/// of a library is a long one), the last pull that brought something, why
/// the last one failed, what of it is kept to try again.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MeshSyncView {
    /// Turned off for this device.
    pub off: bool,
    pub running: bool,
    pub done: u64,
    pub total: u64,
    /// When the last pull that brought rows ended (unix ms).
    pub last_ms: Option<i64>,
    /// Why the last pull failed, or that rows of it wait for the owner
    /// ([`Self::conflicts`]): a blocker to show, as data — its words are
    /// made when the row is drawn.
    pub error: Option<MeshSyncError>,
    /// Its artists, albums, streams this device has under another id: kept
    /// until the owner decides.
    pub conflicts: u32,
    /// Its rows that cannot be written yet (tried again).
    pub waiting: u32,
    /// Its deletes held back while the row is still used here (a file of
    /// this device keeps a song): not an error.
    pub held: u32,
}

impl MeshSyncView {
    /// How far a running pull is, 0..=1.
    pub fn fraction(&self) -> f32 {
        if self.total == 0 { 0.0 } else { (self.done as f64 / self.total as f64).clamp(0.0, 1.0) as f32 }
    }
}

/// Why the mesh service cannot run, as data: its words are made when the
/// settings draw it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeshServiceError {
    /// The network could not be opened: the reason.
    Network(String),
}

impl MeshServiceError {
    /// The words of `self` in `loc`.
    pub fn text_in(&self, loc: Locale) -> String {
        match self {
            Self::Network(why) => nikgob_core::i18n::mesh::fmt_open_net_in(loc, why),
        }
    }
}

/// Why a device's sync row shows a blocker, as data: its words are made
/// when the row is drawn, so a language change reaches a row already shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeshSyncError {
    /// Syncing is off on this database.
    Blocked,
    /// The device does not take sync (turned off there, not an owner).
    Refused,
    /// Its clock runs ahead: nothing of it is taken.
    ClockAhead,
    /// The pull failed.
    Failed,
    /// Its rows wait for the owner: how many.
    Conflicts(u32),
}

impl MeshSyncError {
    /// The words of `self` in `loc`.
    pub fn text_in(&self, loc: Locale) -> String {
        let s = nikgob_core::i18n::mesh_in(loc);
        match self {
            Self::Blocked => s.sync_blocked.to_owned(),
            Self::Refused => s.sync_refused.to_owned(),
            Self::ClockAhead => s.sync_clock_ahead.to_owned(),
            Self::Failed => s.sync_failed.to_owned(),
            Self::Conflicts(n) => nikgob_core::i18n::mesh::fmt_sync_conflicts_in(loc, *n),
        }
    }
}

/// An unpaired instance seen on the home network now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MeshNearbyView {
    pub id: String,
    pub name: String,
    pub kind: MeshKind,
    pub addr: Option<String>,
}

/// The pairing flow.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum MeshPairing {
    #[default]
    Idle,
    /// «Подключить» was pressed: the dialog asks for that device's code.
    WaitingCode { nearby_id: String },
    /// The exchange runs.
    Working,
    /// Why it did not pair, as data: the dialog says it in the UI's
    /// language when it draws it.
    Failed(MeshPairingError),
    /// The code the device showed was wrong: its window shakes, no word.
    FailedWrongCode,
    /// Paired: the new device's id.
    Done { node_id: String },
}

/// Why a pairing did not happen, as data: its words are made when the
/// dialog draws it, so a language change reaches a failure already shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeshPairingError {
    /// The mesh is off.
    MeshOff,
    /// The mesh service does not run.
    NotRunning,
    /// The invite could not be made.
    InviteNotMade,
    /// Only the ring's holder adds devices.
    OnlyHolderAdds,
    /// The ring could not be made.
    RingNotMade,
    /// The invite was cancelled.
    InviteCancelled,
    /// No such device.
    NoSuchDevice,
    /// The code must be six digits.
    CodeSixDigits,
    /// The device is gone.
    DeviceGone,
    /// It is in a ring already.
    AlreadyPaired,
    /// The other side would not pair: its refusal.
    Refused(MeshRefusal),
    /// The invite ticket was not taken.
    Ticket(MeshTicketError),
    /// The exchange broke.
    Protocol,
    /// No link to the device.
    NoLink,
    /// The ring could not be saved.
    SaveFailed,
}

/// Why the other side would not pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeshRefusal {
    WrongCode,
    /// No such invite, or it expired or was spent.
    NoInvite,
    /// Another pairing runs there.
    Busy,
    /// It is in a ring already.
    Paired,
    /// It cannot sign (not the ring's holder).
    NotHolder,
    Internal,
    /// The ring has no room for another device.
    Full,
}

/// Why an invite ticket was not taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeshTicketError {
    /// Not a ticket of this player.
    NotATicket,
    /// A ticket of a newer player.
    Version,
    Expired,
}

impl MeshTicketError {
    /// The words of `self` in `loc`.
    pub fn text_in(self, loc: Locale) -> String {
        let s = nikgob_core::i18n::mesh_in(loc);
        match self {
            Self::NotATicket => s.not_a_ticket,
            Self::Version => s.newer_invite,
            Self::Expired => s.invite_expired,
        }
        .to_owned()
    }
}

impl MeshPairingError {
    /// The words of `self` in `loc`.
    pub fn text_in(&self, loc: Locale) -> String {
        let s = nikgob_core::i18n::mesh_in(loc);
        match self {
            Self::MeshOff => s.mesh_off,
            Self::NotRunning => s.mesh_not_running,
            Self::InviteNotMade => s.invite_not_made,
            Self::OnlyHolderAdds => s.only_holder_adds,
            Self::RingNotMade => s.ring_not_made,
            Self::InviteCancelled => s.invite_cancelled,
            Self::NoSuchDevice => s.no_such_device,
            Self::CodeSixDigits => s.code_six_digits,
            Self::DeviceGone => s.device_gone,
            Self::AlreadyPaired => s.already_paired,
            Self::Refused(r) => match r {
                MeshRefusal::WrongCode => s.wrong_code,
                MeshRefusal::NoInvite => s.invite_gone,
                MeshRefusal::Busy => s.device_busy,
                MeshRefusal::Paired => s.device_paired,
                MeshRefusal::NotHolder => s.only_holder_pairs,
                MeshRefusal::Internal => s.bad_answer,
                MeshRefusal::Full => s.ring_full,
            },
            Self::Protocol => s.bad_answer,
            Self::Ticket(e) => return e.text_in(loc),
            Self::NoLink => s.no_link,
            Self::SaveFailed => s.pair_save_failed,
        }
        .to_owned()
    }
}

/// The invite being shown: the ticket for `nikgobd pair <ticket>` (or a
/// QR), until `expires_ms` (unix ms) or one use.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MeshInvite {
    pub ticket: String,
    pub expires_ms: i64,
    /// What a device that joins with it becomes («Гость» of the window).
    pub scope: MeshScope,
}

/// «Межсеть → Устройства».
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshView {
    /// `[mesh] enabled` of settings.toml (on unless switched off).
    pub enabled: bool,
    /// The mesh service runs (its endpoint is bound, mDNS announces it).
    pub running: bool,
    pub this: MeshNodeView,
    /// Paired devices, this one excluded.
    pub nodes: Vec<MeshNodeView>,
    /// Unpaired instances seen by mDNS now.
    pub nearby: Vec<MeshNearbyView>,
    pub pairing: MeshPairing,
    pub invite: Option<MeshInvite>,
    /// This instance holds the ring's key: it pairs and forgets devices.
    pub ring_holder: bool,
    /// This instance is in a ring.
    pub paired: bool,
    /// This instance's 6-digit code while it is in no ring (another
    /// device's «Подключить» asks for it).
    pub code: Option<String>,
    /// Why the service cannot run (no network, a port), as data: its words
    /// are made when the settings draw it.
    pub error: Option<MeshServiceError>,
    /// `[mesh] name` of settings.toml (empty: the computer's name).
    pub name_setting: String,
    /// The settings can be saved (there is a settings.toml).
    pub configurable: bool,
    /// «Оригиналы FLAC на ПК» (`[mesh] keep_masters`).
    pub keep_masters: KeepMasters,
    /// A forget that waits for the owner: it would lose originals.
    pub forget_ask: Option<MeshForgetAsk>,
}

impl MeshView {
    /// The devices there are: this one and the paired ones.
    pub fn device_count(&self) -> usize {
        1 + self.nodes.len()
    }

    /// This instance may start a pairing: it holds the ring, or is in none
    /// (the first pairing makes the ring here).
    pub fn can_pair(&self) -> bool {
        self.running && (self.ring_holder || !self.paired)
    }

    pub fn node(&self, id: &str) -> Option<&MeshNodeView> {
        self.nodes.iter().find(|n| n.id == id)
    }
}

/// A paired device as the mesh service knows it: the view's row and its
/// certificate (JSON) for the node table.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshSnapshotNode {
    pub view: MeshNodeView,
    pub cert: String,
}

/// What the mesh service reports to the runtime (only when it changed).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshSnapshot {
    pub running: bool,
    pub this: MeshNodeView,
    pub nodes: Vec<MeshSnapshotNode>,
    pub nearby: Vec<MeshNearbyView>,
    pub pairing: MeshPairing,
    pub invite: Option<MeshInvite>,
    pub ring_holder: bool,
    pub paired: bool,
    pub code: Option<String>,
    pub error: Option<MeshServiceError>,
}

/// «Компьютер», «Raspberry Pi»…: a device's kind in words, in the active locale.
pub fn kind_label(k: MeshKind) -> &'static str {
    let s = nikgob_core::i18n::mesh();
    match k {
        MeshKind::Pc => s.kind_pc,
        MeshKind::Pi => s.kind_pi,
        MeshKind::Phone => s.kind_phone,
        MeshKind::Tv => s.kind_tv,
        MeshKind::Server => s.kind_server,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pairing's failure and a sync row's blocker are data: the same one
    /// reads in both languages.
    #[test]
    fn failures_read_in_both_languages() {
        assert_eq!(MeshPairingError::MeshOff.text_in(Locale::Ru), "Межсеть выключена");
        assert_eq!(MeshPairingError::MeshOff.text_in(Locale::En), "Mesh is off");
        assert_eq!(MeshPairingError::NoLink.text_in(Locale::Ru), "Нет связи с устройством");
        assert_eq!(MeshPairingError::NoLink.text_in(Locale::En), "Cannot connect to the device");
        assert_eq!(MeshPairingError::Refused(MeshRefusal::Full).text_in(Locale::En), "No room for another device in the ring");
        assert_eq!(MeshPairingError::Ticket(MeshTicketError::Expired).text_in(Locale::Ru), "Приглашение истекло");
        assert_eq!(MeshSyncError::Conflicts(3).text_in(Locale::Ru), "Не все изменения приняты: 3");
        assert_eq!(MeshSyncError::Conflicts(3).text_in(Locale::En), "Not all changes accepted: 3");
        assert_eq!(MeshServiceError::Network("x".into()).text_in(Locale::Ru), "Не удалось открыть сеть: x");
        assert_eq!(MeshServiceError::Network("x".into()).text_in(Locale::En), "Could not open the network: x");
    }

    #[test]
    fn words_round_trip() {
        for k in MeshKind::ALL {
            assert_eq!(MeshKind::parse(k.key()), Some(k));
        }
        assert_eq!(MeshKind::parse(" PI "), Some(MeshKind::Pi));
        assert_eq!(MeshKind::parse("toaster"), None);
        for s in [MeshScope::Owner, MeshScope::Guest] {
            assert_eq!(MeshScope::parse(s.key()), Some(s));
        }
        for p in [MeshProfile::Full, MeshProfile::Library, MeshProfile::Favorites { cap_bytes: 8 << 30 }, MeshProfile::SoundOnly] {
            assert_eq!(MeshProfile::parse(&p.key()), Some(p));
        }
        assert_eq!(MeshProfile::parse("favorites:x"), None);
        for f in [MeshFormat::Orig, MeshFormat::Opus, MeshFormat::Mp3] {
            assert_eq!(MeshFormat::parse(f.key()), Some(f));
        }
        assert_eq!(MeshProfile::Favorites { cap_bytes: 1 }.format(), MeshFormat::Orig, "originals only for now");
        assert!(MeshFormat::Orig.ready() && !MeshFormat::Mp3.ready());
    }

    #[test]
    fn who_may_pair() {
        let mut v = MeshView { running: true, ..MeshView::default() };
        assert!(v.can_pair(), "in no ring: the first pairing makes one");
        v.paired = true;
        assert!(!v.can_pair(), "a member that does not hold the ring");
        v.ring_holder = true;
        assert!(v.can_pair());
        v.running = false;
        assert!(!v.can_pair());
    }

    #[test]
    fn this_computer_counts_among_the_devices() {
        let mut v = MeshView::default();
        assert_eq!(v.device_count(), 1);
        v.nodes.push(MeshNodeView { id: "pi".into(), ..MeshNodeView::default() });
        assert_eq!(v.device_count(), 2);
    }

    #[test]
    fn a_remote_song_moves_with_its_clock() {
        let p = MeshPlaying { position_ms: 10_000, at_ms: 1_000, rate: 1.0, duration_ms: Some(12_000), ..MeshPlaying::default() };
        assert_eq!(p.position_at(2_000), 11_000);
        assert_eq!(p.position_at(9_000), 12_000, "never past its end");
        assert_eq!(p.position_at(500), 10_000, "a clock a little behind");
        let paused = MeshPlaying { paused: true, ..p.clone() };
        assert_eq!(paused.position_at(5_000), 10_000);
        assert_eq!(MeshProfile::Favorites { cap_bytes: 1 }.label(), "Избранное");
        assert!(!MeshProfile::SoundOnly.stores());
    }

    #[test]
    fn presence_survives_json() {
        let p = MeshPlaying { title: "T".into(), artists: vec!["A".into()], paused: false, position_ms: 1, at_ms: 2, rate: 1.0, duration_ms: None };
        let back: MeshPlaying = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(back, p);
        assert_eq!(serde_json::to_string(&MeshKind::Pi).unwrap(), "\"pi\"");
    }
}

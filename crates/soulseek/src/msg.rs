//! The messages of the protocol this client speaks, in both directions (the
//! tests' mock server and peers use the other side). Layouts follow the
//! Nicotine+ protocol documentation (`doc/SLSKPROTOCOL.md`); codes not
//! listed here are read as `Other` and ignored.

use std::net::Ipv4Addr;

use md5::{Digest as _, Md5};

use crate::codec::{Malformed, Reader, Writer, frame_u8, frame_u32, zlib_compress, zlib_decompress};

/// The largest inflated shared-file list or search response accepted.
pub const INFLATE_LIMIT: usize = 64 << 20;

/// Experimental clients use major version 177 (Constants → Major Versions).
pub const MAJOR_VERSION: u32 = 177;
pub const MINOR_VERSION: u32 = 1;

/// The kind of a peer connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ConnType {
    /// Peer messages.
    P,
    /// One file transfer.
    F,
    /// The distributed search network.
    D,
}

impl ConnType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::P => "P",
            Self::F => "F",
            Self::D => "D",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "P" => Some(Self::P),
            "F" => Some(Self::F),
            "D" => Some(Self::D),
            _ => None,
        }
    }
}

fn read_conn_type(r: &mut Reader<'_>) -> Result<ConnType, Malformed> {
    ConnType::parse(&r.str()?).ok_or(Malformed("unknown connection type"))
}

/// MD5 hex digest, as the login wants it.
pub fn md5_hex(s: &str) -> String {
    let d = Md5::digest(s.as_bytes());
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// What a client sends to the server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToServer {
    Login { username: String, password: String, major: u32, minor: u32 },
    SetWaitPort(u32),
    GetPeerAddress(String),
    ConnectToPeer { token: u32, user: String, kind: ConnType },
    MessageAcked(u32),
    FileSearch { token: u32, query: String },
    SetStatus(i32),
    Ping,
    SharedFoldersFiles { dirs: u32, files: u32 },
    HaveNoParent(bool),
    AcceptChildren(bool),
    BranchLevel(u32),
    BranchRoot(String),
    SendUploadSpeed(u32),
    CantConnectToPeer { token: u32, user: String },
    Other(u32),
}

impl ToServer {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        let code = match self {
            Self::Login { username, password, major, minor } => {
                let hash = md5_hex(&format!("{username}{password}"));
                w.str(username).str(password).u32(*major).str(&hash).u32(*minor);
                1
            }
            Self::SetWaitPort(p) => {
                w.u32(*p);
                2
            }
            Self::GetPeerAddress(u) => {
                w.str(u);
                3
            }
            Self::ConnectToPeer { token, user, kind } => {
                w.u32(*token).str(user).str(kind.as_str());
                18
            }
            Self::MessageAcked(id) => {
                w.u32(*id);
                23
            }
            Self::FileSearch { token, query } => {
                w.u32(*token).str(query);
                26
            }
            Self::SetStatus(s) => {
                w.i32(*s);
                28
            }
            Self::Ping => 32,
            Self::SharedFoldersFiles { dirs, files } => {
                w.u32(*dirs).u32(*files);
                35
            }
            Self::HaveNoParent(b) => {
                w.bool(*b);
                71
            }
            Self::AcceptChildren(b) => {
                w.bool(*b);
                100
            }
            Self::BranchLevel(l) => {
                w.u32(*l);
                126
            }
            Self::BranchRoot(r) => {
                w.str(r);
                127
            }
            Self::SendUploadSpeed(s) => {
                w.u32(*s);
                121
            }
            Self::CantConnectToPeer { token, user } => {
                w.u32(*token).str(user);
                1001
            }
            Self::Other(c) => *c,
        };
        frame_u32(code, &w.into_inner())
    }

    /// The server's side of [`Self::encode`] (the mock server).
    pub fn decode(code: u32, body: &[u8]) -> Result<Self, Malformed> {
        let mut r = Reader::new(body);
        Ok(match code {
            1 => {
                let username = r.str()?;
                let password = r.str()?;
                let major = r.u32()?;
                let _hash = r.str()?;
                let minor = r.u32()?;
                Self::Login { username, password, major, minor }
            }
            2 => Self::SetWaitPort(r.u32()?),
            3 => Self::GetPeerAddress(r.str()?),
            18 => {
                let token = r.u32()?;
                let user = r.str()?;
                Self::ConnectToPeer { token, user, kind: read_conn_type(&mut r)? }
            }
            23 => Self::MessageAcked(r.u32()?),
            26 => {
                let token = r.u32()?;
                Self::FileSearch { token, query: r.str()? }
            }
            28 => Self::SetStatus(r.i32()?),
            32 => Self::Ping,
            35 => Self::SharedFoldersFiles { dirs: r.u32()?, files: r.u32()? },
            71 => Self::HaveNoParent(r.bool()?),
            100 => Self::AcceptChildren(r.bool()?),
            126 => Self::BranchLevel(r.u32()?),
            127 => Self::BranchRoot(r.str()?),
            121 => Self::SendUploadSpeed(r.u32()?),
            1001 => {
                let token = r.u32()?;
                Self::CantConnectToPeer { token, user: r.str()? }
            }
            c => Self::Other(c),
        })
    }
}

/// What the server sends to a client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FromServer {
    LoginOk { greet: String, ip: Ipv4Addr, supporter: bool },
    LoginFailed { reason: String, detail: Option<String> },
    PeerAddress { user: String, ip: Ipv4Addr, port: u32 },
    ConnectToPeer { user: String, kind: ConnType, ip: Ipv4Addr, port: u32, token: u32, privileged: bool },
    MessageUser { id: u32, timestamp: u32, user: String, message: String },
    /// Someone searches (user and room searches come this way).
    FileSearch { user: String, token: u32, query: String },
    Relogged,
    /// A distributed message straight from the server: we are a branch root.
    EmbeddedMessage { code: u8, body: Vec<u8> },
    PossibleParents(Vec<(String, Ipv4Addr, u32)>),
    ResetDistributed,
    ExcludedSearchPhrases(Vec<String>),
    CantConnectToPeer { token: u32 },
    Other(u32),
}

impl FromServer {
    pub fn decode(code: u32, body: &[u8]) -> Result<Self, Malformed> {
        let mut r = Reader::new(body);
        Ok(match code {
            1 => {
                if r.bool()? {
                    let greet = r.str()?;
                    let ip = r.ip()?;
                    // The password hash and the supporter flag came later;
                    // older servers end here.
                    let _hash = if r.at_end() { String::new() } else { r.str()? };
                    let supporter = !r.at_end() && r.bool()?;
                    Self::LoginOk { greet, ip, supporter }
                } else {
                    let reason = r.str()?;
                    let detail = if r.at_end() { None } else { Some(r.str()?) };
                    Self::LoginFailed { reason, detail }
                }
            }
            3 => {
                let user = r.str()?;
                let ip = r.ip()?;
                Self::PeerAddress { user, ip, port: r.u32()? }
            }
            18 => {
                let user = r.str()?;
                let kind = read_conn_type(&mut r)?;
                let ip = r.ip()?;
                let port = r.u32()?;
                let token = r.u32()?;
                let privileged = !r.at_end() && r.bool()?;
                // The obfuscation ports came later; older servers send
                // neither (this client does not use them, it only reads
                // them so a longer message is no error).
                if r.remaining() >= 4 {
                    let _obfuscated_port = r.u32()?;
                }
                if r.remaining() >= 4 {
                    let _second_port = r.u32()?;
                }
                Self::ConnectToPeer { user, kind, ip, port, token, privileged }
            }
            22 => {
                let id = r.u32()?;
                let timestamp = r.u32()?;
                let user = r.str()?;
                Self::MessageUser { id, timestamp, user, message: r.str()? }
            }
            26 => {
                let user = r.str()?;
                let token = r.u32()?;
                Self::FileSearch { user, token, query: r.str()? }
            }
            41 => Self::Relogged,
            93 => {
                let code = r.u8()?;
                Self::EmbeddedMessage { code, body: r.rest().to_vec() }
            }
            102 => {
                let n = r.count(12)?;
                let mut v = Vec::with_capacity(n);
                for _ in 0..n {
                    let user = r.str()?;
                    let ip = r.ip()?;
                    v.push((user, ip, r.u32()?));
                }
                Self::PossibleParents(v)
            }
            130 => Self::ResetDistributed,
            160 => {
                let n = r.count(4)?;
                let mut v = Vec::with_capacity(n);
                for _ in 0..n {
                    v.push(r.str()?);
                }
                Self::ExcludedSearchPhrases(v)
            }
            1001 => Self::CantConnectToPeer { token: r.u32()? },
            c => Self::Other(c),
        })
    }

    /// The server's side (the mock server).
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        let code = match self {
            Self::LoginOk { greet, ip, supporter } => {
                w.bool(true).str(greet).ip(*ip).str(&md5_hex("")).bool(*supporter);
                1
            }
            Self::LoginFailed { reason, detail } => {
                w.bool(false).str(reason);
                if let Some(d) = detail {
                    w.str(d);
                }
                1
            }
            Self::PeerAddress { user, ip, port } => {
                w.str(user).ip(*ip).u32(*port).u32(0).u16(0);
                3
            }
            Self::ConnectToPeer { user, kind, ip, port, token, privileged } => {
                w.str(user).str(kind.as_str()).ip(*ip).u32(*port).u32(*token).bool(*privileged).u32(0).u32(0);
                18
            }
            Self::MessageUser { id, timestamp, user, message } => {
                w.u32(*id).u32(*timestamp).str(user).str(message).bool(true);
                22
            }
            Self::FileSearch { user, token, query } => {
                w.str(user).u32(*token).str(query);
                26
            }
            Self::Relogged => 41,
            Self::EmbeddedMessage { code, body } => {
                w.u8(*code).raw(body);
                93
            }
            Self::PossibleParents(v) => {
                w.u32(v.len() as u32);
                for (u, ip, port) in v {
                    w.str(u).ip(*ip).u32(*port);
                }
                102
            }
            Self::ResetDistributed => 130,
            Self::ExcludedSearchPhrases(v) => {
                w.u32(v.len() as u32);
                for p in v {
                    w.str(p);
                }
                160
            }
            Self::CantConnectToPeer { token } => {
                w.u32(*token);
                1001
            }
            Self::Other(c) => *c,
        };
        frame_u32(code, &w.into_inner())
    }
}

/// The first message on a new peer connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PeerInit {
    /// The answer to an indirect connection request, with its token.
    PierceFirewall(u32),
    /// A direct connection: who we are and what the connection is for.
    Init { user: String, kind: ConnType, token: u32 },
}

impl PeerInit {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        match self {
            Self::PierceFirewall(t) => {
                w.u32(*t);
                frame_u8(0, &w.into_inner())
            }
            Self::Init { user, kind, token } => {
                w.str(user).str(kind.as_str()).u32(*token);
                frame_u8(1, &w.into_inner())
            }
        }
    }

    pub fn decode(code: u8, body: &[u8]) -> Result<Self, Malformed> {
        let mut r = Reader::new(body);
        match code {
            0 => Ok(Self::PierceFirewall(r.u32()?)),
            1 => {
                let user = r.str()?;
                let kind = read_conn_type(&mut r)?;
                // Some clients leave the (always zero) token out.
                let token = if r.remaining() >= 4 { r.u32()? } else { 0 };
                Ok(Self::Init { user, kind, token })
            }
            _ => Err(Malformed("unknown peer init code")),
        }
    }
}

/// A file in a search result, a shared list or a folder listing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileEntry {
    /// The full virtual path (search results) or the name inside its
    /// directory (lists).
    pub name: String,
    pub size: u64,
    pub ext: String,
    /// (attribute code, value): 0 bitrate, 1 duration, 2 VBR, 4 sample rate,
    /// 5 bit depth.
    pub attrs: Vec<(u32, u32)>,
}

impl FileEntry {
    fn write(&self, w: &mut Writer) {
        w.u8(1).str(&self.name).u64(self.size).str(&self.ext).u32(self.attrs.len() as u32);
        for (c, v) in &self.attrs {
            w.u32(*c).u32(*v);
        }
    }

    fn read(r: &mut Reader<'_>) -> Result<Self, Malformed> {
        let _code = r.u8()?;
        let name = r.str()?;
        let size = r.u64()?;
        let ext = r.str()?;
        let n = r.count(8)?;
        let mut attrs = Vec::with_capacity(n);
        for _ in 0..n {
            let c = r.u32()?;
            attrs.push((c, r.u32()?));
        }
        Ok(Self { name, size, ext, attrs })
    }

    pub fn attr(&self, code: u32) -> Option<u32> {
        self.attrs.iter().find(|(c, _)| *c == code).map(|(_, v)| *v)
    }
}

fn write_files(w: &mut Writer, files: &[FileEntry]) {
    w.u32(files.len() as u32);
    for f in files {
        f.write(w);
    }
}

fn read_files(r: &mut Reader<'_>) -> Result<Vec<FileEntry>, Malformed> {
    let n = r.count(21)?;
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        v.push(FileEntry::read(r)?);
    }
    Ok(v)
}

/// A directory with its files.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub files: Vec<FileEntry>,
}

fn write_dirs(w: &mut Writer, dirs: &[DirEntry]) {
    w.u32(dirs.len() as u32);
    for d in dirs {
        w.str(&d.name);
        write_files(w, &d.files);
    }
}

fn read_dirs(r: &mut Reader<'_>) -> Result<Vec<DirEntry>, Malformed> {
    let n = r.count(8)?;
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        let name = r.str()?;
        v.push(DirEntry { name, files: read_files(r)? });
    }
    Ok(v)
}

/// A search response.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchReply {
    pub user: String,
    pub token: u32,
    pub files: Vec<FileEntry>,
    pub slot_free: bool,
    pub avg_speed: u32,
    pub queue_len: u32,
    /// Files shared only with the sharer's buddies (not downloadable).
    pub private: Vec<FileEntry>,
}

/// Messages on a `P` connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PeerMsg {
    SharedFileListRequest,
    SharedFileListResponse(Vec<DirEntry>),
    FileSearchResponse(SearchReply),
    UserInfoRequest,
    UserInfoResponse { description: String, total_uploads: u32, queue_size: u32, slots_free: bool },
    FolderContentsRequest { token: u32, folder: String },
    FolderContentsResponse { token: u32, folder: String, dirs: Vec<DirEntry> },
    /// `direction` 1: the peer is ready to upload `filename` to us (with
    /// its size); 0: a legacy download request.
    TransferRequest { direction: u32, token: u32, filename: String, size: Option<u64> },
    TransferResponse { token: u32, allowed: bool, size: Option<u64>, reason: Option<String> },
    QueueUpload(String),
    PlaceInQueueResponse { filename: String, place: u32 },
    UploadFailed(String),
    UploadDenied { filename: String, reason: String },
    PlaceInQueueRequest(String),
    Other(u32),
}

impl PeerMsg {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        let code = match self {
            Self::SharedFileListRequest => 4,
            Self::SharedFileListResponse(dirs) => {
                let mut inner = Writer::new();
                write_dirs(&mut inner, dirs);
                inner.u32(0).u32(0);
                w.raw(&zlib_compress(&inner.into_inner()));
                5
            }
            Self::FileSearchResponse(s) => {
                let mut inner = Writer::new();
                inner.str(&s.user).u32(s.token);
                write_files(&mut inner, &s.files);
                inner.bool(s.slot_free).u32(s.avg_speed).u32(s.queue_len).u32(0);
                write_files(&mut inner, &s.private);
                w.raw(&zlib_compress(&inner.into_inner()));
                9
            }
            Self::UserInfoRequest => 15,
            Self::UserInfoResponse { description, total_uploads, queue_size, slots_free } => {
                w.str(description).bool(false).u32(*total_uploads).u32(*queue_size).bool(*slots_free).u32(0);
                16
            }
            Self::FolderContentsRequest { token, folder } => {
                w.u32(*token).str(folder);
                36
            }
            Self::FolderContentsResponse { token, folder, dirs } => {
                let mut inner = Writer::new();
                inner.u32(*token).str(folder);
                write_dirs(&mut inner, dirs);
                w.raw(&zlib_compress(&inner.into_inner()));
                37
            }
            Self::TransferRequest { direction, token, filename, size } => {
                w.u32(*direction).u32(*token).str(filename);
                if *direction == 1 {
                    w.u64(size.unwrap_or(0));
                }
                40
            }
            Self::TransferResponse { token, allowed, size, reason } => {
                w.u32(*token).bool(*allowed);
                if *allowed {
                    if let Some(s) = size {
                        w.u64(*s);
                    }
                } else {
                    w.str(reason.as_deref().unwrap_or("Cancelled"));
                }
                41
            }
            Self::QueueUpload(f) => {
                w.str(f);
                43
            }
            Self::PlaceInQueueResponse { filename, place } => {
                w.str(filename).u32(*place);
                44
            }
            Self::UploadFailed(f) => {
                w.str(f);
                46
            }
            Self::UploadDenied { filename, reason } => {
                w.str(filename).str(reason);
                50
            }
            Self::PlaceInQueueRequest(f) => {
                w.str(f);
                51
            }
            Self::Other(c) => *c,
        };
        frame_u32(code, &w.into_inner())
    }

    pub fn decode(code: u32, body: &[u8]) -> Result<Self, Malformed> {
        let inflate = |b: &[u8]| zlib_decompress(b, INFLATE_LIMIT).map_err(|_| Malformed("bad zlib data"));
        let mut r = Reader::new(body);
        Ok(match code {
            4 => Self::SharedFileListRequest,
            5 => {
                let data = inflate(body)?;
                let mut r = Reader::new(&data);
                Self::SharedFileListResponse(read_dirs(&mut r)?)
            }
            9 => {
                let data = inflate(body)?;
                let mut r = Reader::new(&data);
                let user = r.str()?;
                let token = r.u32()?;
                let files = read_files(&mut r)?;
                let slot_free = r.bool()?;
                let avg_speed = r.u32()?;
                let queue_len = r.u32()?;
                let mut private = Vec::new();
                if r.remaining() >= 8 {
                    let _unknown = r.u32()?;
                    private = read_files(&mut r)?;
                }
                Self::FileSearchResponse(SearchReply { user, token, files, slot_free, avg_speed, queue_len, private })
            }
            15 => Self::UserInfoRequest,
            16 => {
                let description = r.str()?;
                if r.bool()? {
                    let _picture = r.bytes()?;
                }
                let total_uploads = r.u32()?;
                let queue_size = r.u32()?;
                let slots_free = r.bool()?;
                Self::UserInfoResponse { description, total_uploads, queue_size, slots_free }
            }
            36 => {
                let token = r.u32()?;
                Self::FolderContentsRequest { token, folder: r.str()? }
            }
            37 => {
                let data = inflate(body)?;
                let mut r = Reader::new(&data);
                let token = r.u32()?;
                let folder = r.str()?;
                Self::FolderContentsResponse { token, folder, dirs: read_dirs(&mut r)? }
            }
            40 => {
                let direction = r.u32()?;
                let token = r.u32()?;
                let filename = r.str()?;
                let size = if direction == 1 && r.remaining() >= 8 { Some(r.u64()?) } else { None };
                Self::TransferRequest { direction, token, filename, size }
            }
            41 => {
                let token = r.u32()?;
                let allowed = r.bool()?;
                let (size, reason) = if allowed {
                    (if r.remaining() >= 8 { Some(r.u64()?) } else { None }, None)
                } else {
                    (None, if r.at_end() { None } else { Some(r.str()?) })
                };
                Self::TransferResponse { token, allowed, size, reason }
            }
            43 => Self::QueueUpload(r.str()?),
            44 => {
                let filename = r.str()?;
                Self::PlaceInQueueResponse { filename, place: r.u32()? }
            }
            46 => Self::UploadFailed(r.str()?),
            50 => {
                let filename = r.str()?;
                Self::UploadDenied { filename, reason: r.str()? }
            }
            51 => Self::PlaceInQueueRequest(r.str()?),
            c => Self::Other(c),
        })
    }
}

/// Messages on a `D` connection (and embedded in server messages).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DistribMsg {
    Ping,
    Search { user: String, token: u32, query: String },
    BranchLevel(i32),
    BranchRoot(String),
    ChildDepth(u32),
    /// A distributed message wrapped once more (older SoulseekQt).
    Embedded { code: u8, body: Vec<u8> },
    Other(u8),
}

/// The identifier every distributed search starts with.
const SEARCH_IDENTIFIER: u32 = 49;

impl DistribMsg {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        let code = match self {
            Self::Ping => 0,
            Self::Search { user, token, query } => {
                w.u32(SEARCH_IDENTIFIER).str(user).u32(*token).str(query);
                3
            }
            Self::BranchLevel(l) => {
                w.i32(*l);
                4
            }
            Self::BranchRoot(r) => {
                w.str(r);
                5
            }
            Self::ChildDepth(d) => {
                w.u32(*d);
                7
            }
            Self::Embedded { code, body } => {
                w.u8(*code).raw(body);
                93
            }
            Self::Other(c) => *c,
        };
        frame_u8(code, &w.into_inner())
    }

    /// The body of a search, as the server embeds it.
    pub fn search_body(user: &str, token: u32, query: &str) -> Vec<u8> {
        let mut w = Writer::new();
        w.u32(SEARCH_IDENTIFIER).str(user).u32(token).str(query);
        w.into_inner()
    }

    pub fn decode(code: u8, body: &[u8]) -> Result<Self, Malformed> {
        let mut r = Reader::new(body);
        Ok(match code {
            0 => Self::Ping,
            3 => {
                if r.u32()? != SEARCH_IDENTIFIER {
                    return Err(Malformed("search with an unknown identifier"));
                }
                let user = r.str()?;
                let token = r.u32()?;
                Self::Search { user, token, query: r.str()? }
            }
            4 => Self::BranchLevel(r.i32()?),
            5 => Self::BranchRoot(r.str()?),
            7 => Self::ChildDepth(r.u32()?),
            93 => {
                let code = r.u8()?;
                Self::Embedded { code, body: r.rest().to_vec() }
            }
            c => Self::Other(c),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::{split_u8, split_u32};

    fn server_round_trip(m: ToServer) {
        let f = m.encode();
        let (code, body) = split_u32(&f[4..]).unwrap();
        assert_eq!(ToServer::decode(code, body).unwrap(), m);
    }

    fn from_server_round_trip(m: FromServer) {
        let f = m.encode();
        let (code, body) = split_u32(&f[4..]).unwrap();
        assert_eq!(FromServer::decode(code, body).unwrap(), m);
    }

    fn peer_round_trip(m: PeerMsg) {
        let f = m.encode();
        let (code, body) = split_u32(&f[4..]).unwrap();
        assert_eq!(PeerMsg::decode(code, body).unwrap(), m);
    }

    #[test]
    fn login_hash_is_md5_of_user_and_password() {
        assert_eq!(md5_hex("usernamepassword"), "d51c9a7e9353746a6020f9602d452929");
    }

    /// ConnectToPeer messages: the oldest shape ends at the token, newer
    /// ones add the privileged flag, the newest two obfuscation ports
    /// after it. None of the trailing fields is a must: all decode.
    #[test]
    fn connect_to_peer_reads_short_and_long_lays() {
        let expected = |privileged| FromServer::ConnectToPeer {
            user: "bob".into(),
            kind: ConnType::F,
            ip: "1.2.3.4".parse().unwrap(),
            port: 2240,
            token: 40,
            privileged,
        };
        let head = |w: &mut Writer| {
            w.str("bob").str("F").ip("1.2.3.4".parse().unwrap()).u32(2240).u32(40);
        };
        let mut short = Writer::new();
        head(&mut short);
        assert_eq!(FromServer::decode(18, &short.into_inner()).unwrap(), expected(false));
        let mut middle = Writer::new();
        head(&mut middle);
        middle.bool(true);
        assert_eq!(FromServer::decode(18, &middle.into_inner()).unwrap(), expected(true));
        let mut long = Writer::new();
        head(&mut long);
        long.bool(true).u32(2240).u32(0);
        assert_eq!(FromServer::decode(18, &long.into_inner()).unwrap(), expected(true));
    }

    #[test]
    fn server_messages_round_trip() {
        server_round_trip(ToServer::Login { username: "u".into(), password: "p".into(), major: 177, minor: 1 });
        server_round_trip(ToServer::ConnectToPeer { token: 9, user: "bob".into(), kind: ConnType::F });
        server_round_trip(ToServer::FileSearch { token: 3, query: "aimer flac".into() });
        server_round_trip(ToServer::CantConnectToPeer { token: 3, user: "x".into() });
        from_server_round_trip(FromServer::LoginOk { greet: "hi".into(), ip: "8.8.4.4".parse().unwrap(), supporter: false });
        from_server_round_trip(FromServer::LoginFailed { reason: "INVALIDPASS".into(), detail: None });
        from_server_round_trip(FromServer::PeerAddress { user: "a".into(), ip: "1.2.3.4".parse().unwrap(), port: 2234 });
        from_server_round_trip(FromServer::ConnectToPeer {
            user: "a".into(),
            kind: ConnType::P,
            ip: "1.2.3.4".parse().unwrap(),
            port: 5,
            token: 77,
            privileged: false,
        });
        from_server_round_trip(FromServer::PossibleParents(vec![("p".into(), "4.3.2.1".parse().unwrap(), 2234)]));
        from_server_round_trip(FromServer::EmbeddedMessage { code: 3, body: DistribMsg::search_body("a", 1, "q") });
    }

    #[test]
    fn peer_messages_round_trip() {
        let file = FileEntry { name: "@@music\\A\\01.flac".into(), size: 1234, ext: "flac".into(), attrs: vec![(1, 200), (4, 44100), (5, 16)] };
        peer_round_trip(PeerMsg::FileSearchResponse(SearchReply {
            user: "me".into(),
            token: 5,
            files: vec![file.clone()],
            slot_free: true,
            avg_speed: 1000,
            queue_len: 2,
            private: vec![],
        }));
        peer_round_trip(PeerMsg::SharedFileListResponse(vec![DirEntry { name: "@@music\\A".into(), files: vec![file.clone()] }]));
        peer_round_trip(PeerMsg::FolderContentsResponse { token: 1, folder: "x".into(), dirs: vec![] });
        peer_round_trip(PeerMsg::TransferRequest { direction: 1, token: 2, filename: "f".into(), size: Some(10) });
        peer_round_trip(PeerMsg::TransferRequest { direction: 0, token: 2, filename: "f".into(), size: None });
        peer_round_trip(PeerMsg::TransferResponse { token: 2, allowed: true, size: None, reason: None });
        peer_round_trip(PeerMsg::TransferResponse { token: 2, allowed: false, size: None, reason: Some("Queued".into()) });
        peer_round_trip(PeerMsg::UploadDenied { filename: "f".into(), reason: "File not shared.".into() });
        peer_round_trip(PeerMsg::UserInfoResponse { description: String::new(), total_uploads: 2, queue_size: 0, slots_free: true });
    }

    #[test]
    fn distributed_and_init_round_trip() {
        for m in [
            DistribMsg::Search { user: "a".into(), token: 4, query: "q".into() },
            DistribMsg::BranchLevel(2),
            DistribMsg::BranchRoot("root".into()),
        ] {
            let f = m.encode();
            let (code, body) = split_u8(&f[4..]).unwrap();
            assert_eq!(DistribMsg::decode(code, body).unwrap(), m);
        }
        for m in [PeerInit::PierceFirewall(5), PeerInit::Init { user: "me".into(), kind: ConnType::D, token: 0 }] {
            let f = m.encode();
            let (code, body) = split_u8(&f[4..]).unwrap();
            assert_eq!(PeerInit::decode(code, body).unwrap(), m);
        }
    }
}

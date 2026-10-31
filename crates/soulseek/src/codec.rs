//! Packing of the protocol's values: little-endian integers, strings and byte
//! arrays with a `u32` length, and the length-prefixed frames every
//! connection type uses.

use std::io::{self, Read as _, Write as _};

use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

/// The message did not parse: too short, or a count that cannot be right.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("malformed message: {0}")]
pub struct Malformed(pub &'static str);

/// Builds a message body.
#[derive(Default)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.buf.push(v);
        self
    }

    pub fn bool(&mut self, v: bool) -> &mut Self {
        self.u8(u8::from(v))
    }

    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn i32(&mut self, v: i32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn str(&mut self, v: &str) -> &mut Self {
        self.bytes(v.as_bytes())
    }

    pub fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.u32(v.len() as u32);
        self.buf.extend_from_slice(v);
        self
    }

    /// Bytes without a length (the rest of a message).
    pub fn raw(&mut self, v: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(v);
        self
    }

    /// An IPv4 address: the protocol sends it as a `u32` whose most
    /// significant byte is the first octet.
    pub fn ip(&mut self, ip: std::net::Ipv4Addr) -> &mut Self {
        self.u32(u32::from(ip))
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn into_inner(self) -> Vec<u8> {
        self.buf
    }
}

/// Reads a message body.
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], Malformed> {
        let end = self.pos.checked_add(n).ok_or(Malformed("length overflow"))?;
        let s = self.buf.get(self.pos..end).ok_or(Malformed("message too short"))?;
        self.pos = end;
        Ok(s)
    }

    pub fn u8(&mut self) -> Result<u8, Malformed> {
        Ok(self.take(1)?[0])
    }

    pub fn bool(&mut self) -> Result<bool, Malformed> {
        Ok(self.u8()? != 0)
    }

    pub fn u16(&mut self) -> Result<u16, Malformed> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().expect("2 bytes")))
    }

    pub fn u32(&mut self) -> Result<u32, Malformed> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("4 bytes")))
    }

    pub fn i32(&mut self) -> Result<i32, Malformed> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().expect("4 bytes")))
    }

    pub fn u64(&mut self) -> Result<u64, Malformed> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().expect("8 bytes")))
    }

    pub fn bytes(&mut self) -> Result<&'a [u8], Malformed> {
        let n = self.u32()? as usize;
        self.take(n)
    }

    /// A string: UTF-8, or Latin-1 as old clients send it.
    pub fn str(&mut self) -> Result<String, Malformed> {
        Ok(decode_str(self.bytes()?))
    }

    pub fn ip(&mut self) -> Result<std::net::Ipv4Addr, Malformed> {
        Ok(std::net::Ipv4Addr::from(self.u32()?))
    }

    /// A count of items that each take at least `min_item` bytes: a count
    /// the rest of the message cannot hold is refused before anything is
    /// allocated for it.
    pub fn count(&mut self, min_item: usize) -> Result<usize, Malformed> {
        let n = self.u32()? as usize;
        if n.saturating_mul(min_item.max(1)) > self.remaining() {
            return Err(Malformed("count larger than the message"));
        }
        Ok(n)
    }

    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    pub fn rest(&mut self) -> &'a [u8] {
        let s = &self.buf[self.pos..];
        self.pos = self.buf.len();
        s
    }

    pub fn at_end(&self) -> bool {
        self.remaining() == 0
    }
}

pub fn decode_str(b: &[u8]) -> String {
    match std::str::from_utf8(b) {
        Ok(s) => s.to_owned(),
        Err(_) => b.iter().map(|&c| char::from(c)).collect(),
    }
}

pub fn zlib_compress(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::with_capacity(data.len() / 3), flate2::Compression::default());
    e.write_all(data).expect("writing to memory");
    e.finish().expect("writing to memory")
}

/// Inflates at most `limit` bytes (a hostile peer's zip bomb stops there).
pub fn zlib_decompress(data: &[u8], limit: usize) -> io::Result<Vec<u8>> {
    let mut out = Vec::with_capacity((data.len() * 4).min(limit));
    flate2::read::ZlibDecoder::new(data).take(limit as u64 + 1).read_to_end(&mut out)?;
    if out.len() > limit {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "inflated message too large"));
    }
    Ok(out)
}

/// Frames a message whose code is a `u32` (server and peer messages).
pub fn frame_u32(code: u32, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend_from_slice(&((body.len() + 4) as u32).to_le_bytes());
    out.extend_from_slice(&code.to_le_bytes());
    out.extend_from_slice(body);
    out
}

/// Frames a message whose code is a `u8` (peer init and distributed
/// messages).
pub fn frame_u8(code: u8, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 5);
    out.extend_from_slice(&((body.len() + 1) as u32).to_le_bytes());
    out.push(code);
    out.extend_from_slice(body);
    out
}

/// Reads one length-prefixed frame (code included), refusing frames longer
/// than `limit`.
pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R, limit: usize) -> io::Result<Vec<u8>> {
    let len = r.read_u32_le().await? as usize;
    if len > limit {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("frame of {len} bytes")));
    }
    let mut buf = vec![0; len];
    r.read_exact(&mut buf).await?;
    Ok(buf)
}

pub async fn write_all<W: AsyncWrite + Unpin>(w: &mut W, bytes: &[u8]) -> io::Result<()> {
    w.write_all(bytes).await?;
    w.flush().await
}

/// Splits a frame read by [`read_frame`] into its `u32` code and body.
pub fn split_u32(frame: &[u8]) -> Result<(u32, &[u8]), Malformed> {
    if frame.len() < 4 {
        return Err(Malformed("frame without a code"));
    }
    Ok((u32::from_le_bytes(frame[..4].try_into().expect("4 bytes")), &frame[4..]))
}

/// Splits a frame read by [`read_frame`] into its `u8` code and body.
pub fn split_u8(frame: &[u8]) -> Result<(u8, &[u8]), Malformed> {
    frame.split_first().map(|(c, b)| (*c, b)).ok_or(Malformed("frame without a code"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_values() {
        let mut w = Writer::new();
        w.u8(7).bool(true).u16(513).u32(0xdead_beef).i32(-2).u64(1 << 40).str("Mötley").bytes(&[1, 2]);
        w.ip("1.2.3.4".parse().unwrap());
        let buf = w.into_inner();
        let mut r = Reader::new(&buf);
        assert_eq!(r.u8().unwrap(), 7);
        assert!(r.bool().unwrap());
        assert_eq!(r.u16().unwrap(), 513);
        assert_eq!(r.u32().unwrap(), 0xdead_beef);
        assert_eq!(r.i32().unwrap(), -2);
        assert_eq!(r.u64().unwrap(), 1 << 40);
        assert_eq!(r.str().unwrap(), "Mötley");
        assert_eq!(r.bytes().unwrap(), &[1, 2]);
        assert_eq!(r.ip().unwrap().to_string(), "1.2.3.4");
        assert!(r.at_end());
        assert!(r.u8().is_err());
    }

    #[test]
    fn ip_is_sent_with_the_last_octet_first() {
        let mut w = Writer::new();
        w.ip("10.20.30.40".parse().unwrap());
        assert_eq!(w.into_inner(), vec![40, 30, 20, 10]);
    }

    #[test]
    fn latin1_strings_are_read() {
        let mut w = Writer::new();
        w.bytes(&[0x42, 0xe9, 0x62, 0xe9]);
        let buf = w.into_inner();
        assert_eq!(Reader::new(&buf).str().unwrap(), "Bébé");
    }

    #[test]
    fn huge_counts_are_refused() {
        let buf = 1_000_000u32.to_le_bytes();
        assert!(Reader::new(&buf).count(4).is_err());
    }

    #[test]
    fn zlib_round_trip_and_limit() {
        let data = vec![b'a'; 100_000];
        let z = zlib_compress(&data);
        assert_eq!(zlib_decompress(&z, 100_000).unwrap(), data);
        assert!(zlib_decompress(&z, 99_999).is_err());
    }

    #[test]
    fn login_frame_matches_the_documented_bytes() {
        // The example of the protocol documentation (Server Code 1).
        let mut w = Writer::new();
        w.str("username").str("password").u32(177).str("d51c9a7e9353746a6020f9602d452929").u32(1);
        let frame = frame_u32(1, &w.into_inner());
        assert_eq!(&frame[..8], &[0x48, 0, 0, 0, 1, 0, 0, 0]);
        assert_eq!(frame.len(), 0x48 + 4);
    }
}

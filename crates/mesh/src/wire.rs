//! Messages between two devices on a stream: JSON, each after its length
//! (4 bytes, big-endian), at most [`MAX_FRAME`] long.

use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const MAX_FRAME: usize = 256 * 1024;

fn invalid(e: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
}

pub async fn write_frame<W: AsyncWrite + Unpin, T: Serialize>(w: &mut W, msg: &T) -> std::io::Result<()> {
    let body = serde_json::to_vec(msg).map_err(invalid)?;
    if body.len() > MAX_FRAME {
        return Err(invalid("a message too long"));
    }
    w.write_all(&(body.len() as u32).to_be_bytes()).await?;
    w.write_all(&body).await?;
    w.flush().await
}

pub async fn read_frame<R: AsyncRead + Unpin, T: DeserializeOwned>(r: &mut R) -> std::io::Result<T> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len).await?;
    let n = u32::from_be_bytes(len) as usize;
    if n > MAX_FRAME {
        return Err(invalid("a message too long"));
    }
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf).await?;
    serde_json::from_slice(&buf).map_err(invalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frames_go_through_and_a_huge_one_is_refused() {
        let (mut a, mut b) = tokio::io::duplex(1 << 20);
        write_frame(&mut a, &serde_json::json!({"t": "hi", "n": 1})).await.unwrap();
        let v: serde_json::Value = read_frame(&mut b).await.unwrap();
        assert_eq!(v["n"], 1);
        a.write_all(&(u32::MAX).to_be_bytes()).await.unwrap();
        assert!(read_frame::<_, serde_json::Value>(&mut b).await.is_err());
    }
}

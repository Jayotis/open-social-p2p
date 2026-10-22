//! Frame format shared by all osp protocols: a 4-byte big-endian length
//! followed by that many bytes of JSON.

use anyhow::{Context, Result, ensure};
use serde::{Serialize, de::DeserializeOwned};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Frames larger than this are refused, so a peer can't make us allocate without bound.
pub const MAX_FRAME_LEN: u32 = 16 * 1024 * 1024;

pub async fn write_frame<W, T>(writer: &mut W, msg: &T) -> Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let bytes = serde_json::to_vec(msg)?;
    let len = u32::try_from(bytes.len())
        .ok()
        .filter(|len| *len <= MAX_FRAME_LEN)
        .context("frame too large")?;
    writer.write_all(&len.to_be_bytes()).await?;
    writer.write_all(&bytes).await?;
    writer.flush().await?;
    Ok(())
}

pub async fn read_frame<R, T>(reader: &mut R) -> Result<T>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    read_frame_max(reader, MAX_FRAME_LEN).await
}

/// Like [`read_frame`], but refuses frames longer than `max` bytes. Requests
/// from other parties are small, so reading them with a small limit stops a
/// stranger making us hold a large frame.
pub async fn read_frame_max<R, T>(reader: &mut R, max: u32) -> Result<T>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let len = reader.read_u32().await?;
    ensure!(
        len <= max,
        "frame of {len} bytes exceeds the {max}-byte limit"
    );
    // Grows as bytes arrive, so announcing a large frame and then stalling
    // costs only what was actually sent.
    let mut buf = Vec::with_capacity(len.min(64 * 1024) as usize);
    reader.take(u64::from(len)).read_to_end(&mut buf).await?;
    ensure!(buf.len() == len as usize, "frame ended early");
    serde_json::from_slice(&buf).context("malformed frame")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::social::SocialRequest;

    #[tokio::test]
    async fn frame_round_trip() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        write_frame(
            &mut a,
            &SocialRequest::GetOutbox {
                since: 7,
                limit: 10,
            },
        )
        .await
        .unwrap();
        let got: SocialRequest = read_frame(&mut b).await.unwrap();
        assert!(matches!(
            got,
            SocialRequest::GetOutbox {
                since: 7,
                limit: 10
            }
        ));
    }

    #[tokio::test]
    async fn oversized_frame_is_refused() {
        let (mut a, mut b) = tokio::io::duplex(64);
        a.write_all(&(MAX_FRAME_LEN + 1).to_be_bytes())
            .await
            .unwrap();
        let got: Result<SocialRequest> = read_frame(&mut b).await;
        assert!(got.is_err());
    }

    #[tokio::test]
    async fn frame_over_a_smaller_limit_is_refused_before_its_bytes_arrive() {
        let (mut a, mut b) = tokio::io::duplex(64);
        // Only the length is sent; a reader that waited for the bytes would hang.
        a.write_all(&1025u32.to_be_bytes()).await.unwrap();
        let got: Result<SocialRequest> = read_frame_max(&mut b, 1024).await;
        assert!(got.unwrap_err().to_string().contains("exceeds"));
    }

    #[tokio::test]
    async fn frame_that_ends_early_is_an_error() {
        let (mut a, mut b) = tokio::io::duplex(64);
        a.write_all(&10u32.to_be_bytes()).await.unwrap();
        a.write_all(b"{}").await.unwrap();
        drop(a);
        let got: Result<SocialRequest> = read_frame(&mut b).await;
        assert!(got.unwrap_err().to_string().contains("ended early"));
    }
}

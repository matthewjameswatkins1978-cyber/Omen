use crate::error::LocalIpcError;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const MAX_FRAME_SIZE: usize = 1024 * 1024; // 1 MiB hard ceiling

/// Reads a single length-delimited binary frame from the given reader.
/// Returns `Ok(None)` if EOF is reached cleanly at a frame boundary.
pub async fn read_frame<R>(reader: &mut R) -> Result<Option<Vec<u8>>, LocalIpcError>
where
    R: AsyncRead + Unpin,
{
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(LocalIpcError::Io(e.to_string())),
    }

    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_FRAME_SIZE {
        return Err(LocalIpcError::FrameTooLarge {
            declared_bytes: len,
            max_bytes: MAX_FRAME_SIZE,
        });
    }

    let mut payload = vec![0u8; len];
    if len > 0 {
        reader.read_exact(&mut payload).await.map_err(|e| {
            LocalIpcError::MalformedRequest(format!("Truncated frame payload: {e}"))
        })?;
    }

    Ok(Some(payload))
}

/// Writes a single length-delimited binary frame to the given writer and flushes.
pub async fn write_frame<W>(writer: &mut W, payload: &[u8]) -> Result<(), LocalIpcError>
where
    W: AsyncWrite + Unpin,
{
    if payload.len() > MAX_FRAME_SIZE {
        return Err(LocalIpcError::FrameTooLarge {
            declared_bytes: payload.len(),
            max_bytes: MAX_FRAME_SIZE,
        });
    }

    let len = payload.len() as u32;
    writer
        .write_all(&len.to_be_bytes())
        .await
        .map_err(|e| LocalIpcError::Io(e.to_string()))?;

    if !payload.is_empty() {
        writer
            .write_all(payload)
            .await
            .map_err(|e| LocalIpcError::Io(e.to_string()))?;
    }

    writer
        .flush()
        .await
        .map_err(|e| LocalIpcError::Io(e.to_string()))?;

    Ok(())
}

/// Reads a length-delimited frame and deserializes it from JSON.
pub async fn read_json_frame<T, R>(reader: &mut R) -> Result<Option<T>, LocalIpcError>
where
    T: DeserializeOwned,
    R: AsyncRead + Unpin,
{
    match read_frame(reader).await? {
        Some(bytes) => {
            let msg = serde_json::from_slice::<T>(&bytes).map_err(|e| {
                LocalIpcError::MalformedRequest(format!("Failed to parse JSON: {e}"))
            })?;
            Ok(Some(msg))
        }
        None => Ok(None),
    }
}

/// Serializes the value to JSON and writes it as a length-delimited frame.
pub async fn write_json_frame<T, W>(writer: &mut W, value: &T) -> Result<(), LocalIpcError>
where
    T: Serialize,
    W: AsyncWrite + Unpin,
{
    let bytes = serde_json::to_vec(value)
        .map_err(|e| LocalIpcError::MalformedRequest(format!("Failed to serialize JSON: {e}")))?;
    write_frame(writer, &bytes).await
}

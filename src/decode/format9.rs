//! Format 9: encrypted 16×16 RGB animation.
//!
//! ```text
//! offset  size  description
//! 0       1     format = 9
//! 1       1     declared frame count
//! 2       2     frame duration in ms, big-endian u16
//! 4       ...   AES-128-CBC encrypted RGB frames (768 bytes each)
//! ```

use super::{
    aes_cbc_decrypt, format, DecodedArtwork, FRAME_BYTES, MAX_FRAME_DURATION_MS, SUPPORTED_EDGE,
};
use crate::error::{Error, Result};

/// Header size in bytes.
pub const HEADER_LEN: usize = 4;

pub fn decode(bytes: &[u8]) -> Result<DecodedArtwork> {
    if bytes.len() < HEADER_LEN {
        return Err(Error::InvalidContainer(format!(
            "format 9 header requires {HEADER_LEN} bytes, got {}",
            bytes.len()
        )));
    }
    if bytes[0] != format::RGB_ANIMATION {
        return Err(Error::UnsupportedFormat(bytes[0]));
    }
    let frame_count = bytes[1] as usize;
    if frame_count == 0 {
        return Err(Error::InvalidContainer("frame count is zero".into()));
    }
    let duration = u16::from_be_bytes([bytes[2], bytes[3]]) as u32;
    if duration == 0 || duration > MAX_FRAME_DURATION_MS {
        return Err(Error::InvalidContainer(format!(
            "frame duration {duration} ms out of range 1..={MAX_FRAME_DURATION_MS}"
        )));
    }
    let ciphertext = &bytes[HEADER_LEN..];
    if ciphertext.is_empty() {
        return Err(Error::InvalidContainer("no frame data".into()));
    }
    // Reject absurd claims before touching the ciphertext; bounded by u8 so at
    // most 255 * 768 bytes are ever allocated.
    let expected = frame_count * FRAME_BYTES;
    if ciphertext.len() != expected {
        return Err(Error::InvalidContainer(format!(
            "ciphertext length {} does not match {frame_count} frames × {FRAME_BYTES} bytes = {expected}",
            ciphertext.len()
        )));
    }
    let plaintext = aes_cbc_decrypt(ciphertext)?;
    if plaintext.len() != expected {
        return Err(Error::Decrypt(format!(
            "plaintext length {} != expected {expected}",
            plaintext.len()
        )));
    }
    let frames = plaintext
        .chunks_exact(FRAME_BYTES)
        .map(|c| c.to_vec())
        .collect();
    Ok(DecodedArtwork {
        width: SUPPORTED_EDGE,
        height: SUPPORTED_EDGE,
        frame_duration_ms: duration,
        frames,
    })
}

/// Build a synthetic format 9 binary from RGB frames (for tests and fixtures).
pub fn encode(frames: &[Vec<u8>], frame_duration_ms: u16) -> Result<Vec<u8>> {
    if frames.is_empty() || frames.len() > u8::MAX as usize {
        return Err(Error::InvalidContainer(
            "frame count must be 1..=255".into(),
        ));
    }
    let mut plaintext = Vec::with_capacity(frames.len() * FRAME_BYTES);
    for f in frames {
        if f.len() != FRAME_BYTES {
            return Err(Error::InvalidContainer(format!(
                "frame must be {FRAME_BYTES} bytes"
            )));
        }
        plaintext.extend_from_slice(f);
    }
    let mut out = vec![
        format::RGB_ANIMATION,
        frames.len() as u8,
        (frame_duration_ms >> 8) as u8,
        (frame_duration_ms & 0xff) as u8,
    ];
    out.extend(super::aes_cbc_encrypt(&plaintext)?);
    Ok(out)
}

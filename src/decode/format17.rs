//! Format 17: encrypted LZO1X-compressed static image.
//!
//! ```text
//! offset  size  description
//! 0       1     format = 17
//! 1       1     number of 16-pixel tile rows
//! 2       1     number of 16-pixel tile columns
//! 3       4     LZO block length, big-endian u32
//! 7       ...   AES-128-CBC encrypted data
//! ```

use super::{aes_cbc_decrypt, format, DecodedArtwork, BYTES_PER_PIXEL, SUPPORTED_EDGE};
use crate::error::{Error, Result};

/// Header size in bytes.
pub const HEADER_LEN: usize = 7;
/// Frame duration assigned to the single static frame.
pub const STATIC_FRAME_DURATION_MS: u32 = 1000;

pub fn decode(bytes: &[u8]) -> Result<DecodedArtwork> {
    if bytes.len() < HEADER_LEN {
        return Err(Error::InvalidContainer(format!(
            "format 17 header requires {HEADER_LEN} bytes, got {}",
            bytes.len()
        )));
    }
    if bytes[0] != format::LZO_STATIC {
        return Err(Error::UnsupportedFormat(bytes[0]));
    }
    let rows = bytes[1] as u32;
    let cols = bytes[2] as u32;
    if rows == 0 || cols == 0 {
        return Err(Error::InvalidContainer("zero tile count".into()));
    }
    if rows != 1 || cols != 1 {
        return Err(Error::UnsupportedDimensions {
            width: cols * SUPPORTED_EDGE,
            height: rows * SUPPORTED_EDGE,
        });
    }
    let lzo_len = u32::from_be_bytes([bytes[3], bytes[4], bytes[5], bytes[6]]) as usize;
    let ciphertext = &bytes[HEADER_LEN..];
    if ciphertext.is_empty() {
        return Err(Error::InvalidContainer("no image data".into()));
    }
    if lzo_len == 0 || lzo_len > ciphertext.len() {
        return Err(Error::InvalidContainer(format!(
            "LZO block length {lzo_len} exceeds ciphertext length {}",
            ciphertext.len()
        )));
    }
    let plaintext = aes_cbc_decrypt(ciphertext)?;
    let block = &plaintext[..lzo_len];
    let expected = (SUPPORTED_EDGE * SUPPORTED_EDGE) as usize * BYTES_PER_PIXEL;
    let mut out = vec![0u8; expected];
    let n =
        lzo::decompress_into(block, &mut out).map_err(|e| Error::Decompress(format!("{e:?}")))?;
    if n != expected {
        return Err(Error::Decompress(format!(
            "decompressed {n} bytes, expected {expected}"
        )));
    }
    Ok(DecodedArtwork {
        width: SUPPORTED_EDGE,
        height: SUPPORTED_EDGE,
        frame_duration_ms: STATIC_FRAME_DURATION_MS,
        frames: vec![out],
    })
}

/// Encode a raw byte slice as a literal-only LZO1X block (no back-references).
/// Valid for the reference decoder; used to build synthetic fixtures.
pub fn lzo1x_literal_block(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 8);
    if data.is_empty() {
        out.extend_from_slice(&[0x11, 0x00, 0x00]);
        return out;
    }
    if data.len() <= 238 {
        // Initial literal run encoded as first byte = 17 + len.
        out.push(17 + data.len() as u8);
    } else {
        // Zero-extension literal run: base 18, then 255 per zero byte.
        let mut rem = data.len() - 18;
        out.push(0);
        while rem > 255 {
            out.push(0);
            rem -= 255;
        }
        out.push(rem as u8);
    }
    out.extend_from_slice(data);
    out.extend_from_slice(&[0x11, 0x00, 0x00]);
    out
}

/// Build a synthetic format 17 binary from one 768-byte RGB frame.
pub fn encode(frame: &[u8], rows: u8, cols: u8) -> Result<Vec<u8>> {
    let block = lzo1x_literal_block(frame);
    let mut padded = block.clone();
    while padded.len() % super::AES_BLOCK != 0 {
        padded.push(0);
    }
    let cipher = super::aes_cbc_encrypt(&padded)?;
    let mut out = vec![format::LZO_STATIC, rows, cols];
    out.extend_from_slice(&(block.len() as u32).to_be_bytes());
    out.extend(cipher);
    Ok(out)
}

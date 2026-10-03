//! Divoom 16×16 artwork decoding (formats 9 and 17).

pub mod format17;
pub mod format9;

use aes::cipher::{block_padding::NoPadding, BlockDecryptMut, BlockEncryptMut, KeyIvInit};

use crate::error::{Error, Result};

/// Only supported artwork edge length in version 1.
pub const SUPPORTED_EDGE: u32 = 16;
/// Bytes per RGB888 pixel.
pub const BYTES_PER_PIXEL: usize = 3;
/// Bytes per 16×16 RGB888 frame.
pub const FRAME_BYTES: usize = (SUPPORTED_EDGE * SUPPORTED_EDGE) as usize * BYTES_PER_PIXEL;
/// Maximum accepted frame duration in milliseconds.
pub const MAX_FRAME_DURATION_MS: u32 = 60_000;

/// Shared AES-128-CBC parameters used by Divoom binaries (ASCII).
pub const AES_KEY: &[u8; 16] = b"78hrey23y28ogs89";
/// Shared AES-128-CBC initialization vector (ASCII).
pub const AES_IV: &[u8; 16] = b"1234567890123456";
/// AES block size in bytes.
pub const AES_BLOCK: usize = 16;

/// Container format identifiers (first byte of the binary).
pub mod format {
    pub const RGB_ANIMATION: u8 = 9;
    pub const LZO_STATIC: u8 = 17;
    /// Known but unsupported in version 1.
    pub const KNOWN_UNSUPPORTED: &[u8] = &[18, 26, 31, 41, 42, 43];
}

/// Decoded artwork: RGB888 row-major frames of identical dimensions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedArtwork {
    pub width: u32,
    pub height: u32,
    pub frame_duration_ms: u32,
    pub frames: Vec<Vec<u8>>,
}

impl DecodedArtwork {
    /// Validate the structural invariants listed in the specification.
    pub fn validate(&self) -> Result<()> {
        if self.width == 0 || self.height == 0 {
            return Err(Error::InvalidContainer("zero dimension".into()));
        }
        if self.frames.is_empty() {
            return Err(Error::InvalidContainer("no frames".into()));
        }
        if self.frame_duration_ms == 0 || self.frame_duration_ms > MAX_FRAME_DURATION_MS {
            return Err(Error::InvalidContainer(format!(
                "frame duration {} ms out of range",
                self.frame_duration_ms
            )));
        }
        let expected = self.width as usize * self.height as usize * BYTES_PER_PIXEL;
        if let Some((i, f)) = self
            .frames
            .iter()
            .enumerate()
            .find(|(_, f)| f.len() != expected)
        {
            return Err(Error::InvalidContainer(format!(
                "frame {i} has {} bytes, expected {expected}",
                f.len()
            )));
        }
        Ok(())
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }
}

/// Peek the container format byte without decoding.
pub fn format_of(bytes: &[u8]) -> Result<u8> {
    bytes
        .first()
        .copied()
        .ok_or_else(|| Error::InvalidContainer("empty file".into()))
}

/// Decode a Divoom binary into RGB frames. Dispatches on the format byte.
pub fn decode(bytes: &[u8]) -> Result<DecodedArtwork> {
    let art = match format_of(bytes)? {
        format::RGB_ANIMATION => format9::decode(bytes)?,
        format::LZO_STATIC => format17::decode(bytes)?,
        other => return Err(Error::UnsupportedFormat(other)),
    };
    art.validate()?;
    Ok(art)
}

type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;
type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;

/// Decrypt block-aligned ciphertext with the shared Divoom AES-128-CBC
/// parameters and no padding.
pub fn aes_cbc_decrypt(ciphertext: &[u8]) -> Result<Vec<u8>> {
    if ciphertext.is_empty() {
        return Err(Error::InvalidContainer("empty ciphertext".into()));
    }
    if ciphertext.len() % AES_BLOCK != 0 {
        return Err(Error::InvalidContainer(format!(
            "ciphertext length {} is not a multiple of {AES_BLOCK}",
            ciphertext.len()
        )));
    }
    let mut buf = ciphertext.to_vec();
    let dec = Aes128CbcDec::new_from_slices(AES_KEY, AES_IV)
        .map_err(|e| Error::Decrypt(format!("cipher init: {e}")))?;
    let n = dec
        .decrypt_padded_mut::<NoPadding>(&mut buf)
        .map_err(|e| Error::Decrypt(format!("{e}")))?
        .len();
    buf.truncate(n);
    Ok(buf)
}

/// Encrypt block-aligned plaintext with the shared parameters. Used to build
/// synthetic fixtures in tests; the tool itself never encrypts remote data.
pub fn aes_cbc_encrypt(plaintext: &[u8]) -> Result<Vec<u8>> {
    if plaintext.len() % AES_BLOCK != 0 {
        return Err(Error::InvalidContainer(format!(
            "plaintext length {} is not a multiple of {AES_BLOCK}",
            plaintext.len()
        )));
    }
    let mut buf = plaintext.to_vec();
    let enc = Aes128CbcEnc::new_from_slices(AES_KEY, AES_IV)
        .map_err(|e| Error::Decrypt(format!("cipher init: {e}")))?;
    let len = plaintext.len();
    let n = enc
        .encrypt_padded_mut::<NoPadding>(&mut buf, len)
        .map_err(|e| Error::Decrypt(format!("{e}")))?
        .len();
    buf.truncate(n);
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aes_round_trip() {
        let pt: Vec<u8> = (0..48u8).collect();
        let ct = aes_cbc_encrypt(&pt).unwrap();
        assert_eq!(ct.len(), 48);
        assert_ne!(ct, pt);
        assert_eq!(aes_cbc_decrypt(&ct).unwrap(), pt);
    }

    #[test]
    fn empty_and_unknown_formats() {
        assert!(matches!(decode(&[]), Err(Error::InvalidContainer(_))));
        assert!(matches!(
            decode(&[42, 0, 0]),
            Err(Error::UnsupportedFormat(42))
        ));
        assert!(matches!(
            decode(&[0xff]),
            Err(Error::UnsupportedFormat(0xff))
        ));
    }
}

//! Lossless animated WebP export via `webp-animation` (libwebp).

use std::path::Path;

use webp_animation::{
    AnimParams, ColorMode, Decoder, Encoder, EncoderOptions, EncodingConfig, EncodingType,
};

use crate::decode::DecodedArtwork;
use crate::error::{Error, Result};
use crate::manifest::write_atomic;

/// Summary of a WebP file as read back from disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebpInfo {
    pub width: u32,
    pub height: u32,
    pub frame_count: usize,
    /// Timestamp of each frame in milliseconds, in order.
    pub timestamps_ms: Vec<i32>,
}

/// Encode artwork as a lossless animated WebP with infinite looping and
/// millisecond timestamps `n * frame_duration_ms`.
pub fn encode_webp(art: &DecodedArtwork) -> Result<Vec<u8>> {
    art.validate()?;
    let duration = i32::try_from(art.frame_duration_ms)
        .map_err(|_| Error::WebpEncode("frame duration overflow".into()))?;
    let total = i32::try_from(art.frames.len() as u64 * art.frame_duration_ms as u64)
        .map_err(|_| Error::WebpEncode("animation duration overflow".into()))?;

    let options = EncoderOptions {
        anim_params: AnimParams { loop_count: 0 },
        minimize_size: false,
        kmin: 0,
        kmax: 0,
        allow_mixed: false,
        verbose: false,
        color_mode: ColorMode::Rgba,
        encoding_config: Some(EncodingConfig {
            encoding_type: EncodingType::Lossless,
            quality: 100.0,
            method: 6,
        }),
    };
    let mut encoder = Encoder::new_with_options((art.width, art.height), options)
        .map_err(|e| Error::WebpEncode(format!("{e:?}")))?;

    let mut rgba = Vec::with_capacity(art.width as usize * art.height as usize * 4);
    for (i, frame) in art.frames.iter().enumerate() {
        rgba.clear();
        for px in frame.chunks_exact(3) {
            rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
        }
        let ts = i as i32 * duration;
        encoder
            .add_frame(&rgba, ts)
            .map_err(|e| Error::WebpEncode(format!("frame {i}: {e:?}")))?;
    }
    let data = encoder
        .finalize(total)
        .map_err(|e| Error::WebpEncode(format!("finalize: {e:?}")))?;
    Ok(data.to_vec())
}

/// Decode a WebP file into RGBA frames and timestamps.
pub fn read_webp(bytes: &[u8]) -> Result<(WebpInfo, Vec<Vec<u8>>)> {
    let decoder = Decoder::new(bytes).map_err(|e| Error::WebpEncode(format!("decode: {e:?}")))?;
    let (width, height) = decoder.dimensions();
    let mut frames = Vec::new();
    let mut timestamps_ms = Vec::new();
    for frame in decoder {
        timestamps_ms.push(frame.timestamp());
        frames.push(frame.data().to_vec());
    }
    if frames.is_empty() {
        return Err(Error::WebpEncode("WebP contains no frames".into()));
    }
    Ok((
        WebpInfo {
            width,
            height,
            frame_count: frames.len(),
            timestamps_ms,
        },
        frames,
    ))
}

/// Validate a WebP byte buffer and return its summary.
pub fn inspect_webp(bytes: &[u8]) -> Result<WebpInfo> {
    read_webp(bytes).map(|(info, _)| info)
}

/// Encode and write a WebP file via a temporary file and atomic rename.
/// Returns the encoded bytes.
pub fn write_webp_atomic(path: &Path, art: &DecodedArtwork) -> Result<Vec<u8>> {
    let bytes = encode_webp(art)?;
    // Re-open the encoded data to validate before installing it.
    let info = inspect_webp(&bytes)?;
    validate_against(&info, art)?;
    write_atomic(path, &bytes)?;
    Ok(bytes)
}

/// Check a reopened WebP against its source artwork.
///
/// libwebp's animation encoder merges *identical consecutive* frames into one
/// frame with a longer duration, so the frame count may be lower than the
/// source count. Pixels and the total timeline are still exact: the last
/// timestamp must equal `frame_count × frame_duration_ms`.
pub fn validate_against(info: &WebpInfo, art: &DecodedArtwork) -> Result<()> {
    let expected_total = art.frames.len() as i64 * art.frame_duration_ms as i64;
    let last = info.timestamps_ms.last().copied().unwrap_or(0) as i64;
    // A single-frame output is a still image: libwebp reports timestamp 0.
    let timeline_ok = last == expected_total || (info.frame_count == 1 && last == 0);
    if info.width != art.width
        || info.height != art.height
        || info.frame_count == 0
        || info.frame_count > art.frames.len()
        || !timeline_ok
    {
        return Err(Error::WebpEncode(format!(
            "validation mismatch: got {}x{} with {} frames ending at {} ms, expected {}x{} with at most {} frames ending at {} ms",
            info.width,
            info.height,
            info.frame_count,
            last,
            art.width,
            art.height,
            art.frames.len(),
            expected_total
        )));
    }
    Ok(())
}

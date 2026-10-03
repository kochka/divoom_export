mod common;

use common::{format17_fixture, format9_fixture, frame};
use divoom_export::decode::{self, DecodedArtwork};
use divoom_export::export::{encode_webp, inspect_webp, read_webp, write_webp_atomic};

fn rgba_of(rgb: &[u8]) -> Vec<u8> {
    rgb.chunks_exact(3)
        .flat_map(|p| [p[0], p[1], p[2], 255])
        .collect()
}

#[test]
fn single_frame_exact_pixels() {
    let art = decode::decode(&format9_fixture(1, 100, 4)).unwrap();
    let bytes = encode_webp(&art).unwrap();
    assert_eq!(&bytes[..4], b"RIFF");
    let (info, frames) = read_webp(&bytes).unwrap();
    assert_eq!((info.width, info.height), (16, 16));
    assert_eq!(info.frame_count, 1);
    assert_eq!(frames[0], rgba_of(&frame(4)));
}

#[test]
fn multi_frame_exact_pixels_and_timestamps() {
    let art = decode::decode(&format9_fixture(6, 120, 40)).unwrap();
    let bytes = encode_webp(&art).unwrap();
    let (info, frames) = read_webp(&bytes).unwrap();
    assert_eq!(info.frame_count, 6);
    for (i, f) in frames.iter().enumerate() {
        assert_eq!(f, &rgba_of(&frame(40 + i as u8)), "frame {i}");
    }
    // libwebp reports the *end* timestamp of each frame: (n+1) * duration.
    let expected: Vec<i32> = (1..=6).map(|n| n * 120).collect();
    assert_eq!(info.timestamps_ms, expected);
}

#[test]
fn static_format17_exports_one_frame() {
    let art = decode::decode(&format17_fixture(8)).unwrap();
    let bytes = encode_webp(&art).unwrap();
    let (info, frames) = read_webp(&bytes).unwrap();
    assert_eq!(info.frame_count, 1);
    assert_eq!(frames[0], rgba_of(&frame(8)));
}

#[test]
fn no_scaling_or_interpolation() {
    // A frame with hard single-pixel edges must survive byte-for-byte.
    let mut rgb = vec![0u8; 16 * 16 * 3];
    for y in 0..16 {
        for x in 0..16 {
            let i = (y * 16 + x) * 3;
            let on = (x + y) % 2 == 0;
            rgb[i] = if on { 255 } else { 0 };
            rgb[i + 1] = if on { 0 } else { 255 };
            rgb[i + 2] = (x * 16) as u8;
        }
    }
    let art = DecodedArtwork {
        width: 16,
        height: 16,
        frame_duration_ms: 1,
        frames: vec![rgb.clone()],
    };
    let bytes = encode_webp(&art).unwrap();
    let (info, frames) = read_webp(&bytes).unwrap();
    assert_eq!((info.width, info.height), (16, 16));
    assert_eq!(frames[0], rgba_of(&rgb));
}

#[test]
fn atomic_write_produces_valid_file_without_temp_leftovers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("artwork.webp");
    let art = decode::decode(&format9_fixture(2, 50, 1)).unwrap();
    write_webp_atomic(&path, &art).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(inspect_webp(&bytes).unwrap().frame_count, 2);
    let entries: Vec<_> = std::fs::read_dir(path.parent().unwrap()).unwrap().collect();
    assert_eq!(entries.len(), 1);
}

#[test]
fn rejects_invalid_artwork() {
    let art = DecodedArtwork {
        width: 16,
        height: 16,
        frame_duration_ms: 100,
        frames: vec![vec![0u8; 10]],
    };
    assert!(encode_webp(&art).is_err());
    assert!(inspect_webp(b"not a webp").is_err());
}

#[test]
fn identical_consecutive_frames_are_merged_but_timeline_is_exact() {
    // libwebp collapses identical consecutive frames; the total duration and
    // the pixels of every remaining frame must still match the source.
    let a = frame(1);
    let b = frame(2);
    let art = DecodedArtwork {
        width: 16,
        height: 16,
        frame_duration_ms: 100,
        frames: vec![a.clone(), a.clone(), a.clone(), b.clone(), b.clone()],
    };
    let bytes = encode_webp(&art).unwrap();
    let (info, frames) = read_webp(&bytes).unwrap();
    divoom_export::export::validate_against(&info, &art).unwrap();
    assert!(info.frame_count <= 5 && info.frame_count >= 2);
    assert_eq!(*info.timestamps_ms.last().unwrap(), 500);
    assert_eq!(frames[0], rgba_of(&a));
    assert_eq!(frames[frames.len() - 1], rgba_of(&b));

    // A mismatch in the timeline is still rejected.
    let other = DecodedArtwork {
        frame_duration_ms: 50,
        ..art.clone()
    };
    assert!(divoom_export::export::validate_against(&info, &other).is_err());
}

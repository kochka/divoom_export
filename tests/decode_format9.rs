mod common;

use common::{format9_fixture, frame};
use divoom_export::decode::{self, aes_cbc_encrypt, format9, FRAME_BYTES};
use divoom_export::Error;

#[test]
fn single_frame_round_trip() {
    let bytes = format9_fixture(1, 100, 1);
    let art = decode::decode(&bytes).unwrap();
    assert_eq!((art.width, art.height), (16, 16));
    assert_eq!(art.frame_duration_ms, 100);
    assert_eq!(art.frames.len(), 1);
    assert_eq!(art.frames[0], frame(1));
}

#[test]
fn multi_frame_round_trip() {
    let bytes = format9_fixture(5, 250, 9);
    let art = decode::decode(&bytes).unwrap();
    assert_eq!(art.frames.len(), 5);
    for (i, f) in art.frames.iter().enumerate() {
        assert_eq!(f, &frame(9 + i as u8), "frame {i}");
    }
}

#[test]
fn duration_is_big_endian() {
    let bytes = format9_fixture(1, 0x0102, 0);
    assert_eq!(&bytes[2..4], &[0x01, 0x02]);
    assert_eq!(decode::decode(&bytes).unwrap().frame_duration_ms, 258);
}

#[test]
fn rejects_zero_and_excessive_duration() {
    let mut bytes = format9_fixture(1, 100, 0);
    bytes[2] = 0;
    bytes[3] = 0;
    assert!(matches!(
        decode::decode(&bytes),
        Err(Error::InvalidContainer(_))
    ));
    let mut bytes = format9_fixture(1, 100, 0);
    let big = 60_001u16.to_be_bytes();
    bytes[2] = big[0];
    bytes[3] = big[1];
    assert!(matches!(
        decode::decode(&bytes),
        Err(Error::InvalidContainer(_))
    ));
}

#[test]
fn rejects_unaligned_ciphertext() {
    let mut bytes = format9_fixture(1, 100, 0);
    bytes.push(0xAA);
    assert!(matches!(
        decode::decode(&bytes),
        Err(Error::InvalidContainer(_))
    ));
}

#[test]
fn rejects_inconsistent_frame_count() {
    let mut bytes = format9_fixture(2, 100, 0);
    bytes[1] = 3;
    assert!(matches!(
        decode::decode(&bytes),
        Err(Error::InvalidContainer(_))
    ));
    bytes[1] = 0;
    assert!(matches!(
        decode::decode(&bytes),
        Err(Error::InvalidContainer(_))
    ));
}

#[test]
fn rejects_truncated_and_header_only_inputs() {
    let bytes = format9_fixture(2, 100, 0);
    assert!(matches!(
        decode::decode(&bytes[..3]),
        Err(Error::InvalidContainer(_))
    ));
    assert!(matches!(
        decode::decode(&bytes[..4]),
        Err(Error::InvalidContainer(_))
    ));
    assert!(matches!(
        decode::decode(&bytes[..4 + FRAME_BYTES]),
        Err(Error::InvalidContainer(_))
    ));
}

#[test]
fn excessive_allocation_claims_are_rejected_before_decrypting() {
    // Declares 255 frames but carries a single block of ciphertext.
    let mut bytes = vec![9u8, 255, 0, 100];
    bytes.extend(aes_cbc_encrypt(&[0u8; 16]).unwrap());
    assert!(matches!(
        decode::decode(&bytes),
        Err(Error::InvalidContainer(_))
    ));
}

#[test]
fn wrong_format_byte_is_reported() {
    let mut bytes = format9_fixture(1, 100, 0);
    bytes[0] = 18;
    assert!(matches!(
        decode::decode(&bytes),
        Err(Error::UnsupportedFormat(18))
    ));
    assert!(matches!(
        format9::decode(&bytes),
        Err(Error::UnsupportedFormat(18))
    ));
}

mod common;

use common::{format17_fixture, frame};
use divoom_export::decode::{self, aes_cbc_encrypt, format17};
use divoom_export::Error;

#[test]
fn static_lzo_round_trip() {
    let bytes = format17_fixture(3);
    let art = decode::decode(&bytes).unwrap();
    assert_eq!((art.width, art.height), (16, 16));
    assert_eq!(art.frames.len(), 1);
    assert_eq!(art.frames[0], frame(3));
    assert!(art.frame_duration_ms > 0);
}

#[test]
fn literal_block_encoder_handles_short_runs() {
    let data: Vec<u8> = (0..100u8).collect();
    let block = format17::lzo1x_literal_block(&data);
    let out = lzo::decompress(&block, 100).unwrap();
    assert_eq!(out, data);
}

#[test]
fn rejects_non_16x16_dimensions() {
    let bytes = format17::encode(&frame(0), 2, 1).unwrap();
    assert!(matches!(
        decode::decode(&bytes),
        Err(Error::UnsupportedDimensions {
            width: 16,
            height: 32
        })
    ));
    let bytes = format17::encode(&frame(0), 1, 4).unwrap();
    assert!(matches!(
        decode::decode(&bytes),
        Err(Error::UnsupportedDimensions {
            width: 64,
            height: 16
        })
    ));
}

#[test]
fn rejects_zero_tiles_and_truncation() {
    let bytes = format17::encode(&frame(0), 0, 1).unwrap();
    assert!(matches!(
        decode::decode(&bytes),
        Err(Error::InvalidContainer(_))
    ));
    let bytes = format17_fixture(0);
    assert!(matches!(
        decode::decode(&bytes[..6]),
        Err(Error::InvalidContainer(_))
    ));
    assert!(matches!(
        decode::decode(&bytes[..7]),
        Err(Error::InvalidContainer(_))
    ));
    assert!(matches!(
        decode::decode(&bytes[..23]),
        Err(Error::InvalidContainer(_))
    ));
}

#[test]
fn rejects_length_claims_beyond_ciphertext() {
    let mut bytes = format17_fixture(0);
    bytes[3..7].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(matches!(
        decode::decode(&bytes),
        Err(Error::InvalidContainer(_))
    ));
}

#[test]
fn rejects_corrupt_lzo_stream() {
    // A valid container whose LZO payload is garbage.
    let junk = vec![0xFFu8; 32];
    let cipher = aes_cbc_encrypt(&junk).unwrap();
    let mut bytes = vec![17u8, 1, 1];
    bytes.extend_from_slice(&(junk.len() as u32).to_be_bytes());
    bytes.extend(cipher);
    assert!(matches!(decode::decode(&bytes), Err(Error::Decompress(_))));
}

#[test]
fn rejects_short_decompressed_output() {
    let short: Vec<u8> = (0..64u8).collect();
    let block = format17::lzo1x_literal_block(&short);
    let mut padded = block.clone();
    while padded.len() % 16 != 0 {
        padded.push(0);
    }
    let mut bytes = vec![17u8, 1, 1];
    bytes.extend_from_slice(&(block.len() as u32).to_be_bytes());
    bytes.extend(aes_cbc_encrypt(&padded).unwrap());
    assert!(matches!(decode::decode(&bytes), Err(Error::Decompress(_))));
}

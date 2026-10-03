//! Export of decoded artwork to standard image containers.

pub mod webp;

pub use webp::{
    encode_webp, inspect_webp, read_webp, validate_against, write_webp_atomic, WebpInfo,
};

//! Block decoding, driven straight rather than through a file.
//!
//! Reaching `codec::decode` through a whole XISF file means the fuzzer has to
//! keep a signature, a length field and well-formed XML intact before it can
//! vary a single compression parameter, which it will almost never manage. So
//! the parameters are taken from the input directly and the rest of the bytes
//! are the block.
//!
//! The declared size is deliberately held to 32 bits. The interesting region
//! is where a declaration is *plausible* -- large enough to matter, small
//! enough to pass the expansion-ratio check -- and a size drawn from a full 64
//! bits is astronomically large essentially always, which only ever exercises
//! the one branch that rejects it.
#![no_main]

use libfuzzer_sys::fuzz_target;
use xisf_core::block::{Codec, Compression};

fuzz_target!(|data: &[u8]| {
    if data.len() < 8 {
        return;
    }
    let (control, stored) = data.split_at(8);

    let codec = match control[0] % 5 {
        0 => Codec::Zlib,
        1 => Codec::Lz4,
        2 => Codec::Lz4Hc,
        3 => Codec::Zstd,
        _ => Codec::Other("nosuch".into()),
    };

    let uncompressed_size =
        u32::from_le_bytes([control[1], control[2], control[3], control[4]]) as u64;

    // 0 means unshuffled; the parser rejects an item size of 1, so anything
    // else is offset past it to keep the shuffled paths reachable.
    let shuffle_item_size = match control[5] {
        0 => None,
        n => Some(u64::from(n) + 1),
    };

    // A subblock split, when the input asks for one: `n` pieces carved out of
    // the bytes present. The totals are what `decode` checks, so getting them
    // wrong is as interesting as getting them right.
    let subblocks = match control[6] {
        0 => Vec::new(),
        n => {
            let pieces = usize::from(n).min(16);
            let each = stored.len() / pieces.max(1);
            let mut out: Vec<(u64, u64)> = (0..pieces)
                .map(|_| (each as u64, u64::from(control[7]).saturating_mul(each as u64)))
                .collect();
            // Make the compressed lengths account for every byte present, so
            // the split is legal and the decoders are actually reached.
            if let Some(last) = out.last_mut() {
                last.0 += (stored.len() - each * pieces) as u64;
            }
            out
        }
    };

    let compression =
        Compression { codec, uncompressed_size, shuffle_item_size, subblocks };
    let _ = xisf_core::codec::decode(stored, &compression);
});

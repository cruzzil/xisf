//! A whole monolithic file, from the signature to the pixels.
//!
//! The broadest target: it is `XisfFile::open` on a hostile file, which is the
//! operation a caller actually performs and so the one whose failures are
//! reachable in the wild. Blocks are decoded rather than merely addressed,
//! because that is where the codecs, the checksums and the size arithmetic
//! meet -- and where a decompression bomb would show up as the fuzzer's own
//! RSS limit being hit rather than as a crash.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(reader) = xisf_core::Reader::from_bytes(data.to_vec()) else { return };

    for element in reader.header().root.descendants() {
        // Both halves: the stored bytes, and the decoded ones. They fail
        // differently -- a checksum mismatch is not a bad codec stream -- and
        // only the second reaches the decompressors.
        let _ = reader.verify(&element.data);
        let _ = reader.stored_block(&element.data);
        let _ = reader.block(&element.data);
    }
});

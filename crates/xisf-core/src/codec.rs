//! Decoding a stored data block back into its bytes.
//!
//! A block may be compressed, and may additionally have been *byte shuffled*
//! first. Shuffling rearranges an array of N-byte items so that all the first
//! bytes come first, then all the second bytes, and so on. Adjacent bytes then
//! tend to be similar -- the high bytes of a float array barely change -- which
//! gives the general-purpose codecs far more to work with. It is a
//! rearrangement, not a compression: on its own it changes nothing about the
//! size.
//!
//! The order matters and is fixed by the spec: an encoder shuffles and *then*
//! compresses, so a decoder decompresses and *then* unshuffles.

use crate::block::{Codec, Compression};
use crate::err;
use crate::error::Result;

/// The largest expansion ratio a codec can plausibly achieve.
///
/// DEFLATE's theoretical best is about 1032:1 and LZ4's is 255:1, so this is
/// generous for both. It exists because `uncompressed_size` is a number in a
/// file rather than a fact: without a bound, a header saying
/// `compression="zlib:4398046511104"` over a hundred compressed bytes asks
/// the reader to reserve four terabytes, and a decoder that obliges is a
/// denial-of-service primitive that costs an attacker a hundred bytes.
const MAX_EXPANSION_RATIO: u64 = 2048;

/// The most that is reserved up front, whatever the header claims.
///
/// Beyond this the buffer grows as the data arrives, so an implausible
/// declaration that slips past the ratio check still cannot commit the
/// allocation before a single byte has been decompressed.
// Only the codecs that decode into a growable buffer use this, so with none
// of them compiled in it is unread -- correctly.
const MAX_PREALLOCATION: usize = 64 << 20;

/// Decompress and unshuffle a stored block.
pub fn decode(stored: &[u8], compression: &Compression) -> Result<Vec<u8>> {
    let size = usize::try_from(compression.uncompressed_size)
        .map_err(|_| err!(Unsupported, "the block is too large for this platform"))?;

    // Checked before anything is allocated, and against the bytes actually
    // present rather than against a constant, so a small hostile block is
    // refused while a large legitimate one is not.
    let ceiling = (stored.len() as u64).saturating_mul(MAX_EXPANSION_RATIO).max(1024);
    if compression.uncompressed_size > ceiling {
        return Err(err!(
            Compression,
            "a {}-byte block claims to decompress to {} bytes, beyond any ratio {} can achieve",
            stored.len(),
            compression.uncompressed_size,
            compression.codec.name()
        ));
    }

    let plain = if compression.subblocks.is_empty() {
        decompress(stored, &compression.codec, size)?
    } else {
        decompress_subblocks(stored, compression, size)?
    };
    if plain.len() != size {
        return Err(err!(
            Compression,
            "the block declares {size} bytes uncompressed but produced {}",
            plain.len()
        ));
    }

    match compression.shuffle_item_size {
        None => Ok(plain),
        Some(item) => {
            let item = usize::try_from(item)
                .map_err(|_| err!(BadAttribute, "the shuffle item size is unusable"))?;
            Ok(unshuffle(&plain, item))
        }
    }
}

/// Decompress a block stored as several independent streams end to end.
///
/// Codecs have input limits -- zlib cannot take more than 4GiB at once -- and
/// splitting a block also lets an encoder compress the pieces in parallel, so
/// a large image from a parallel encoder arrives this way. Each subblock is
/// its own complete stream: feeding the whole buffer to one codec call
/// recovers only the first, which is why this is not something a decoder can
/// quietly ignore.
fn decompress_subblocks(stored: &[u8], compression: &Compression, size: usize) -> Result<Vec<u8>> {
    // The declared pieces must account for exactly the bytes present and
    // exactly the bytes claimed, checked before any of it is decoded.
    let mut compressed_total: u64 = 0;
    let mut uncompressed_total: u64 = 0;
    for (compressed, uncompressed) in &compression.subblocks {
        compressed_total = compressed_total
            .checked_add(*compressed)
            .ok_or_else(|| err!(Compression, "the subblock lengths overflow"))?;
        uncompressed_total = uncompressed_total
            .checked_add(*uncompressed)
            .ok_or_else(|| err!(Compression, "the subblock lengths overflow"))?;
    }
    if compressed_total != stored.len() as u64 {
        return Err(err!(
            Compression,
            "the subblocks account for {compressed_total} bytes but the block holds {}",
            stored.len()
        ));
    }
    if uncompressed_total != compression.uncompressed_size {
        return Err(err!(
            Compression,
            "the subblocks decompress to {uncompressed_total} bytes but the block declares {}",
            compression.uncompressed_size
        ));
    }

    let mut out = Vec::with_capacity(size.min(MAX_PREALLOCATION));
    let mut rest = stored;
    for (n, (compressed, uncompressed)) in compression.subblocks.iter().enumerate() {
        let compressed = usize::try_from(*compressed)
            .map_err(|_| err!(Unsupported, "subblock {n} is too large for this platform"))?;
        let uncompressed = usize::try_from(*uncompressed)
            .map_err(|_| err!(Unsupported, "subblock {n} is too large for this platform"))?;

        let (chunk, remainder) = rest.split_at(compressed);
        rest = remainder;

        let plain = decompress(chunk, &compression.codec, uncompressed)?;
        if plain.len() != uncompressed {
            return Err(err!(
                Compression,
                "subblock {n} declares {uncompressed} bytes but produced {}",
                plain.len()
            ));
        }
        out.extend_from_slice(&plain);
    }
    Ok(out)
}

// With every codec feature off, each arm below is compiled out and only the
// catch-all remains, leaving these parameters unread. That is the correct
// behaviour for such a build, not an oversight.
#[cfg_attr(not(any(feature = "zlib", feature = "lz4", feature = "zstd")), allow(unused_variables))]
fn decompress(stored: &[u8], codec: &Codec, size: usize) -> Result<Vec<u8>> {
    match codec {
        #[cfg(feature = "zlib")]
        Codec::Zlib => {
            use std::io::Read;
            let mut out = Vec::with_capacity(size.min(MAX_PREALLOCATION));
            flate2::read::ZlibDecoder::new(stored)
                .read_to_end(&mut out)
                .map_err(|e| err!(Compression, "zlib: {e}"))?;
            Ok(out)
        }
        // LZ4HC is an encoder setting, not a different stream format, so both
        // decode with the plain LZ4 block decoder.
        #[cfg(feature = "lz4")]
        Codec::Lz4 | Codec::Lz4Hc => {
            lz4_flex::block::decompress(stored, size).map_err(|e| err!(Compression, "lz4: {e}"))
        }
        #[cfg(feature = "zstd")]
        Codec::Zstd => {
            use std::io::Read;
            let mut out = Vec::with_capacity(size.min(MAX_PREALLOCATION));
            ruzstd::decoding::StreamingDecoder::new(stored)
                .map_err(|e| err!(Compression, "zstd: {e}"))?
                .read_to_end(&mut out)
                .map_err(|e| err!(Compression, "zstd: {e}"))?;
            Ok(out)
        }
        #[allow(unreachable_patterns)]
        other => Err(err!(Unsupported, "cannot decode {} blocks", other.name())),
    }
}

/// Reverse byte shuffling over items of `item` bytes.
///
/// The shuffled form stores byte 0 of every item, then byte 1 of every item,
/// and so on. Any trailing bytes that do not fill a whole item are stored
/// unshuffled at the end, which is what the spec's reference formulation does
/// and what keeps a length that is not a multiple of the item size legal.
pub fn unshuffle(shuffled: &[u8], item: usize) -> Vec<u8> {
    if item < 2 || shuffled.len() < item {
        return shuffled.to_vec();
    }
    let count = shuffled.len() / item;
    let mut out = vec![0u8; shuffled.len()];

    for byte in 0..item {
        let plane = &shuffled[byte * count..(byte + 1) * count];
        for (index, value) in plane.iter().enumerate() {
            out[index * item + byte] = *value;
        }
    }
    // The remainder past the last whole item is carried over as-is.
    let shuffled_len = count * item;
    out[shuffled_len..].copy_from_slice(&shuffled[shuffled_len..]);
    out
}

/// Apply byte shuffling. The inverse of [`unshuffle`].
pub fn shuffle(plain: &[u8], item: usize) -> Vec<u8> {
    if item < 2 || plain.len() < item {
        return plain.to_vec();
    }
    let count = plain.len() / item;
    let mut out = vec![0u8; plain.len()];

    for byte in 0..item {
        for index in 0..count {
            out[byte * count + index] = plain[index * item + byte];
        }
    }
    let shuffled_len = count * item;
    out[shuffled_len..].copy_from_slice(&plain[shuffled_len..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shuffling_round_trips() {
        for item in 2..=8usize {
            for len in 0..40usize {
                let data: Vec<u8> = (0..len).map(|i| (i * 7 + 1) as u8).collect();
                let back = unshuffle(&shuffle(&data, item), item);
                assert_eq!(back, data, "item {item}, len {len}");
            }
        }
    }

    #[test]
    fn shuffling_groups_bytes_by_position() {
        // Two 4-byte items: shuffling puts both first bytes together.
        let data = [1u8, 2, 3, 4, 11, 12, 13, 14];
        assert_eq!(shuffle(&data, 4), [1, 11, 2, 12, 3, 13, 4, 14]);
    }

    #[test]
    fn a_trailing_partial_item_survives() {
        // Nine bytes at four per item: two whole items and one byte over.
        let data: Vec<u8> = (1..=9).collect();
        assert_eq!(unshuffle(&shuffle(&data, 4), 4), data);
        assert_eq!(*shuffle(&data, 4).last().unwrap(), 9);
    }

    /// `uncompressed_size` is a number in a file, not a fact. A hundred-byte
    /// block claiming to expand to terabytes must be refused before anything
    /// is reserved, or opening a file becomes a denial of service that costs
    /// the attacker a hundred bytes.
    #[test]
    fn an_implausible_expansion_ratio_is_refused_before_allocating() {
        let stored = vec![0u8; 100];
        for declared in [1u64 << 42, u64::MAX / 2, 100 * MAX_EXPANSION_RATIO + 1] {
            let compression = Compression {
                codec: Codec::Zlib,
                uncompressed_size: declared,
                shuffle_item_size: None,
                subblocks: Vec::new(),
            };
            let err = decode(&stored, &compression).unwrap_err();
            assert_eq!(err.kind(), crate::ErrorKind::Compression, "{declared} was not refused");
            assert!(err.message().contains("beyond any ratio"), "{}", err.message());
        }
    }

    /// The bound must not reject legitimate files. A small block that really
    /// does expand a long way is normal -- a run of zeroes, say.
    #[cfg(feature = "zlib")]
    #[test]
    fn a_genuinely_compressible_block_still_decodes() {
        use std::io::Write;

        let plain = vec![0u8; 500_000];
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&plain).unwrap();
        let stored = encoder.finish().unwrap();

        // Well over a hundred to one, and it must still be accepted.
        assert!(plain.len() / stored.len() > 100, "this test needs a high ratio to be meaningful");
        let compression = Compression {
            codec: Codec::Zlib,
            uncompressed_size: plain.len() as u64,
            shuffle_item_size: None,
            subblocks: Vec::new(),
        };
        assert_eq!(decode(&stored, &compression).unwrap(), plain);
    }

    #[test]
    fn an_item_size_of_one_changes_nothing() {
        let data = [3u8, 1, 4, 1, 5];
        assert_eq!(shuffle(&data, 1), data);
        assert_eq!(unshuffle(&data, 1), data);
    }
}

//! Distributed XISF: a header file plus separate data blocks files.
//!
//! A monolithic `.xisf` carries its header and its blocks in one file. A
//! *distributed* unit splits them:
//!
//! - an **XISF header file** (`.xish`) holding only the XML header, with no
//!   signature or preamble -- the header is the whole file;
//! - zero or more **XISF data blocks files** (`.xisb`), which the header
//!   addresses with `location="path:..."` or `location="url:..."`.
//!
//! A blocks file is not a flat run of bytes. It is:
//!
//! ```text
//! "XISB0100"          8 bytes, signature
//! reserved            8 bytes, zero
//! block index         a singly linked list of nodes
//! [data blocks]       at the positions the index gives
//! [unused space]      blocks may sit anywhere, so there may be gaps
//! ```
//!
//! The index being a *linked list* rather than a table is the part worth
//! knowing: nodes can be appended anywhere in the file, so a writer can add
//! blocks to an existing file without rewriting it. It also means a reader
//! must not trust it blindly -- a cycle in the `next` pointers would loop
//! forever, and a corrupt file is exactly where that shows up, so the walk
//! below is bounded and refuses to revisit a node.

use crate::err;
use crate::error::Result;

/// The eight bytes a data blocks file starts with. Note `XISB`, not `XISF`.
pub const BLOCKS_SIGNATURE: &[u8; 8] = b"XISB0100";

/// Signature and reserved field, before the first index node.
pub const BLOCKS_PREAMBLE_LEN: usize = 16;

/// A block index element: 40 bytes describing one block.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct IndexElement {
    /// Unique within the file, and how the header names this block.
    pub id: u64,
    /// Byte position from the start of the file. Zero means a *free* element:
    /// a placeholder for a block not yet written.
    pub position: u64,
    /// Length as stored. Zero for a free element.
    pub length: u64,
    /// Length after decompression, or zero when the block is not compressed
    /// -- and also zero for a free element, so it is not a reliable signal on
    /// its own.
    pub uncompressed_length: u64,
}

impl IndexElement {
    /// Serialised size, fixed by the specification.
    pub const SIZE: usize = 40;

    /// Whether this is a placeholder rather than a real block.
    pub fn is_free(&self) -> bool {
        self.position == 0
    }

    fn parse(bytes: &[u8]) -> Self {
        let read = |offset: usize| {
            u64::from_le_bytes(bytes[offset..offset + 8].try_into().expect("checked length"))
        };
        IndexElement {
            id: read(0),
            position: read(8),
            length: read(16),
            uncompressed_length: read(24),
            // Bytes 32..40 are reserved and must be zero; nothing reads them.
        }
    }

    fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.id.to_le_bytes());
        out.extend_from_slice(&self.position.to_le_bytes());
        out.extend_from_slice(&self.length.to_le_bytes());
        out.extend_from_slice(&self.uncompressed_length.to_le_bytes());
        out.extend_from_slice(&[0u8; 8]);
    }
}

/// A parsed data blocks file: its index, flattened.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct BlocksIndex {
    /// Every element from every node, in the order the list walks them.
    pub elements: Vec<IndexElement>,
}

impl BlocksIndex {
    /// The element with a given identifier, if the file has one.
    pub fn get(&self, id: u64) -> Option<&IndexElement> {
        self.elements.iter().find(|e| e.id == id)
    }

    /// Elements that actually point at data, skipping free placeholders.
    pub fn occupied(&self) -> impl Iterator<Item = &IndexElement> {
        self.elements.iter().filter(|e| !e.is_free())
    }
}

/// How many index nodes a walk will follow before giving up.
///
/// The list is caller-controlled data, so a bound is not optional. A file with
/// this many nodes is already unreasonable, and one with more is either
/// hostile or broken.
const MAX_NODES: usize = 100_000;

/// Read a data blocks file's index.
pub fn parse_blocks_file(bytes: &[u8]) -> Result<BlocksIndex> {
    if bytes.len() < BLOCKS_PREAMBLE_LEN {
        return Err(err!(
            Truncated,
            "a data blocks file needs at least {BLOCKS_PREAMBLE_LEN} bytes, this has {}",
            bytes.len()
        ));
    }
    if &bytes[..8] != BLOCKS_SIGNATURE {
        return Err(err!(
            NotXisf,
            "expected the signature {:?}, found {:?}",
            String::from_utf8_lossy(BLOCKS_SIGNATURE),
            String::from_utf8_lossy(&bytes[..8])
        ));
    }
    if bytes[8..16] != [0; 8] {
        return Err(err!(BadHeader, "the reserved field must be zero"));
    }

    let mut elements = Vec::new();
    let mut seen: Vec<u64> = Vec::new();
    let mut position = BLOCKS_PREAMBLE_LEN as u64;

    for _ in 0..MAX_NODES {
        // A node position of zero ends the list.
        if position == 0 {
            return Ok(BlocksIndex { elements });
        }
        // A cycle would otherwise loop until the bound, which is a slow way
        // to say "corrupt". Naming it is better than timing out.
        if seen.contains(&position) {
            return Err(err!(BadHeader, "the block index loops back to position {position}"));
        }
        seen.push(position);

        let start = usize::try_from(position)
            .map_err(|_| err!(Truncated, "an index node lies beyond this platform's range"))?;
        // Length, reserved, next: 16 bytes of node header.
        let header_end = start
            .checked_add(16)
            .ok_or_else(|| err!(Truncated, "an index node position overflows"))?;
        if header_end > bytes.len() {
            return Err(err!(Truncated, "an index node at {start} runs past the end of the file"));
        }

        let count = u32::from_le_bytes(bytes[start..start + 4].try_into().expect("4 bytes"));
        if bytes[start + 4..start + 8] != [0; 4] {
            return Err(err!(BadHeader, "an index node's reserved field is not zero"));
        }
        let next = u64::from_le_bytes(bytes[start + 8..start + 16].try_into().expect("8 bytes"));

        let count = count as usize;
        let elements_end = count
            .checked_mul(IndexElement::SIZE)
            .and_then(|n| header_end.checked_add(n))
            .ok_or_else(|| err!(Truncated, "an index node declares too many elements"))?;
        if elements_end > bytes.len() {
            return Err(err!(
                Truncated,
                "an index node at {start} declares {count} elements, which run past the end"
            ));
        }

        for i in 0..count {
            let at = header_end + i * IndexElement::SIZE;
            elements.push(IndexElement::parse(&bytes[at..at + IndexElement::SIZE]));
        }
        position = next;
    }

    Err(err!(BadHeader, "the block index has more than {MAX_NODES} nodes"))
}

/// Build a data blocks file from a set of blocks.
///
/// Writes a single index node listing every block, then the blocks in order.
/// The specification allows several nodes and gaps, both of which exist so a
/// file can be extended in place; a fresh file needs neither.
pub fn write_blocks_file(blocks: &[(u64, Vec<u8>)]) -> Result<Vec<u8>> {
    let node_size = 16 + blocks.len() * IndexElement::SIZE;
    let first_block = BLOCKS_PREAMBLE_LEN
        .checked_add(node_size)
        .ok_or_else(|| err!(Unsupported, "too many blocks for this platform"))?;

    let mut seen: Vec<u64> = Vec::new();
    let mut elements = Vec::with_capacity(blocks.len());
    let mut position = first_block as u64;
    for (id, data) in blocks {
        // The identifier is how the header names a block, so a duplicate
        // makes the file ambiguous rather than merely odd.
        if seen.contains(id) {
            return Err(err!(InvalidArgument, "two blocks share the identifier {id}"));
        }
        seen.push(*id);

        elements.push(IndexElement {
            id: *id,
            position,
            length: data.len() as u64,
            uncompressed_length: 0,
        });
        position += data.len() as u64;
    }

    let mut out =
        Vec::with_capacity(first_block + blocks.iter().map(|(_, d)| d.len()).sum::<usize>());
    out.extend_from_slice(BLOCKS_SIGNATURE);
    out.extend_from_slice(&[0u8; 8]);

    out.extend_from_slice(&(blocks.len() as u32).to_le_bytes());
    out.extend_from_slice(&[0u8; 4]);
    // One node, so there is no next.
    out.extend_from_slice(&0u64.to_le_bytes());
    for element in &elements {
        element.write(&mut out);
    }

    for (_, data) in blocks {
        out.extend_from_slice(data);
    }
    Ok(out)
}

/// Read the header of a distributed unit, which is the whole `.xish` file.
///
/// Unlike a monolithic file there is no signature to check, so a wrong file
/// here fails as an XML error rather than a signature mismatch. The root
/// element check in [`crate::header::parse`] is what catches it.
pub fn parse_header_file(bytes: &[u8]) -> Result<crate::header::Header> {
    let text = std::str::from_utf8(bytes)
        .map_err(|e| err!(BadHeader, "an XISF header file must be UTF-8: {e}"))?;
    crate::header::parse(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ErrorKind;

    fn blocks_file(blocks: &[(u64, &[u8])]) -> Vec<u8> {
        let owned: Vec<(u64, Vec<u8>)> = blocks.iter().map(|(id, d)| (*id, d.to_vec())).collect();
        write_blocks_file(&owned).expect("write")
    }

    #[test]
    fn a_blocks_file_round_trips() {
        let bytes = blocks_file(&[(1, b"first"), (7, b"second block"), (99, b"")]);
        let index = parse_blocks_file(&bytes).expect("parse");

        assert_eq!(index.elements.len(), 3);
        assert_eq!(index.get(7).unwrap().length, 12);
        assert_eq!(index.get(99).unwrap().length, 0);
        assert!(index.get(1234).is_none());

        // And the positions must actually address the data.
        for (id, expected) in [(1u64, &b"first"[..]), (7, b"second block")] {
            let element = index.get(id).unwrap();
            let start = element.position as usize;
            let end = start + element.length as usize;
            assert_eq!(&bytes[start..end], expected, "block {id} is not where the index says");
        }
    }

    #[test]
    fn the_signature_is_xisb_not_xisf() {
        let mut bytes = blocks_file(&[(1, b"x")]);
        bytes[3] = b'F'; // XISB -> XISF
        assert_eq!(parse_blocks_file(&bytes).unwrap_err().kind(), ErrorKind::NotXisf);
    }

    #[test]
    fn free_elements_are_recognised_and_skipped() {
        // Built by hand: a free element has position and length zero.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(BLOCKS_SIGNATURE);
        bytes.extend_from_slice(&[0u8; 8]);
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 4]);
        bytes.extend_from_slice(&0u64.to_le_bytes());
        IndexElement { id: 1, position: 0, length: 0, uncompressed_length: 0 }.write(&mut bytes);
        let data_at = (16 + 16 + 2 * IndexElement::SIZE) as u64;
        IndexElement { id: 2, position: data_at, length: 3, uncompressed_length: 0 }
            .write(&mut bytes);
        bytes.extend_from_slice(b"abc");

        let index = parse_blocks_file(&bytes).expect("parse");
        assert_eq!(index.elements.len(), 2);
        assert!(index.get(1).unwrap().is_free());
        assert_eq!(index.occupied().count(), 1, "the free element should be skipped");
    }

    /// The index is a linked list of caller-controlled positions, so a cycle
    /// has to be caught rather than followed.
    #[test]
    fn a_cyclic_index_is_refused_rather_than_followed() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(BLOCKS_SIGNATURE);
        bytes.extend_from_slice(&[0u8; 8]);
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 4]);
        // Next points back at this very node.
        bytes.extend_from_slice(&(BLOCKS_PREAMBLE_LEN as u64).to_le_bytes());

        let err = parse_blocks_file(&bytes).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::BadHeader);
        assert!(err.message().contains("loops"), "{}", err.message());
    }

    #[test]
    fn a_node_pointing_past_the_end_is_refused() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(BLOCKS_SIGNATURE);
        bytes.extend_from_slice(&[0u8; 8]);
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 4]);
        bytes.extend_from_slice(&1_000_000u64.to_le_bytes());
        assert_eq!(parse_blocks_file(&bytes).unwrap_err().kind(), ErrorKind::Truncated);
    }

    /// A declared element count that runs past the file must not be trusted:
    /// it is the obvious way to make a reader read out of bounds.
    #[test]
    fn an_absurd_element_count_is_refused() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(BLOCKS_SIGNATURE);
        bytes.extend_from_slice(&[0u8; 8]);
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 4]);
        bytes.extend_from_slice(&0u64.to_le_bytes());
        assert_eq!(parse_blocks_file(&bytes).unwrap_err().kind(), ErrorKind::Truncated);
    }

    #[test]
    fn truncation_at_every_length_is_an_error_not_a_panic() {
        let bytes = blocks_file(&[(1, b"first"), (2, b"second")]);
        for n in 0..bytes.len() {
            let _ = parse_blocks_file(&bytes[..n]);
        }
    }

    #[test]
    fn corruption_of_any_single_byte_never_panics() {
        let original = blocks_file(&[(1, b"first"), (2, b"second")]);
        for i in 0..original.len().min(200) {
            let mut bytes = original.clone();
            bytes[i] ^= 0xff;
            let _ = parse_blocks_file(&bytes);
        }
    }

    #[test]
    fn duplicate_identifiers_are_refused_when_writing() {
        let err = write_blocks_file(&[(1, b"a".to_vec()), (1, b"b".to_vec())]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidArgument);
    }

    #[test]
    fn an_empty_blocks_file_is_valid() {
        let bytes = write_blocks_file(&[]).expect("write");
        let index = parse_blocks_file(&bytes).expect("parse");
        assert!(index.elements.is_empty());
    }

    #[test]
    fn a_header_file_is_the_header_alone() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<xisf version="1.0"><Image geometry="4:4:1" sampleFormat="UInt8"
      location="path(data.xisb)"/></xisf>"#;
        let header = parse_header_file(xml.as_bytes()).expect("parse");
        assert_eq!(header.images().len(), 1);

        // A monolithic file is not a header file: it starts with a signature,
        // which is not XML.
        let mut monolithic = Vec::from(crate::layout::SIGNATURE);
        monolithic.extend_from_slice(&[0u8; 8]);
        assert!(parse_header_file(&monolithic).is_err());
    }
}

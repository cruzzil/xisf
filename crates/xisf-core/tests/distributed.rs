//! A distributed unit, end to end: header file plus data blocks file.
//!
//! The pieces are unit-tested separately; this checks they fit together, which
//! is where the interesting mistake lives. A `location="path:blocks.xisb:7"`
//! names a block by its *identifier* in the file's index, not by its position
//! and not by an ordinal — a reader that treated the whole file as the block,
//! or the number as an offset, would produce plausible-looking rubbish rather
//! than an error.

use std::path::PathBuf;

use xisf_core::distributed::{parse_blocks_file, write_blocks_file};
use xisf_core::{ErrorKind, Reader};

/// A scratch directory that removes itself.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("xisf-distributed-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("scratch directory");
        Self(path)
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The pixel data for a 4x4 UInt16 image: deterministic, and distinguishable
/// from the signature and index bytes that surround it in the file.
fn pixels() -> Vec<u8> {
    (0..32u32).map(|i| (i * 7 + 13) as u8).collect()
}

fn header_naming(block: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><xisf version="1.0"
xmlns="http://www.pixinsight.com/xisf"><Image geometry="4:4:1"
sampleFormat="UInt16" colorSpace="Gray" location="{block}"/></xisf>"#
    )
}

/// A header file that is not monolithic, addressing a block in a `.xisb`.
fn write_unit(scratch: &Scratch, blocks: &[(u64, Vec<u8>)], locator: &str) -> PathBuf {
    std::fs::write(scratch.join("data.xisb"), write_blocks_file(blocks).expect("blocks file"))
        .expect("write blocks");

    // The reader resolves `path:` relative to the file it came from, so the
    // header has to be a real file on disk beside the blocks.
    let header = scratch.join("unit.xish");
    std::fs::write(&header, header_naming(locator)).expect("write header");
    header
}

/// A monolithic wrapper whose only job is to carry the same `location`, so the
/// existing `Reader` can be pointed at it. A `.xish` has no signature, so it
/// is not something `Reader::open` accepts.
fn monolithic_naming(scratch: &Scratch, locator: &str) -> PathBuf {
    let xml = header_naming(locator);
    let mut bytes = Vec::from(*b"XISF0100");
    bytes.extend_from_slice(&(xml.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&[0u8; 4]);
    bytes.extend_from_slice(xml.as_bytes());

    let path = scratch.join("unit.xisf");
    std::fs::write(&path, bytes).expect("write");
    path
}

#[test]
fn a_block_is_read_out_of_a_data_blocks_file_by_identifier() {
    let scratch = Scratch::new("by-id");
    let data = pixels();

    // Three blocks, with the wanted one neither first nor last, so an
    // implementation that ignored the identifier would pick the wrong bytes.
    let blocks = vec![(1u64, vec![0xAA; 16]), (7u64, data.clone()), (9u64, vec![0xBB; 8])];
    std::fs::write(scratch.join("data.xisb"), write_blocks_file(&blocks).unwrap()).unwrap();

    let path = monolithic_naming(&scratch, "path(data.xisb):7");
    let reader = Reader::open(&path).expect("open");
    let image = reader.header().images()[0];

    let read = reader.block(&image.data).expect("block");
    assert_eq!(&*read, &data[..], "the wrong block was read");
}

/// A blocks file addressed without an identifier is ambiguous, and guessing
/// would hand back the signature and index as pixel data.
#[test]
fn a_blocks_file_must_be_addressed_by_identifier() {
    let scratch = Scratch::new("no-id");
    std::fs::write(scratch.join("data.xisb"), write_blocks_file(&[(1, pixels())]).unwrap())
        .unwrap();

    let path = monolithic_naming(&scratch, "path(data.xisb)");
    let reader = Reader::open(&path).expect("open");
    let err = reader.block(&reader.header().images()[0].data).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::BadAttribute);
    assert!(err.message().contains("must name a block"), "{}", err.message());
}

/// A plain external file is the whole block, which is the other legal shape.
#[test]
fn a_plain_external_file_is_the_whole_block() {
    let scratch = Scratch::new("plain");
    let data = pixels();
    std::fs::write(scratch.join("raw.dat"), &data).unwrap();

    let path = monolithic_naming(&scratch, "path(raw.dat)");
    let reader = Reader::open(&path).expect("open");
    let read = reader.block(&reader.header().images()[0].data).expect("block");
    assert_eq!(&*read, &data[..]);
}

/// ...and naming a block inside one is a mistake worth reporting.
#[test]
fn a_plain_file_may_not_be_addressed_by_identifier() {
    let scratch = Scratch::new("plain-id");
    std::fs::write(scratch.join("raw.dat"), pixels()).unwrap();

    let path = monolithic_naming(&scratch, "path(raw.dat):3");
    let reader = Reader::open(&path).expect("open");
    let err = reader.block(&reader.header().images()[0].data).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::BadAttribute);
}

#[test]
fn an_unknown_identifier_is_reported() {
    let scratch = Scratch::new("unknown-id");
    std::fs::write(scratch.join("data.xisb"), write_blocks_file(&[(1, pixels())]).unwrap())
        .unwrap();

    let path = monolithic_naming(&scratch, "path(data.xisb):42");
    let reader = Reader::open(&path).expect("open");
    let err = reader.block(&reader.header().images()[0].data).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::NotFound);
}

/// The header file half of a distributed unit parses on its own.
#[test]
fn a_header_file_parses_and_names_its_blocks() {
    let scratch = Scratch::new("header-file");
    let header_path = write_unit(&scratch, &[(3, pixels())], "path(data.xisb):3");

    let bytes = std::fs::read(&header_path).expect("read");
    let header = xisf_core::distributed::parse_header_file(&bytes).expect("parse");

    let images = header.images();
    assert_eq!(images.len(), 1);
    assert_eq!(
        images[0].data.location,
        Some(xisf_core::block::Location::Path { path: "data.xisb".into(), index: Some(3) })
    );
}

/// The index survives a round trip through the file format, positions
/// included -- the thing most likely to be off by the size of a header.
#[test]
fn index_positions_address_the_blocks_they_name() {
    let blocks: Vec<(u64, Vec<u8>)> =
        (0..5u64).map(|i| (i * 3 + 1, vec![i as u8; (i as usize + 1) * 10])).collect();
    let bytes = write_blocks_file(&blocks).expect("write");
    let index = parse_blocks_file(&bytes).expect("parse");

    assert_eq!(index.elements.len(), blocks.len());
    for (id, expected) in &blocks {
        let element = index.get(*id).unwrap_or_else(|| panic!("block {id} is missing"));
        let start = element.position as usize;
        let end = start + element.length as usize;
        assert_eq!(&bytes[start..end], &expected[..], "block {id} is not where the index says");
    }
}

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

// ---- Writing --------------------------------------------------------

/// The round trip that matters: write a distributed unit, save both halves,
/// and read the pixels back through the ordinary reader.
#[test]
fn a_distributed_unit_written_here_reads_back() {
    use xisf_core::image::{ColorSpace, Image, PixelStorage, SampleFormat};
    use xisf_core::writer::{BlockOptions, PendingImage, Writer};

    let scratch = Scratch::new("write-roundtrip");

    let mut writer = Writer::new().with_creator("xisf-rs");
    let mut expected = Vec::new();
    for (index, channels) in [1u64, 3, 1].into_iter().enumerate() {
        let image = Image {
            dimensions: vec![6, 5],
            channels,
            sample_format: SampleFormat::UInt16,
            color_space: if channels >= 3 { ColorSpace::Rgb } else { ColorSpace::Gray },
            pixel_storage: PixelStorage::Planar,
            bounds: None,
            id: None,
            uuid: None,
            image_type: None,
            offset: None,
            orientation: None,
        };
        let size = image.data_size().unwrap() as usize;
        let data: Vec<u8> = (0..size).map(|i| (i * 13 + index * 7) as u8).collect();
        expected.push(data.clone());
        writer
            .add_image(PendingImage::new(image, data, BlockOptions::default()))
            .expect("add_image");
    }

    let unit = writer.to_distributed("data.xisb").expect("to_distributed");
    std::fs::write(scratch.join("unit.xish"), &unit.header).unwrap();
    std::fs::write(scratch.join(&unit.blocks_file_name), &unit.blocks).unwrap();

    // The header file is XML and nothing else: no signature, no preamble.
    assert!(unit.header.starts_with(b"<?xml"), "a header file starts with its XML");
    assert!(unit.blocks.starts_with(b"XISB0100"), "a blocks file starts with XISB");

    // Read it back the way a consumer would.
    let header = xisf_core::distributed::parse_header_file(&unit.header).expect("parse header");
    assert_eq!(header.images().len(), 3);

    // And the blocks resolve. The reader needs a file on disk to resolve
    // `path(...)` relative to, so this goes through a monolithic wrapper
    // carrying the same locators.
    let index = parse_blocks_file(&unit.blocks).expect("parse blocks");
    assert_eq!(index.occupied().count(), 3);

    for (n, image) in header.images().iter().enumerate() {
        let Some(xisf_core::block::Location::Path { path, index: id }) = &image.data.location
        else {
            panic!("image {n} is not addressed by path");
        };
        assert_eq!(path, "data.xisb");
        let element = index.get(id.expect("an identifier")).expect("the block is indexed");

        let start = element.position as usize;
        let end = start + element.length as usize;
        assert_eq!(&unit.blocks[start..end], &expected[n][..], "image {n} has the wrong bytes");
    }
}

/// Reading it through the ordinary `Reader`, which is what a consumer
/// actually does, rather than only through the pieces. Compressed on
/// purpose, so the block is not merely a copy of the input -- which needs a
/// codec compiled in.
#[cfg(feature = "zlib")]
#[test]
fn the_reader_resolves_a_written_distributed_unit() {
    use xisf_core::image::{ColorSpace, Image, PixelStorage, SampleFormat};
    use xisf_core::writer::{BlockOptions, CompressionRequest, PendingImage, Writer};

    let scratch = Scratch::new("reader-resolves");
    let image = Image {
        dimensions: vec![8, 4],
        channels: 1,
        sample_format: SampleFormat::UInt16,
        color_space: ColorSpace::Gray,
        pixel_storage: PixelStorage::Planar,
        bounds: None,
        id: None,
        uuid: None,
        image_type: None,
        offset: None,
        orientation: None,
    };
    let size = image.data_size().unwrap() as usize;
    let data: Vec<u8> = (0..size).map(|i| (i * 5 + 1) as u8).collect();

    let mut writer = Writer::new();
    writer
        .add_image(PendingImage::new(
            image,
            data.clone(),
            BlockOptions {
                // Compressed, so the block is not merely a copy of the input.
                compression: Some(CompressionRequest {
                    codec: xisf_core::writer::Codec2::Zlib,
                    shuffle_item_size: Some(2),
                }),
                checksum: None,
            },
        ))
        .unwrap();

    let unit = writer.to_distributed("blocks.xisb").expect("to_distributed");
    std::fs::write(scratch.join(&unit.blocks_file_name), &unit.blocks).unwrap();

    // A monolithic file carrying the distributed header's body, so `Reader`
    // has a path to resolve `path(blocks.xisb)` against.
    let xml = String::from_utf8(unit.header.clone()).unwrap();
    let mut monolithic = Vec::from(*b"XISF0100");
    monolithic.extend_from_slice(&(xml.len() as u32).to_le_bytes());
    monolithic.extend_from_slice(&[0u8; 4]);
    monolithic.extend_from_slice(xml.as_bytes());
    let path = scratch.join("unit.xisf");
    std::fs::write(&path, monolithic).unwrap();

    let reader = Reader::open(&path).expect("open");
    let read = reader.block(&reader.header().images()[0].data).expect("block");
    assert_eq!(&*read, &data[..], "the pixels did not survive the round trip");
}

#[test]
fn a_blocks_file_name_must_be_a_plain_name() {
    use xisf_core::writer::Writer;
    for name in ["", "sub/dir.xisb", "/absolute.xisb"] {
        assert!(Writer::new().to_distributed(name).is_err(), "{name:?} should have been refused");
    }
}

/// A distributed unit's header file opens directly.
///
/// A `.xish` is XML with no signature, so it used to be rejected outright and
/// the only way to read one was to wrap it in a monolithic file by hand. The
/// writer produces these, which made them a form the library could emit but
/// not read back.
#[test]
fn a_header_file_opens_without_being_wrapped() {
    let scratch = Scratch::new("xish-open");
    let data = pixels();
    let blocks = vec![(1u64, data.clone())];
    std::fs::write(scratch.join("data.xisb"), write_blocks_file(&blocks).unwrap()).unwrap();

    let path = scratch.join("unit.xish");
    std::fs::write(&path, header_naming("path(data.xisb):0x1")).unwrap();

    let reader = Reader::open(&path).expect("a .xish should open");
    let image = reader.header().images()[0];
    assert_eq!(&*reader.block(&image.data).expect("block"), &data[..]);
}

/// Detection is by content, not by suffix, and the two forms cannot collide:
/// an XML document cannot begin with the eight signature bytes.
#[test]
fn the_two_file_forms_are_told_apart_by_what_they_contain() {
    let scratch = Scratch::new("forms");
    let xml = header_naming("inline:base64");

    // A header file under the wrong name still opens...
    let odd = scratch.join("unit.xisf");
    std::fs::write(&odd, &xml).unwrap();
    assert!(Reader::open(&odd).is_ok(), "a header file was refused for its name");

    // ...a leading byte order mark is tolerated, since UTF-8 XML may carry
    // one and it is not part of the document...
    let bom = scratch.join("bom.xish");
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(xml.as_bytes());
    std::fs::write(&bom, &bytes).unwrap();
    assert!(Reader::open(&bom).is_ok(), "a byte order mark was not skipped");

    // ...and something that is neither is still refused as neither.
    let junk = scratch.join("junk.xisf");
    std::fs::write(&junk, b"not xisf and not xml either").unwrap();
    let err = Reader::open(&junk).expect_err("arbitrary bytes were accepted");
    assert_eq!(err.kind(), xisf_core::ErrorKind::NotXisf);
}

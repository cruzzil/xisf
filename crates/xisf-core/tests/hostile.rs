//! What a reader must refuse when the file is not friendly.
//!
//! An XISF header is untrusted input in the ordinary case, not the exotic one:
//! images are downloaded, shared and unpacked from archives all the time, and
//! whoever supplies the file usually supplies the directory it lands in too.
//! Every locator, size and offset in the header is therefore attacker-chosen,
//! and the reader's job is to refuse rather than to trust.
//!
//! Each test here corresponds to something that actually worked at some point
//! during development, so none of them is hypothetical.

use std::path::PathBuf;

use xisf_core::{ErrorKind, Reader};

/// A scratch directory that removes itself.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("xisf-hostile-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("scratch");
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

/// Write a monolithic file whose single image uses `location`.
fn unit_with(scratch: &Scratch, name: &str, location: &str) -> PathBuf {
    let xml = format!(
        r#"<xisf version="1.0"><Image geometry="4:1:1" sampleFormat="UInt8" location="{location}"/></xisf>"#
    );
    let mut bytes = Vec::from(*b"XISF0100");
    bytes.extend_from_slice(&(xml.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&[0u8; 4]);
    bytes.extend_from_slice(xml.as_bytes());

    let path = scratch.join(name);
    std::fs::write(&path, bytes).expect("write");
    path
}

fn block_of(path: &PathBuf) -> xisf_core::Result<Vec<u8>> {
    let reader = Reader::open(path)?;
    let data = reader.header().images()[0].data.clone();
    reader.stored_block(&data).map(|b| b.to_vec())
}

/// A locator with no `..` in it at all can still leave the directory, by
/// being a symbolic link. Checking the path's *components* passes it; only
/// resolving both ends and checking containment catches it.
#[cfg(unix)]
#[test]
fn a_symlinked_locator_cannot_escape_the_directory() {
    let scratch = Scratch::new("symlink");
    std::fs::create_dir_all(scratch.join("unit")).unwrap();
    std::fs::write(scratch.join("secret.txt"), b"TOP SECRET").unwrap();
    std::os::unix::fs::symlink(scratch.join("secret.txt"), scratch.join("unit/blocks.xisb"))
        .unwrap();

    let xml = r#"<xisf version="1.0"><Image geometry="4:1:1" sampleFormat="UInt8" location="path(blocks.xisb)"/></xisf>"#;
    let mut bytes = Vec::from(*b"XISF0100");
    bytes.extend_from_slice(&(xml.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&[0u8; 4]);
    bytes.extend_from_slice(xml.as_bytes());
    let path = scratch.join("unit/unit.xisf");
    std::fs::write(&path, bytes).unwrap();

    let err = block_of(&path).expect_err("a symlink out of the directory was followed");
    assert_eq!(err.kind(), ErrorKind::BadAttribute);
    assert!(err.message().contains("outside"), "{}", err.message());
}

/// A symlink that stays inside the directory is fine: the rule is
/// containment, not a ban on links.
#[cfg(unix)]
#[test]
fn a_symlink_within_the_directory_is_allowed() {
    let scratch = Scratch::new("symlink-ok");
    std::fs::write(scratch.join("real.bin"), b"data").unwrap();
    std::os::unix::fs::symlink(scratch.join("real.bin"), scratch.join("link.bin")).unwrap();

    let path = unit_with(&scratch, "unit.xisf", "path(link.bin)");
    assert_eq!(block_of(&path).expect("an in-directory link should be read"), b"data");
}

/// A fifo passes every path check and then never ends. `/dev/zero` is the
/// same shape: a locator that names one turns a read into an unbounded one.
/// Skipped under Miri, which interprets rather than executes and so cannot
/// spawn `mkfifo`. The property still holds there; it just cannot be built.
#[cfg(all(unix, not(miri)))]
#[test]
fn a_locator_naming_something_that_is_not_a_file_is_refused() {
    let scratch = Scratch::new("fifo");
    let fifo = scratch.join("pipe.xisb");

    // mkfifo via the shell, so this needs no libc dependency.
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !made {
        eprintln!("skipping: mkfifo unavailable");
        return;
    }

    let path = unit_with(&scratch, "unit.xisf", "path(pipe.xisb)");
    let err = block_of(&path).expect_err("a fifo was opened as a data block");
    assert_eq!(err.kind(), ErrorKind::BadAttribute);
    assert!(err.message().contains("not a regular file"), "{}", err.message());
}

/// Traversal in the locator itself, which is the obvious form.
#[test]
fn traversal_in_the_locator_is_refused() {
    let scratch = Scratch::new("traversal");
    std::fs::create_dir_all(scratch.join("unit")).unwrap();
    std::fs::write(scratch.join("secret.txt"), b"secret").unwrap();

    for locator in ["path(../secret.txt)", "path(a/../../secret.txt)", "path(/etc/hostname)"] {
        let xml = format!(
            r#"<xisf version="1.0"><Image geometry="4:1:1" sampleFormat="UInt8" location="{locator}"/></xisf>"#
        );
        let mut bytes = Vec::from(*b"XISF0100");
        bytes.extend_from_slice(&(xml.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&[0u8; 4]);
        bytes.extend_from_slice(xml.as_bytes());
        let path = scratch.join("unit/unit.xisf");
        std::fs::write(&path, bytes).unwrap();

        assert!(block_of(&path).is_err(), "{locator} was followed");
    }
}

/// A remote block must not be fetched by the library at all. This is the
/// property that makes `url(...)` safe by default: not that the fetch is
/// careful, but that there is no fetch.
#[test]
fn a_remote_block_is_not_fetched_without_a_resolver() {
    let scratch = Scratch::new("url");
    let path = unit_with(&scratch, "unit.xisf", "url(http://169.254.169.254/latest/meta-data/)");

    let err = block_of(&path).expect_err("a URL was fetched");
    assert_eq!(err.kind(), ErrorKind::Unsupported);
    assert!(err.message().contains("set_url_resolver"), "{}", err.message());
}

/// An attachment offset is an absolute file position chosen by the header.
#[test]
fn attachment_offsets_are_bounded_at_both_ends() {
    let scratch = Scratch::new("attachment");
    for location in [
        "attachment:0:4",                    // inside the header
        "attachment:100000:4",               // past the end
        "attachment:4:18446744073709551615", // length overflows
        "attachment:18446744073709551615:4", // position overflows
    ] {
        let path = unit_with(&scratch, "unit.xisf", location);
        assert!(block_of(&path).is_err(), "{location} was accepted");
    }
}

/// How wide the brute-force sweeps below go.
///
/// Miri interprets rather than executes, so a sweep of thousands of parses
/// takes hours there rather than a second. It is narrowed rather than skipped:
/// under Miri these tests exist to check the memory map and the unsafe code a
/// parse touches, which a handful of inputs exercises just as well, while the
/// breadth belongs to the ordinary test run on every platform.
const SWEEP: usize = if cfg!(miri) { 24 } else { 600 };

/// The whole corpus, truncated at every length, must never panic. A reader
/// that indexes past the end on a short file is a crash in a decoder.
#[test]
fn truncation_at_every_length_never_panics() {
    let corpus = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus");
    let mut files = 0;

    for sub in ["pixinsight", "generated"] {
        let Ok(entries) = std::fs::read_dir(corpus.join(sub)) else { continue };
        for entry in entries.flatten().take(if cfg!(miri) { 1 } else { 6 }) {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "xisf") {
                continue;
            }
            let bytes = std::fs::read(&path).expect("read");
            files += 1;

            // Every length up to the header, then a sample beyond it: the
            // interesting boundaries are all in the first few hundred bytes,
            // and the block offsets past them.
            let dense = bytes.len().min(SWEEP);
            for n in 0..dense {
                exercise(&bytes[..n]);
            }
            let stride = if cfg!(miri) { bytes.len().max(1) } else { 97 };
            for n in (dense..bytes.len()).step_by(stride) {
                exercise(&bytes[..n]);
            }
        }
    }
    assert!(files > 0, "no corpus file was truncated");
}

/// Flipping any single byte must never panic either.
#[test]
fn single_byte_corruption_never_panics() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/pixinsight/simple_10by10.xisf");
    let Ok(original) = std::fs::read(&path) else {
        eprintln!("skipping: corpus file missing");
        return;
    };

    for i in 0..original.len().min(SWEEP * 2) {
        let mut bytes = original.clone();
        bytes[i] ^= 0xff;
        exercise(&bytes);
    }
}

/// Everything a caller might reach for, so a panic anywhere surfaces.
fn exercise(bytes: &[u8]) {
    let Ok(reader) = Reader::from_bytes(bytes.to_vec()) else { return };
    for element in reader.header().root.descendants() {
        let _ = reader.stored_block(&element.data);
        let _ = reader.block(&element.data);
        let _ = reader.verify(&element.data);
        let _ = xisf_core::image::Image::parse(element);
        let _ = xisf_core::property::Property::parse(element);
    }
}

/// Deeply nested XML must be refused rather than crashing the process.
///
/// A stack overflow in Rust aborts: it is not a panic and cannot be caught,
/// so a library that overflows on a hostile file takes the whole process with
/// it, including every unrelated request it was serving. The tree is built by
/// the parser but walked, cloned, compared and *dropped* recursively, so the
/// depth has to be bounded when it is read rather than at each use.
///
/// 300,000 elements is 2MB of XML -- nothing, as uploads go.
#[test]
fn deeply_nested_xml_is_refused_rather_than_overflowing_the_stack() {
    let depth = 300_000;
    let mut xml = String::from(r#"<xisf version="1.0">"#);
    for _ in 0..depth {
        xml.push_str("<a>");
    }
    for _ in 0..depth {
        xml.push_str("</a>");
    }
    xml.push_str("</xisf>");

    let mut bytes = Vec::from(*b"XISF0100");
    bytes.extend_from_slice(&(xml.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&[0u8; 4]);
    bytes.extend_from_slice(xml.as_bytes());

    let err = Reader::from_bytes(bytes).expect_err("a 300,000-deep header was accepted");
    assert_eq!(err.kind(), ErrorKind::BadHeader);
    assert!(err.message().contains("nested"), "{}", err.message());
}

/// A header that is mostly elements turns a small file into a large tree.
/// The cap is on what the parser will build, not on what the caller asks for
/// afterwards, because by then the memory is already committed.
#[test]
fn an_absurd_number_of_elements_is_refused() {
    let mut xml = String::from(r#"<xisf version="1.0">"#);
    for _ in 0..2_000_000 {
        xml.push_str("<a/>");
    }
    xml.push_str("</xisf>");

    let mut bytes = Vec::from(*b"XISF0100");
    bytes.extend_from_slice(&(xml.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&[0u8; 4]);
    bytes.extend_from_slice(xml.as_bytes());

    let err = Reader::from_bytes(bytes).expect_err("two million elements were accepted");
    assert_eq!(err.kind(), ErrorKind::BadHeader);
    assert!(err.message().contains("elements"), "{}", err.message());
}

/// The limits must not be so tight that they reject real files. Every file in
/// the corpus, including PixInsight's own, has to keep opening.
#[test]
fn the_structural_limits_do_not_reject_real_files() {
    let corpus = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus");
    let mut opened = 0;
    for sub in ["pixinsight", "generated"] {
        let Ok(entries) = std::fs::read_dir(corpus.join(sub)) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "xisf") {
                continue;
            }
            Reader::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            opened += 1;
        }
    }
    assert!(opened > 0, "no corpus file was opened");
}

/// A file that records a checksum is telling the reader how to know whether
/// the bytes are the bytes that were written. Handing them over without
/// looking is a silent corruption the format went out of its way to make
/// detectable, so the check is on unless a caller turns it off.
#[cfg(feature = "checksums")]
#[test]
fn a_block_that_fails_its_own_checksum_is_refused_by_default() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/pixinsight/Sample_F32_ZlibCompression_Sha256Security.xisf");
    let Ok(original) = std::fs::read(&path) else {
        eprintln!("skipping: corpus file missing");
        return;
    };

    // Locate the image block from the header rather than guessing, then flip
    // a byte inside it: the checksum covers the block as stored.
    let reader = Reader::from_bytes(original.clone()).expect("open");
    let data = reader.header().images()[0].data.clone();
    assert!(data.checksum.is_some(), "this corpus file should record a checksum");
    let Some(xisf_core::block::Location::Attachment { position, .. }) = data.location else {
        panic!("expected an attached block");
    };

    let mut corrupted = original.clone();
    corrupted[position as usize] ^= 0x01;

    let reader = Reader::from_bytes(corrupted).expect("the header is still intact");
    let data = reader.header().images()[0].data.clone();

    // `stored_block`, not `block`: the checksum covers the block as stored,
    // so this is checkable without a codec feature being compiled in.
    let err = reader.stored_block(&data).expect_err("a corrupted block was handed over");
    assert_eq!(err.kind(), ErrorKind::ChecksumMismatch);

    // `verify` still reports rather than fails, which is what makes it useful
    // for telling "corrupt" apart from "unreadable".
    assert_eq!(reader.verify(&data).expect("verify"), xisf_core::ChecksumStatus::Invalid);

    // And a caller who wants the bytes anyway can still have them.
    let mut lenient = Reader::from_bytes(original).expect("open");
    lenient.set_verify_checksums(false);
    let data = lenient.header().images()[0].data.clone();
    assert!(lenient.stored_block(&data).is_ok(), "the intact file should read either way");
}

/// The block index is a linked list of nodes, each declaring how many index
/// elements follow it. Cycles are caught and the node count is bounded, but
/// nothing stopped the nodes from *overlapping*: a hundred thousand distinct
/// positions, each declaring a whole file's worth of elements, is a small
/// file that asks for hundreds of gigabytes of index.
///
/// Both ways in are covered. A local `path(...)` block seeks through the
/// index and holds one element at a time, so it cannot accumulate at all; a
/// `url(...)` block arrives as bytes a resolver already fetched, and that
/// path builds the whole index, so it is bounded by what the file could
/// honestly describe.
#[test]
fn overlapping_index_nodes_cannot_multiply_into_a_memory_bomb() {
    const SIZE: usize = 64 * 1024;
    let mut blocks = vec![0u8; SIZE];
    blocks[..8].copy_from_slice(b"XISB0100");

    // As many nodes as fit, sixteen bytes apart, each declaring as many
    // elements as still fit in the file from where it sits -- so every node
    // is individually well formed and within bounds. The element data they
    // point at overlaps the following nodes, which nothing forbids.
    let mut declared = 0u64;
    let mut position = 16usize;
    while position + 32 <= SIZE {
        let next = position + 16;
        let count = ((SIZE - position - 16) / 40) as u32;
        blocks[position..position + 4].copy_from_slice(&count.to_le_bytes());
        blocks[position + 4..position + 8].copy_from_slice(&[0; 4]);
        blocks[position + 8..position + 16].copy_from_slice(&(next as u64).to_le_bytes());
        declared += u64::from(count);
        position = next;
    }
    assert!(declared > 1_000_000, "the construction should declare millions: {declared}");

    // A 64KB file can describe at most 1,638 blocks, since each costs forty
    // bytes on disk. It claims over a million.
    let err = xisf_core::distributed::parse_blocks_file(&blocks)
        .expect_err("an index declaring millions of elements was built in full");
    assert!(err.message().contains("elements"), "{}", err.message());

    // The seeking path walks the same file without accumulating, so it
    // finishes rather than exhausting memory, whatever it concludes.
    let scratch = Scratch::new("index-bomb");
    std::fs::write(scratch.join("data.xisb"), &blocks).unwrap();
    let path = unit_with(&scratch, "unit.xisf", "path(data.xisb):0x1");
    let _ = block_of(&path);
}

/// The same treatment the monolithic path gets, for data blocks files.
///
/// A blocks file is parsed with far more arithmetic than a header is --
/// positions, lengths and a linked list, all read from the file -- and it
/// arrives from wherever the header pointed, so it is no more trustworthy.
/// Both routes into it are swept: the slice parser a `url(...)` block uses,
/// and the seeking reader a `path(...)` block uses.
#[test]
fn a_corrupt_blocks_file_never_panics() {
    let blocks: Vec<(u64, Vec<u8>)> =
        vec![(1, vec![0xAA; 64]), (7, vec![0xBB; 200]), (9, Vec::new())];
    let original = xisf_core::distributed::write_blocks_file(&blocks).expect("write");

    let scratch = Scratch::new("blocks-fuzz");
    let probe = |bytes: &[u8]| {
        let _ = xisf_core::distributed::parse_blocks_file(bytes);

        // And through the reader, which seeks rather than parsing in full.
        std::fs::write(scratch.join("data.xisb"), bytes).unwrap();
        for locator in ["path(data.xisb):0x1", "path(data.xisb):0x7", "path(data.xisb):0x9"] {
            let path = unit_with(&scratch, "unit.xisf", locator);
            let _ = block_of(&path);
        }
    };

    for n in 0..original.len() {
        probe(&original[..n]);
    }
    for i in 0..original.len() {
        let mut bytes = original.clone();
        bytes[i] ^= 0xff;
        probe(&bytes);
    }
    // Every byte of the first index node set to each extreme, since that is
    // where the counts and pointers live.
    for i in 16..original.len().min(96) {
        for value in [0x00, 0x01, 0x7f, 0x80, 0xff] {
            let mut bytes = original.clone();
            bytes[i] = value;
            probe(&bytes);
        }
    }
}

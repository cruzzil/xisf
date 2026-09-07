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
#[cfg(unix)]
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

/// The whole corpus, truncated at every length, must never panic. A reader
/// that indexes past the end on a short file is a crash in a decoder.
#[test]
fn truncation_at_every_length_never_panics() {
    let corpus = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus");
    let mut files = 0;

    for sub in ["pixinsight", "generated"] {
        let Ok(entries) = std::fs::read_dir(corpus.join(sub)) else { continue };
        for entry in entries.flatten().take(6) {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "xisf") {
                continue;
            }
            let bytes = std::fs::read(&path).expect("read");
            files += 1;

            // Every length up to the header, then a sample beyond it: the
            // interesting boundaries are all in the first few hundred bytes,
            // and the block offsets past them.
            let dense = bytes.len().min(600);
            for n in 0..dense {
                exercise(&bytes[..n]);
            }
            for n in (dense..bytes.len()).step_by(97) {
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

    for i in 0..original.len().min(1500) {
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

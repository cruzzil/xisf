//! Opening a monolithic XISF file and getting data blocks out of it.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use base64::Engine as _;

use crate::block::{Checksum, ChecksumAlgorithm, Location, TextEncoding};
use crate::codec;
use crate::err;
use crate::error::Result;
use crate::header::{self, DataRef, Header};
use crate::layout::{self, Layout};

/// Whether a block's stored bytes matched its recorded checksum.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChecksumStatus {
    /// The element records no checksum. The spec makes it optional, so this
    /// is not a failure.
    Absent,
    Valid,
    Invalid,
}

impl ChecksumStatus {
    pub fn is_failure(self) -> bool {
        self == ChecksumStatus::Invalid
    }
}

/// Where a reader's bytes come from.
enum Source {
    Mapped(memmap2::Mmap),
    Owned(Vec<u8>),
}

impl std::ops::Deref for Source {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Source::Mapped(m) => m,
            Source::Owned(v) => v,
        }
    }
}

impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self {
            Source::Mapped(_) => "Mapped",
            Source::Owned(_) => "Owned",
        };
        write!(f, "{kind}({} bytes)", self.len())
    }
}

/// An open XISF file.
#[derive(Debug)]
pub struct Reader {
    source: Source,
    layout: Layout,
    header: Header,
    /// Where the file came from, so a `path:` block can be resolved relative
    /// to it, as the spec requires.
    path: Option<PathBuf>,
}

impl Reader {
    /// Open and scan a file from disk.
    ///
    /// The file is memory-mapped, so an attached block costs nothing until it
    /// is read.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let file = std::fs::File::open(path)?;

        // SAFETY: mapping is unsafe because another process truncating the
        // file turns a later read into SIGBUS. That hazard is inherent to
        // memory mapping; XISF files are written whole rather than modified
        // in place, and mapping is what lets a multi-gigabyte image be read
        // without loading it all.
        let mapped = unsafe { memmap2::Mmap::map(&file) }?;
        let (layout, header) = Self::scan(&mapped)?;
        Ok(Self { source: Source::Mapped(mapped), layout, header, path: Some(path.into()) })
    }

    /// Scan a file already in memory.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        let (layout, header) = Self::scan(&bytes)?;
        Ok(Self { source: Source::Owned(bytes), layout, header, path: None })
    }

    fn scan(bytes: &[u8]) -> Result<(Layout, Header)> {
        let layout = layout::scan(bytes)?;
        let header = header::parse(layout::header_str(bytes, &layout)?)?;
        Ok((layout, header))
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn bytes(&self) -> &[u8] {
        &self.source
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The stored bytes of an element's data block, before decompression.
    ///
    /// An attached block is borrowed straight from the mapping; a text-encoded
    /// one has to be decoded, so it is owned.
    pub fn stored_block(&self, data: &DataRef) -> Result<Cow<'_, [u8]>> {
        let location = data
            .location
            .as_ref()
            .ok_or_else(|| err!(NotFound, "this element has no data block"))?;

        match location {
            Location::Attachment { position, size } => {
                let start = usize::try_from(*position)
                    .map_err(|_| err!(Unsupported, "block position is beyond this platform"))?;
                let len = usize::try_from(*size)
                    .map_err(|_| err!(Unsupported, "block size is beyond this platform"))?;
                let end = start
                    .checked_add(len)
                    .ok_or_else(|| err!(Truncated, "block position + size overflows"))?;

                // A `location` is caller-controlled addressing, so both ends
                // are checked before any slicing happens.
                if start < self.layout.data_start() {
                    return Err(err!(
                        BadAttribute,
                        "a block at {start} would overlap the header, which ends at {}",
                        self.layout.data_start()
                    ));
                }
                if end > self.source.len() {
                    return Err(err!(
                        Truncated,
                        "a block of {len} bytes at {start} runs {} past the end of the file",
                        end - self.source.len()
                    ));
                }
                Ok(Cow::Borrowed(&self.source[start..end]))
            }

            Location::Inline { encoding } => {
                let text = data
                    .text
                    .as_deref()
                    .ok_or_else(|| err!(NotFound, "an inline block with no character data"))?;
                Ok(Cow::Owned(decode_text(text, *encoding)?))
            }
            Location::Embedded => {
                let text = data
                    .text
                    .as_deref()
                    .ok_or_else(|| err!(NotFound, "an embedded block with no <Data> content"))?;
                Ok(Cow::Owned(decode_text(text, TextEncoding::Base64)?))
            }

            Location::Path { path, .. } => {
                Ok(Cow::Owned(std::fs::read(self.resolve_relative(path)?)?))
            }
            Location::Url { url, .. } => {
                Err(err!(Unsupported, "external URL blocks are not read: {url}"))
            }
        }
    }

    /// The usable bytes of a block: decoded, decompressed and unshuffled.
    pub fn block(&self, data: &DataRef) -> Result<Cow<'_, [u8]>> {
        let stored = self.stored_block(data)?;
        match &data.compression {
            None => Ok(stored),
            Some(compression) => Ok(Cow::Owned(codec::decode(&stored, compression)?)),
        }
    }

    /// Check a block's stored bytes against its recorded checksum.
    ///
    /// The checksum covers the block *as stored*, so a compressed block can be
    /// verified without being decompressed.
    pub fn verify(&self, data: &DataRef) -> Result<ChecksumStatus> {
        let Some(checksum) = &data.checksum else {
            return Ok(ChecksumStatus::Absent);
        };
        let stored = self.stored_block(data)?;
        Ok(if digest(&stored, checksum.algorithm)? == checksum.digest {
            ChecksumStatus::Valid
        } else {
            ChecksumStatus::Invalid
        })
    }

    /// Resolve a `path:` locator against the file's own directory.
    ///
    /// Anything that could escape that directory is refused: the locator names
    /// a companion file, and a header is not a reason to read elsewhere.
    fn resolve_relative(&self, locator: &str) -> Result<PathBuf> {
        let base = self.path.as_ref().and_then(|p| p.parent()).ok_or_else(|| {
            err!(NotFound, "this file has no directory to resolve {locator:?} in")
        })?;

        let candidate = Path::new(locator);
        if candidate.is_absolute() || locator.contains("://") {
            return Err(err!(BadAttribute, "{locator:?} is not a relative path"));
        }
        for component in candidate.components() {
            use std::path::Component;
            match component {
                Component::Normal(_) | Component::CurDir => {}
                _ => {
                    return Err(err!(
                        BadAttribute,
                        "{locator:?} climbs out of the referring file's directory"
                    ));
                }
            }
        }
        Ok(base.join(candidate))
    }
}

/// Decode a text-encoded block. Whitespace is insignificant in both encodings.
fn decode_text(text: &str, encoding: TextEncoding) -> Result<Vec<u8>> {
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    match encoding {
        TextEncoding::Base64 => base64::engine::general_purpose::STANDARD
            .decode(compact.as_bytes())
            .map_err(|e| err!(BadAttribute, "base64: {e}")),
        TextEncoding::Hex => {
            if !compact.len().is_multiple_of(2) {
                return Err(err!(BadAttribute, "hex data has an odd number of digits"));
            }
            (0..compact.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&compact[i..i + 2], 16))
                .collect::<std::result::Result<Vec<u8>, _>>()
                .map_err(|e| err!(BadAttribute, "hex: {e}"))
        }
    }
}

/// Hash `bytes` with the algorithm named.
pub fn digest(bytes: &[u8], algorithm: ChecksumAlgorithm) -> Result<Vec<u8>> {
    #[cfg(feature = "checksums")]
    {
        use sha1::Digest as _;
        Ok(match algorithm {
            ChecksumAlgorithm::Sha1 => sha1::Sha1::digest(bytes).to_vec(),
            ChecksumAlgorithm::Sha256 => sha2::Sha256::digest(bytes).to_vec(),
            ChecksumAlgorithm::Sha512 => sha2::Sha512::digest(bytes).to_vec(),
            ChecksumAlgorithm::Sha3_256 => sha3::Sha3_256::digest(bytes).to_vec(),
            ChecksumAlgorithm::Sha3_512 => sha3::Sha3_512::digest(bytes).to_vec(),
        })
    }
    #[cfg(not(feature = "checksums"))]
    {
        let _ = bytes;
        Err(err!(Unsupported, "{} support is not compiled in", algorithm.name()))
    }
}

/// The hex spelling of a digest, as a `checksum` attribute carries it.
pub fn digest_hex(digest: &[u8]) -> String {
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Recompute a checksum attribute for `bytes`.
pub fn checksum_of(bytes: &[u8], algorithm: ChecksumAlgorithm) -> Result<Checksum> {
    Ok(Checksum { algorithm, digest: digest(bytes, algorithm)? })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ErrorKind;

    fn build(header: &str, trailing: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(crate::layout::SIGNATURE);
        out.extend_from_slice(&(header.len() as u32).to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(header.as_bytes());
        out.extend_from_slice(trailing);
        out
    }

    #[test]
    fn reads_an_inline_block() {
        let xml = r#"<xisf version="1.0"><Image location="inline:base64">QUJD</Image></xisf>"#;
        let reader = Reader::from_bytes(build(xml, &[])).unwrap();
        let image = reader.header().images()[0];
        assert_eq!(&*reader.block(&image.data).unwrap(), b"ABC");
    }

    #[test]
    fn reads_a_hex_block_ignoring_whitespace() {
        let xml = r#"<xisf version="1.0"><Image location="inline:hex">41 42
        43</Image></xisf>"#;
        let reader = Reader::from_bytes(build(xml, &[])).unwrap();
        let image = reader.header().images()[0];
        assert_eq!(&*reader.block(&image.data).unwrap(), b"ABC");
    }

    #[test]
    fn reads_an_attached_block() {
        let xml = r#"<xisf version="1.0"><Image location="attachment:64:3"/></xisf>"#;
        let mut trailing = vec![0u8; 64];
        trailing.extend_from_slice(b"XYZ");
        let bytes = build(xml, &trailing[16 + xml.len()..]);
        let reader = Reader::from_bytes(bytes).unwrap();
        let image = reader.header().images()[0];
        assert_eq!(&*reader.block(&image.data).unwrap(), b"XYZ");
    }

    /// A `location` is addressing the caller controls, so every way of
    /// pointing it somewhere it should not go is refused rather than sliced.
    #[test]
    fn a_block_may_not_address_outside_the_file() {
        for location in [
            "attachment:0:10",                   // overlaps the header
            "attachment:100000:1",               // past the end
            "attachment:18446744073709551615:1", // overflows
        ] {
            let xml = format!(r#"<xisf version="1.0"><Image location="{location}"/></xisf>"#);
            let reader = Reader::from_bytes(build(&xml, &[0; 32])).unwrap();
            let image = reader.header().images()[0];
            assert!(
                reader.stored_block(&image.data).is_err(),
                "{location} should have been refused"
            );
        }
    }

    #[test]
    fn a_path_block_may_not_escape_its_directory() {
        let xml = r#"<xisf version="1.0"><Image location="path:../secret"/></xisf>"#;
        let reader = Reader::from_bytes(build(xml, &[])).unwrap();
        let image = reader.header().images()[0];
        let err = reader.stored_block(&image.data).unwrap_err();
        // Without a path on the reader it cannot resolve at all; with one it
        // would be refused for climbing out. Either way it is not read.
        assert!(matches!(err.kind(), ErrorKind::NotFound | ErrorKind::BadAttribute));
    }

    #[test]
    fn checksums_are_over_the_stored_bytes() {
        let digest = digest_hex(&digest(b"ABC", ChecksumAlgorithm::Sha256).unwrap());
        let xml = format!(
            r#"<xisf version="1.0"><Image location="inline:base64" checksum="sha-256:{digest}">QUJD</Image></xisf>"#
        );
        let reader = Reader::from_bytes(build(&xml, &[])).unwrap();
        let image = reader.header().images()[0];
        assert_eq!(reader.verify(&image.data).unwrap(), ChecksumStatus::Valid);
    }

    #[test]
    fn a_wrong_checksum_is_reported_not_ignored() {
        let xml = format!(
            r#"<xisf version="1.0"><Image location="inline:base64" checksum="sha-256:{}">QUJD</Image></xisf>"#,
            "00".repeat(32)
        );
        let reader = Reader::from_bytes(build(&xml, &[])).unwrap();
        let image = reader.header().images()[0];
        assert_eq!(reader.verify(&image.data).unwrap(), ChecksumStatus::Invalid);
    }

    #[test]
    fn no_checksum_is_not_a_failure() {
        let xml = r#"<xisf version="1.0"><Image location="inline:base64">QUJD</Image></xisf>"#;
        let reader = Reader::from_bytes(build(xml, &[])).unwrap();
        let image = reader.header().images()[0];
        assert_eq!(reader.verify(&image.data).unwrap(), ChecksumStatus::Absent);
    }
}

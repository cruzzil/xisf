//! Opening a monolithic XISF file and getting data blocks out of it.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use base64::Engine as _;

use crate::block::{Checksum, ChecksumAlgorithm, HEADER_DIR_TOKEN, Location, TextEncoding};
use crate::codec;
use crate::err;
use crate::error::Result;
use crate::header::{self, DataRef, Header};
use crate::layout::{self, Layout};

/// How a caller fetches a `url(...)` block. See
/// [`Reader::set_url_resolver`].
pub type UrlResolver = Box<dyn Fn(&str) -> Result<Vec<u8>> + Send + Sync>;

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
    /// Absent under Miri, which cannot execute `mmap`: `Reader::open` reads
    /// the file whole there, so nothing would construct this and `dead_code`
    /// would fire.
    #[cfg(not(miri))]
    Mapped(memmap2::Mmap),
    Owned(Vec<u8>),
}

impl std::ops::Deref for Source {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            #[cfg(not(miri))]
            Source::Mapped(m) => m,
            Source::Owned(v) => v,
        }
    }
}

impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self {
            #[cfg(not(miri))]
            Source::Mapped(_) => "Mapped",
            Source::Owned(_) => "Owned",
        };
        write!(f, "{kind}({} bytes)", self.len())
    }
}

/// An open XISF file.
pub struct Reader {
    source: Source,
    layout: Layout,
    header: Header,
    /// See [`Reader::follow_absolute_paths`].
    allow_absolute_paths: bool,
    /// See [`Reader::set_url_resolver`].
    url_resolver: Option<UrlResolver>,
    /// See [`Reader::set_verify_checksums`]. On unless a caller turns it off.
    verify_checksums: bool,
    /// Where the file came from, so a `path:` block can be resolved relative
    /// to it, as the spec requires.
    path: Option<PathBuf>,
}

impl std::fmt::Debug for Reader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reader")
            .field("source", &self.source)
            .field("path", &self.path)
            .field("images", &self.header.images().len())
            .field("url_resolver", &self.url_resolver.is_some())
            .finish_non_exhaustive()
    }
}

impl Reader {
    /// Open and scan a file from disk.
    ///
    /// The file is memory-mapped, so an attached block costs nothing until it
    /// is read.
    ///
    /// # Mapped files and concurrent modification
    ///
    /// Because the file stays mapped for as long as the reader lives, another
    /// process that *truncates* it out from under us turns a later block read
    /// into `SIGBUS`, which no amount of checking in this crate can catch:
    /// the bytes were valid when they were checked and stopped existing
    /// afterwards. This is inherent to memory mapping rather than specific to
    /// this crate, and it is the reason [`Reader::from_bytes`] exists -- a
    /// caller reading files that others may be rewriting should read the
    /// bytes themselves and hand them over, which costs a copy and removes
    /// the hazard entirely.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();

        // Miri interprets rather than executes, so it has no `mmap`. Reading
        // the file whole is observably the same to every caller -- `Source`
        // hands out a `&[u8]` either way -- and it is what lets this crate be
        // checked under Miri at all.
        #[cfg(miri)]
        {
            let bytes = std::fs::read(path)?;
            let (layout, header) = Self::scan(&bytes)?;
            return Ok(Self {
                source: Source::Owned(bytes),
                layout,
                header,
                path: Some(path.into()),
                allow_absolute_paths: false,
                verify_checksums: true,
                url_resolver: None,
            });
        }

        #[cfg(not(miri))]
        {
            let file = std::fs::File::open(path)?;

            // SAFETY: mapping is unsafe because another process truncating
            // the file turns a later read into SIGBUS. That hazard is
            // inherent to memory mapping; XISF files are written whole rather
            // than modified in place, and mapping is what lets a
            // multi-gigabyte image be read without loading it all.
            let mapped = unsafe { memmap2::Mmap::map(&file) }?;
            let (layout, header) = Self::scan(&mapped)?;
            Ok(Self {
                source: Source::Mapped(mapped),
                layout,
                header,
                path: Some(path.into()),
                allow_absolute_paths: false,
                verify_checksums: true,
                url_resolver: None,
            })
        }
    }

    /// Scan a file already in memory.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        let (layout, header) = Self::scan(&bytes)?;
        Ok(Self {
            source: Source::Owned(bytes),
            layout,
            header,
            path: None,
            allow_absolute_paths: false,
            verify_checksums: true,
            url_resolver: None,
        })
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
        let stored = self.stored_block_unchecked(data)?;

        // A file that records a checksum is telling the reader how to know
        // whether these bytes are the bytes that were written. Handing them
        // over without looking, when the file itself said how to check, is a
        // silent corruption the format went out of its way to make
        // detectable -- so the check is on unless a caller turns it off.
        if self.verify_checksums
            && let Some(checksum) = &data.checksum
            && digest(&stored, checksum.algorithm)? != checksum.digest
        {
            return Err(err!(
                ChecksumMismatch,
                "the block's {} checksum does not match its contents",
                checksum.algorithm.name()
            ));
        }
        Ok(stored)
    }

    /// The stored bytes without checking the recorded checksum.
    ///
    /// This is what [`Reader::verify`] reads, since it reports a mismatch
    /// rather than failing on one.
    fn stored_block_unchecked(&self, data: &DataRef) -> Result<Cow<'_, [u8]>> {
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

            Location::Path { path, index } => {
                Ok(Cow::Owned(read_external_block(&self.resolve_relative(path)?, path, *index)?))
            }
            Location::Url { url, index } => {
                let Some(fetch) = &self.url_resolver else {
                    return Err(err!(
                        Unsupported,
                        "{url:?} is a remote block; install a resolver with \
                         `set_url_resolver` to allow fetching"
                    ));
                };
                let bytes = fetch(url)?;
                Ok(Cow::Owned(extract_external(bytes, *index, url)?))
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
        let stored = self.stored_block_unchecked(data)?;
        Ok(if digest(&stored, checksum.algorithm)? == checksum.digest {
            ChecksumStatus::Valid
        } else {
            ChecksumStatus::Invalid
        })
    }

    /// Whether to check a block against its recorded checksum when reading it.
    ///
    /// On by default, because the specification requires a decoder to check a
    /// digest the file went to the trouble of recording, and because handing
    /// back corrupt bytes without a word is the worse failure.
    ///
    /// It costs one hash pass over the block: about 15 ms for a 32 MB frame
    /// with sha-1 or sha-256, and about 55 ms with sha-512 -- roughly twice
    /// what the typed read that usually follows it costs, and small against
    /// the disk read that must precede it. See `docs/PERFORMANCE.md`.
    ///
    /// Turning it off is for a caller who reads the same block repeatedly, or
    /// who has verified the file by other means; [`Reader::verify`] then
    /// remains available on demand, reporting a mismatch rather than failing.
    ///
    /// Blocks with no recorded checksum are unaffected: the spec makes them
    /// optional, and their absence is not a failure.
    pub fn set_verify_checksums(&mut self, verify: bool) {
        self.verify_checksums = verify;
    }

    /// Install a resolver for `url(...)` blocks.
    ///
    /// **This library never makes a network request itself, and that is a
    /// deliberate design decision rather than an omission.**
    ///
    /// A `url(...)` locator is a request, written in a file, for the reader to
    /// go and fetch something. Honouring it automatically would mean that
    /// `Reader::open` on a local file could reach out to a host the caller
    /// never named -- which is a server-side request forgery primitive when
    /// the caller is a service, and reaches cloud metadata endpoints and
    /// internal addresses that are not otherwise exposed. It would also make
    /// timeouts, proxies, redirects and certificate trust this crate's
    /// problem, and pull in an HTTP and TLS stack that the rest of the
    /// workspace does without.
    ///
    /// Handing the decision to the caller solves all of it at once. The
    /// resolver receives the URL exactly as the file wrote it and returns the
    /// bytes, so a caller can allow only the schemes and hosts they mean to,
    /// apply their own timeouts, or refuse. Without one, a remote block is an
    /// `Unsupported` error that says so.
    ///
    /// ```no_run
    /// # use xisf_core::Reader;
    /// # fn main() -> xisf_core::Result<()> {
    /// let mut reader = Reader::open("unit.xisf")?;
    /// reader.set_url_resolver(|url| {
    ///     if !url.starts_with("https://data.example.com/") {
    ///         return Err(xisf_core::Error::new(
    ///             xisf_core::ErrorKind::Unsupported,
    ///             format!("refusing to fetch {url}"),
    ///         ));
    ///     }
    ///     // ... fetch and return the bytes ...
    ///     # unimplemented!()
    /// });
    /// # Ok(())
    /// # }
    /// ```
    pub fn set_url_resolver(
        &mut self,
        resolver: impl Fn(&str) -> Result<Vec<u8>> + Send + Sync + 'static,
    ) {
        self.url_resolver = Some(Box::new(resolver));
    }

    /// Remove any installed resolver, so remote blocks are refused again.
    pub fn clear_url_resolver(&mut self) {
        self.url_resolver = None;
    }

    /// Whether to follow `path(...)` locators that name an absolute path.
    ///
    /// Off by default, and that is a deliberate departure from the letter of
    /// the specification, which defines the field as an *absolute* path. A
    /// header is untrusted input: a file that says
    /// `location="path(/etc/shadow)"` is asking a library to read a file the
    /// user never named, and returning its bytes as pixel data is an
    /// arbitrary-file-read primitive dressed as an image decoder.
    ///
    /// Relative locators still resolve beside the referring file, which is
    /// what a distributed unit actually needs and what the corpus uses. A
    /// caller who genuinely wants absolute paths -- a trusted pipeline, say --
    /// turns them on and takes that decision knowingly.
    pub fn follow_absolute_paths(&mut self, allow: bool) {
        self.allow_absolute_paths = allow;
    }

    /// Resolve a `path(...)` locator to a file to read.
    ///
    /// A relative locator is resolved beside the referring file and may not
    /// climb out of its directory; an absolute one is refused unless
    /// [`follow_absolute_paths`](Reader::follow_absolute_paths) says otherwise.
    fn resolve_relative(&self, locator: &str) -> Result<PathBuf> {
        // `@header_dir` is the specification's own spelling for "beside the
        // file that named this", and it is the only relative form the
        // grammar actually defines -- `path(@header_dir/blocks.xisb)`. It is
        // stripped here rather than anywhere later so that everything after
        // it, including the containment check below, sees an ordinary
        // relative path and `@header_dir/../..` is refused like any other
        // attempt to climb out.
        if let Some(rest) = locator.strip_prefix(HEADER_DIR_TOKEN) {
            let rest = rest.strip_prefix('/').unwrap_or(rest);
            if rest.is_empty() {
                return Err(err!(BadAttribute, "{locator:?} names a directory, not a file"));
            }
            return self.resolve_beside(rest);
        }

        let candidate = Path::new(locator);
        if candidate.is_absolute() {
            if !self.allow_absolute_paths {
                return Err(err!(
                    BadAttribute,
                    "{locator:?} is an absolute path; following one from a file header is \
                     refused unless the caller opts in"
                ));
            }
            return Ok(candidate.to_path_buf());
        }
        self.resolve_beside(locator)
    }

    /// Resolve a locator relative to the referring file's own directory.
    ///
    /// A locator with no `@header_dir` prefix and no leading slash is
    /// accepted here too. The grammar does not define that form -- a `path()`
    /// is either absolute or `@header_dir`-prefixed -- but files written that
    /// way exist, and reading one beside its header is what its author
    /// plainly meant and is no more permissive than the documented form.
    fn resolve_beside(&self, locator: &str) -> Result<PathBuf> {
        let base = self.path.as_ref().and_then(|p| p.parent()).ok_or_else(|| {
            err!(NotFound, "this file has no directory to resolve {locator:?} in")
        })?;

        let candidate = Path::new(locator);
        if locator.contains("://") {
            return Err(err!(BadAttribute, "{locator:?} is a URL, not a path"));
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
        let joined = base.join(candidate);

        // Checking the *components* is not enough. A locator with no `..` in
        // it at all -- a plain `blocks.xisb` -- can be a symbolic link to
        // anywhere, and following it reads a file the locator never named.
        // Whoever supplies a malicious unit generally supplies the directory
        // it unpacks into, so planting the link beside the header is exactly
        // as easy as writing the header. Resolving both ends and checking
        // containment is what actually holds.
        let resolved = joined
            .canonicalize()
            .map_err(|e| err!(NotFound, "{locator:?} could not be resolved: {e}"))?;
        let root = base
            .canonicalize()
            .map_err(|e| err!(NotFound, "the referring file's directory is unreadable: {e}"))?;
        if !resolved.starts_with(&root) {
            return Err(err!(
                BadAttribute,
                "{locator:?} resolves to {}, outside the referring file's directory",
                resolved.display()
            ));
        }
        Ok(resolved)
    }
}

/// Read an external block file, refusing anything that is not a regular file.
///
/// A named pipe or a character device passes every path check and then never
/// ends: reading `/dev/zero` exhausts memory, reading a fifo blocks forever.
/// Neither is a data blocks file, and a decoder has no reason to open one.
fn read_external_block(path: &Path, locator: &str, index: Option<u64>) -> Result<Vec<u8>> {
    let metadata = std::fs::metadata(path)
        .map_err(|e| err!(NotFound, "{locator:?} could not be read: {e}"))?;
    if !metadata.is_file() {
        return Err(err!(
            BadAttribute,
            "{locator:?} is not a regular file, so it is not a data block"
        ));
    }

    // Which of the two shapes this is decides how much of it has to be read,
    // so the signature is looked at before anything larger is committed to.
    let mut file = std::fs::File::open(path).map_err(|e| err!(NotFound, "{locator:?}: {e}"))?;
    let mut signature = [0u8; 8];
    let is_blocks_file = match std::io::Read::read_exact(&mut file, &mut signature) {
        Ok(()) => &signature == crate::distributed::BLOCKS_SIGNATURE,
        // Too short to be a blocks file, so it can only be a plain resource.
        Err(_) => false,
    };

    if !is_blocks_file {
        if index.is_some() {
            return Err(err!(
                BadAttribute,
                "{locator:?} names a block by identifier but is not an XISF data blocks file"
            ));
        }
        // A plain external resource is its own block, so its whole contents
        // are wanted and its size is not something to second-guess.
        return Ok(std::fs::read(path)?);
    }

    let id = index.ok_or_else(|| {
        err!(BadAttribute, "{locator:?} is a data blocks file, so the locator must name a block")
    })?;
    crate::distributed::read_block_from(&mut file, metadata.len(), id)?
        .ok_or_else(|| err!(NotFound, "{locator:?} has no block with identifier {id}"))
}

/// Take one block out of an external file.
///
/// Two shapes are legal here and they are told apart by the file itself. An
/// XISF *data blocks file* begins with `XISB0100` and holds an index naming
/// each block by identifier; the locator's trailing number is that
/// identifier, not an ordinal. Any other file is a plain external resource
/// whose whole contents are the block.
///
/// Reading a blocks file as though it were plain would hand the caller the
/// signature and index as though they were pixels, which is the sort of thing
/// that produces an image of noise rather than an error.
fn extract_external(bytes: Vec<u8>, index: Option<u64>, path: &str) -> Result<Vec<u8>> {
    let is_blocks_file = bytes.len() >= crate::distributed::BLOCKS_SIGNATURE.len()
        && &bytes[..8] == crate::distributed::BLOCKS_SIGNATURE;

    if !is_blocks_file {
        if index.is_some() {
            return Err(err!(
                BadAttribute,
                "{path:?} names a block by identifier but is not an XISF data blocks file"
            ));
        }
        return Ok(bytes);
    }

    let id = index.ok_or_else(|| {
        err!(BadAttribute, "{path:?} is a data blocks file, so the locator must name a block")
    })?;
    let parsed = crate::distributed::parse_blocks_file(&bytes)?;
    let element = parsed
        .get(id)
        .ok_or_else(|| err!(NotFound, "{path:?} has no block with identifier {id}"))?;
    if element.is_free() {
        return Err(err!(NotFound, "block {id} in {path:?} is a free placeholder with no data"));
    }

    let start = usize::try_from(element.position)
        .map_err(|_| err!(Unsupported, "block {id} lies beyond this platform's range"))?;
    let len = usize::try_from(element.length)
        .map_err(|_| err!(Unsupported, "block {id} is too large for this platform"))?;
    let end = start
        .checked_add(len)
        .ok_or_else(|| err!(Truncated, "block {id}'s position and length overflow"))?;
    if end > bytes.len() {
        return Err(err!(
            Truncated,
            "block {id} in {path:?} runs {} bytes past the end of the file",
            end - bytes.len()
        ));
    }
    Ok(bytes[start..end].to_vec())
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
        // The offset is absolute in the file, so it has to clear the preamble
        // and the header rather than being counted from the data.
        const AT: usize = 256;
        let xml = format!(r#"<xisf version="1.0"><Image location="attachment:{AT}:3"/></xisf>"#);
        let padding = AT - (crate::layout::PREAMBLE_LEN + xml.len());
        let mut trailing = vec![0u8; padding];
        trailing.extend_from_slice(b"XYZ");

        let reader = Reader::from_bytes(build(&xml, &trailing)).unwrap();
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

    /// A header is untrusted input, so neither climbing out of the referring
    /// file's directory nor naming an absolute path is followed by default.
    #[test]
    fn external_paths_are_not_followed_where_they_should_not_be() {
        for locator in ["path(../secret)", "path(/etc/passwd)", "path(a/../../b)"] {
            let xml = format!(r#"<xisf version="1.0"><Image location="{locator}"/></xisf>"#);
            let reader = Reader::from_bytes(build(&xml, &[])).unwrap();
            let image = reader.header().images()[0];
            let err = reader.stored_block(&image.data).unwrap_err();
            assert!(
                matches!(err.kind(), ErrorKind::NotFound | ErrorKind::BadAttribute),
                "{locator} produced {err}"
            );
        }
    }

    /// Turning absolute paths on is a decision the caller makes knowingly.
    #[test]
    fn absolute_paths_are_followed_only_when_asked_for() {
        let dir = std::env::temp_dir().join(format!("xisf-abs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("block.bin");
        std::fs::write(&target, b"payload").unwrap();

        let xml = format!(
            r#"<xisf version="1.0"><Image location="path({})"/></xisf>"#,
            target.display().to_string().replace('\\', "/")
        );
        let mut reader = Reader::from_bytes(build(&xml, &[])).unwrap();

        let data = reader.header().images()[0].data.clone();
        assert_eq!(
            reader.stored_block(&data).unwrap_err().kind(),
            ErrorKind::BadAttribute,
            "an absolute path was followed without being asked for"
        );

        reader.follow_absolute_paths(true);
        assert_eq!(&*reader.stored_block(&data).unwrap(), b"payload");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // Needs a hash implementation compiled in; without the feature the
    // library correctly refuses rather than pretending to verify.
    /// A remote block is refused until the caller says how to fetch it, and
    /// the resolver sees the URL exactly as the file wrote it -- which is
    /// what lets a caller apply a policy rather than trusting the file.
    #[test]
    fn url_blocks_need_a_resolver_the_caller_installs() {
        let xml =
            r#"<xisf version="1.0"><Image location="url(https://example.com/b.dat)"/></xisf>"#;
        let mut reader = Reader::from_bytes(build(xml, &[])).unwrap();
        let data = reader.header().images()[0].data.clone();

        let err = reader.stored_block(&data).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Unsupported);
        assert!(err.message().contains("set_url_resolver"), "{}", err.message());

        // With a resolver, the URL arrives verbatim.
        reader.set_url_resolver(|url| {
            assert_eq!(url, "https://example.com/b.dat");
            Ok(b"fetched".to_vec())
        });
        assert_eq!(&*reader.stored_block(&data).unwrap(), b"fetched");

        // And a resolver may refuse, which must surface rather than be eaten.
        reader.set_url_resolver(|url| {
            Err(crate::Error::new(ErrorKind::Unsupported, format!("refusing {url}")))
        });
        assert!(reader.stored_block(&data).unwrap_err().message().contains("refusing"));

        reader.clear_url_resolver();
        assert_eq!(reader.stored_block(&data).unwrap_err().kind(), ErrorKind::Unsupported);
    }

    /// A remote data blocks file is addressed by identifier exactly as a
    /// local one is, so the resolver only has to supply bytes.
    #[test]
    fn a_remote_blocks_file_is_addressed_by_identifier() {
        let blocks = crate::distributed::write_blocks_file(&[
            (1, b"first".to_vec()),
            (0x2a, b"the wanted one".to_vec()),
        ])
        .unwrap();

        let xml = r#"<xisf version="1.0"><Image location="url(https://h/b.xisb):0x2a"/></xisf>"#;
        let mut reader = Reader::from_bytes(build(xml, &[])).unwrap();
        reader.set_url_resolver(move |_| Ok(blocks.clone()));

        let data = reader.header().images()[0].data.clone();
        assert_eq!(&*reader.stored_block(&data).unwrap(), b"the wanted one");
    }

    #[cfg(feature = "checksums")]
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

    // Needs a hash implementation compiled in; without the feature the
    // library correctly refuses rather than pretending to verify.
    #[cfg(feature = "checksums")]
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

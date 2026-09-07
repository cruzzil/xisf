//! The attributes that say where a data block is, and what has been done to it.
//!
//! Three attributes travel together on every element that carries data, and
//! each has its own small grammar. They are parsed here rather than at the
//! call sites, because getting them wrong is how a reader ends up addressing
//! bytes that are not there.
/// The token a relative `path(...)` locator begins with.
///
/// The specification defines exactly two `path()` forms: an absolute path,
/// and `path(@header_dir/rel-path)` for one relative to the directory holding
/// the header. The token is literal -- it is not a directory named
/// `@header_dir`.
pub const HEADER_DIR_TOKEN: &str = "@header_dir";

use crate::err;
use crate::error::{ErrorKind, Result};

/// Where a data block's bytes are.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Location {
    /// In the element's own character data, text-encoded.
    Inline { encoding: TextEncoding },
    /// In a child `<Data>` element, base64-encoded.
    ///
    /// The spec gives this its own form rather than folding it into `inline`
    /// because an element that may have child elements cannot also carry
    /// character data unambiguously.
    Embedded,
    /// A byte range of this file, after the header.
    Attachment { position: u64, size: u64 },
    /// Another file, named by a path relative to this one.
    Path { path: String, index: Option<u64> },
    /// Another resource, named by a URL.
    Url { url: String, index: Option<u64> },
}

/// How a text-encoded block is spelled.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TextEncoding {
    /// RFC 4648 Base64.
    Base64,
    /// RFC 4648 Base16, which the spec calls `hex`.
    Hex,
}

impl Location {
    /// Parse a `location` attribute.
    ///
    /// ```text
    /// location="inline:base64"                 (or :hex)
    /// location="embedded"
    /// location="attachment:1234:5678"          position:size
    /// location="path(/data/blocks.xisb)"       optionally ):0x7a73...
    /// location="url(http://host/f.xisb)"       optionally ):0x7a73...
    /// ```
    ///
    /// Note the shapes differ: `inline` and `attachment` take colon-separated
    /// fields, while `path` and `url` **parenthesise** their locator. That is
    /// what lets a URL contain colons and slashes without ambiguity, and it
    /// means a parser that assumed one form throughout gets the other wrong.
    /// A literal parenthesis inside a URL is XML-escaped by the writer, so it
    /// has already been unescaped by the time it arrives here -- which is why
    /// the closing parenthesis is found from the *end*.
    ///
    /// The optional trailing identifier names a block within an XISF data
    /// blocks file. The specification says it should be written in
    /// hexadecimal, and real files use `0x`-prefixed values, but it defines
    /// the field as an unsigned integer, so plain decimal is accepted too.
    pub fn parse(text: &str) -> Result<Self> {
        let text = text.trim();
        if let Some(location) = Self::parse_parenthesised(text)? {
            return Ok(location);
        }

        let (scheme, rest) = match text.split_once(':') {
            Some((scheme, rest)) => (scheme, Some(rest)),
            None => (text, None),
        };

        match scheme {
            "embedded" => Ok(Location::Embedded),
            "inline" => {
                let encoding = match rest {
                    Some("base64") => TextEncoding::Base64,
                    Some("hex") => TextEncoding::Hex,
                    other => {
                        return Err(err!(
                            BadAttribute,
                            "inline encoding must be base64 or hex, got {:?}",
                            other.unwrap_or("")
                        ));
                    }
                };
                Ok(Location::Inline { encoding })
            }
            "attachment" => {
                let rest = rest.unwrap_or("");
                let (position, size) = rest.split_once(':').ok_or_else(|| {
                    err!(BadAttribute, "attachment needs position:size, got {rest:?}")
                })?;
                Ok(Location::Attachment {
                    position: parse_u64(position, "attachment position")?,
                    size: parse_u64(size, "attachment size")?,
                })
            }
            other => Err(err!(BadAttribute, "unknown location scheme {other:?}")),
        }
    }

    /// Parse the parenthesised `path(...)` and `url(...)` forms.
    fn parse_parenthesised(text: &str) -> Result<Option<Self>> {
        let (scheme, rest) = match text.strip_prefix("path(") {
            Some(rest) => ("path", rest),
            None => match text.strip_prefix("url(") {
                Some(rest) => ("url", rest),
                None => return Ok(None),
            },
        };

        // From the end, so a locator containing its own parentheses -- legal,
        // once XML-unescaped -- does not truncate here.
        let close = rest.rfind(')').ok_or_else(|| {
            err!(BadAttribute, "{scheme}(...) is missing its closing parenthesis")
        })?;
        let locator = rest[..close].trim();
        if locator.is_empty() {
            return Err(err!(BadAttribute, "{scheme}(...) has an empty locator"));
        }

        let index = match rest[close + 1..].trim() {
            "" => None,
            tail => {
                let digits = tail
                    .strip_prefix(':')
                    .ok_or_else(|| err!(BadAttribute, "unexpected {tail:?} after {scheme}(...)"))?;
                Some(parse_block_id(digits.trim())?)
            }
        };

        Ok(Some(if scheme == "path" {
            Location::Path { path: locator.to_string(), index }
        } else {
            Location::Url { url: locator.to_string(), index }
        }))
    }

    /// Whether the bytes live outside the file that named them.
    pub fn is_external(&self) -> bool {
        matches!(self, Location::Path { .. } | Location::Url { .. })
    }
}

/// The byte order a data block's multi-byte values are stored in.
///
/// The spec makes the attribute optional and little-endian the default, so a
/// block with no `byteOrder` is little-endian regardless of the host. It also
/// says the attribute is unnecessary for blocks with no multi-byte structure
/// -- byte vectors, UTF-8 strings, 8-bit images -- and must never appear on an
/// `ICCProfile`, whose own specification fixes it as big-endian.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ByteOrder {
    #[default]
    Little,
    Big,
}

impl ByteOrder {
    pub fn parse(name: &str) -> Result<Self> {
        Ok(match name {
            "little" => ByteOrder::Little,
            "big" => ByteOrder::Big,
            other => return Err(err!(BadAttribute, "unknown byteOrder {other:?}")),
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            ByteOrder::Little => "little",
            ByteOrder::Big => "big",
        }
    }

    /// Whether this is the order this machine uses, in which case multi-byte
    /// values need no swapping.
    pub fn is_native(self) -> bool {
        (self == ByteOrder::Little) == cfg!(target_endian = "little")
    }
}

/// A compression codec.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Codec {
    /// RFC 1950 zlib.
    Zlib,
    /// LZ4 block format.
    Lz4,
    /// LZ4 block format, high-compression encoder. Decodes as [`Codec::Lz4`].
    Lz4Hc,
    /// Zstandard. Not among XISF 1.0's standard codecs, but libXISF writes
    /// it, so it is read here.
    Zstd,
    /// A codec this build cannot decode, kept by name.
    ///
    /// XISF 1.0 names zlib, LZ4 and LZ4HC as its standard codecs, but an
    /// encoder may write others -- libXISF writes ZSTD. A file containing one
    /// such block is still a readable file: its header parses, its other
    /// blocks decode, and only *this* block fails, at the point something
    /// asks for its bytes. Refusing the whole file at parse time would make
    /// its metadata unreachable for no reason.
    Other(String),
}

impl Codec {
    pub fn name(&self) -> &str {
        match self {
            Codec::Zlib => "zlib",
            Codec::Lz4 => "lz4",
            Codec::Lz4Hc => "lz4hc",
            Codec::Zstd => "zstd",
            Codec::Other(name) => name,
        }
    }

    /// Whether this build can decode the codec.
    pub fn is_supported(&self) -> bool {
        !matches!(self, Codec::Other(_))
    }
}

/// A parsed `compression` attribute.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Compression {
    pub codec: Codec,
    /// The size the block decompresses to. The spec requires it, so a decoder
    /// never has to guess how much to allocate.
    pub uncompressed_size: u64,
    /// Element size for byte unshuffling, or `None` when not shuffled.
    ///
    /// Shuffling stores all the first bytes of each element, then all the
    /// second bytes, and so on, which makes the codecs' job much easier on
    /// numeric data. An item size of 1 is a no-op and the spec forbids it as
    /// a shuffled form.
    pub shuffle_item_size: Option<u64>,
    /// Compression subblocks, as `(compressed, uncompressed)` byte counts.
    ///
    /// Empty means the block is one compressed stream, which is the ordinary
    /// case. When present, the stored bytes are that many *independent*
    /// streams laid end to end, each decompressing to its own length -- so a
    /// decoder that fed the whole buffer to one codec call would recover
    /// only the first subblock.
    ///
    /// They exist because codecs have input limits -- zlib cannot compress
    /// more than 4GiB at once -- and because splitting a block lets an
    /// encoder compress the pieces in parallel.
    pub subblocks: Vec<(u64, u64)>,
}

impl Compression {
    /// Parse a `subblocks` attribute: `c1,u1:c2,u2:...:cN,uN`.
    pub fn parse_subblocks(text: &str) -> Result<Vec<(u64, u64)>> {
        text.split(':')
            .map(|pair| {
                let (compressed, uncompressed) = pair.trim().split_once(',').ok_or_else(|| {
                    err!(BadAttribute, "a subblock must be `compressed,uncompressed`, got {pair:?}")
                })?;
                let number = |field: &str| -> Result<u64> {
                    field.trim().parse::<u64>().map_err(|_| {
                        err!(BadAttribute, "a subblock length must be an integer, got {field:?}")
                    })
                };
                Ok((number(compressed)?, number(uncompressed)?))
            })
            .collect()
    }

    /// Parse a `compression` attribute.
    ///
    /// ```text
    /// compression="zlib:30000"
    /// compression="zlib+sh:30000:4"
    /// ```
    pub fn parse(text: &str) -> Result<Self> {
        let mut fields = text.split(':');
        let method = fields.next().unwrap_or("");
        let (name, shuffled) = match method.strip_suffix("+sh") {
            Some(name) => (name, true),
            None => (method, false),
        };

        let codec = match name {
            "zlib" => Codec::Zlib,
            "lz4" => Codec::Lz4,
            "lz4hc" => Codec::Lz4Hc,
            "zstd" => Codec::Zstd,
            other => Codec::Other(other.to_string()),
        };

        let uncompressed_size = fields
            .next()
            .ok_or_else(|| err!(BadAttribute, "compression needs an uncompressed size"))
            .and_then(|s| parse_u64(s, "uncompressed size"))?;

        let shuffle_item_size = match (shuffled, fields.next()) {
            (true, Some(item)) => {
                let size = parse_u64(item, "shuffle item size")?;
                if size < 2 {
                    return Err(err!(
                        BadAttribute,
                        "a shuffled block needs an item size of 2 or more, got {size}"
                    ));
                }
                Some(size)
            }
            (true, None) => {
                return Err(err!(BadAttribute, "a shuffled block must state its item size"));
            }
            (false, _) => None,
        };

        Ok(Compression { codec, uncompressed_size, shuffle_item_size, subblocks: Vec::new() })
    }
}

/// A hash algorithm the spec allows for block checksums.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChecksumAlgorithm {
    Sha1,
    Sha256,
    Sha512,
    Sha3_256,
    Sha3_512,
}

impl ChecksumAlgorithm {
    /// The spec gives each algorithm a preferred spelling and an alternate
    /// one (`sha-1` and `sha1`); both are accepted.
    pub fn parse(name: &str) -> Result<Self> {
        Ok(match name {
            "sha-1" | "sha1" => ChecksumAlgorithm::Sha1,
            "sha-256" | "sha256" => ChecksumAlgorithm::Sha256,
            "sha-512" | "sha512" => ChecksumAlgorithm::Sha512,
            "sha3-256" => ChecksumAlgorithm::Sha3_256,
            "sha3-512" => ChecksumAlgorithm::Sha3_512,
            other => return Err(err!(Unsupported, "unknown checksum algorithm {other:?}")),
        })
    }

    /// The preferred spelling, which is what we emit.
    pub fn name(self) -> &'static str {
        match self {
            ChecksumAlgorithm::Sha1 => "sha-1",
            ChecksumAlgorithm::Sha256 => "sha-256",
            ChecksumAlgorithm::Sha512 => "sha-512",
            ChecksumAlgorithm::Sha3_256 => "sha3-256",
            ChecksumAlgorithm::Sha3_512 => "sha3-512",
        }
    }

    /// Digest length in bytes.
    pub fn digest_len(self) -> usize {
        match self {
            ChecksumAlgorithm::Sha1 => 20,
            ChecksumAlgorithm::Sha256 | ChecksumAlgorithm::Sha3_256 => 32,
            ChecksumAlgorithm::Sha512 | ChecksumAlgorithm::Sha3_512 => 64,
        }
    }
}

/// A parsed `checksum` attribute.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Checksum {
    pub algorithm: ChecksumAlgorithm,
    pub digest: Vec<u8>,
}

impl Checksum {
    /// Parse a `checksum` attribute, `algorithm:hexdigest`.
    ///
    /// The checksum is over the block as *stored* — compressed, if it is
    /// compressed — so it can be verified without decompressing.
    pub fn parse(text: &str) -> Result<Self> {
        let (name, digest) = text
            .split_once(':')
            .ok_or_else(|| err!(BadAttribute, "checksum needs algorithm:digest, got {text:?}"))?;
        let algorithm = ChecksumAlgorithm::parse(name)?;

        if digest.len() != algorithm.digest_len() * 2 {
            return Err(err!(
                BadAttribute,
                "{} needs {} hex digits, got {}",
                algorithm.name(),
                algorithm.digest_len() * 2,
                digest.len()
            ));
        }
        let bytes = (0..digest.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&digest[i..i + 2], 16))
            .collect::<core::result::Result<Vec<u8>, _>>()
            .map_err(|_| err!(BadAttribute, "checksum digest is not hexadecimal"))?;

        Ok(Checksum { algorithm, digest: bytes })
    }
}

/// Parse a block index identifier, which the specification says should be
/// hexadecimal but defines as an unsigned integer.
fn parse_block_id(text: &str) -> Result<u64> {
    let (digits, radix) = match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => (hex, 16),
        None => (text, 10),
    };
    if digits.is_empty() {
        return Err(err!(BadAttribute, "a block identifier is empty"));
    }
    u64::from_str_radix(digits, radix)
        .map_err(|_| err!(BadAttribute, "{text:?} is not a block identifier"))
}

fn parse_u64(text: &str, what: &str) -> Result<u64> {
    text.parse::<u64>().map_err(|_| {
        crate::error::Error::new(
            ErrorKind::BadAttribute,
            format!("{what} must be a decimal integer, got {text:?}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations_parse() {
        assert_eq!(
            Location::parse("inline:base64").unwrap(),
            Location::Inline { encoding: TextEncoding::Base64 }
        );
        assert_eq!(
            Location::parse("inline:hex").unwrap(),
            Location::Inline { encoding: TextEncoding::Hex }
        );
        assert_eq!(Location::parse("embedded").unwrap(), Location::Embedded);
        assert_eq!(
            Location::parse("attachment:9869:23042").unwrap(),
            Location::Attachment { position: 9869, size: 23042 }
        );
    }

    /// `path` and `url` parenthesise their locator, unlike `inline` and
    /// `attachment`, which are colon-separated. Assuming one form throughout
    /// gets the other wrong.
    #[test]
    fn path_and_url_are_parenthesised() {
        assert_eq!(
            Location::parse("url(file://host/blocks.xisb)").unwrap(),
            Location::Url { url: "file://host/blocks.xisb".into(), index: None }
        );
        assert_eq!(
            Location::parse("path(/data/blocks.xisb)").unwrap(),
            Location::Path { path: "/data/blocks.xisb".into(), index: None }
        );
        // A URL is full of colons and slashes; none of them ends the locator.
        assert_eq!(
            Location::parse("url(http://host:8080/a/b?c=d&e=f)").unwrap(),
            Location::Url { url: "http://host:8080/a/b?c=d&e=f".into(), index: None }
        );
    }

    /// The identifier is written in hexadecimal in practice, but defined as
    /// an unsigned integer, so both spellings are accepted.
    #[test]
    fn block_identifiers_may_be_hexadecimal_or_decimal() {
        assert_eq!(
            Location::parse("path(/d/b.xisb):0x7a73526b584c6167").unwrap(),
            Location::Path { path: "/d/b.xisb".into(), index: Some(0x7a73_526b_584c_6167) }
        );
        assert_eq!(
            Location::parse("url(file:///d/b.xisb):0").unwrap(),
            Location::Url { url: "file:///d/b.xisb".into(), index: Some(0) }
        );
        assert_eq!(
            Location::parse("path(/d/b.xisb):42").unwrap(),
            Location::Path { path: "/d/b.xisb".into(), index: Some(42) }
        );
    }

    /// A locator may contain parentheses of its own once XML-unescaped, which
    /// is why the closing one is found from the end.
    #[test]
    fn a_locator_may_contain_parentheses() {
        assert_eq!(
            Location::parse("url(ftp://ftp.example.com/public/example(2016).dat)").unwrap(),
            Location::Url {
                url: "ftp://ftp.example.com/public/example(2016).dat".into(),
                index: None
            }
        );
    }

    #[test]
    fn bad_locations_are_rejected() {
        for text in [
            "",
            "inline",
            "inline:utf8",
            "attachment",
            "attachment:1",
            "wat",
            "path(",  // no closing parenthesis
            "path()", // empty locator
            "url()",
            "path(/a):",    // an empty identifier
            "path(/a):zz",  // not a number
            "path(/a)junk", // trailing rubbish
        ] {
            assert!(Location::parse(text).is_err(), "{text:?} should not parse");
        }
    }

    #[test]
    fn byte_order_defaults_to_little_endian() {
        // The spec's default, which is a property of the format rather than
        // of the machine reading it.
        assert_eq!(ByteOrder::default(), ByteOrder::Little);
        assert_eq!(ByteOrder::parse("little").unwrap(), ByteOrder::Little);
        assert_eq!(ByteOrder::parse("big").unwrap(), ByteOrder::Big);
        assert!(ByteOrder::parse("network").is_err());
        assert!(ByteOrder::parse("Little").is_err(), "the spelling is lower case");
    }

    #[test]
    fn compression_parses_with_and_without_shuffling() {
        assert_eq!(
            Compression::parse("zlib:30000").unwrap(),
            Compression {
                codec: Codec::Zlib,
                uncompressed_size: 30000,
                shuffle_item_size: None,
                subblocks: Vec::new(),
            }
        );
        assert_eq!(
            Compression::parse("zlib+sh:30000:4").unwrap(),
            Compression {
                codec: Codec::Zlib,
                uncompressed_size: 30000,
                shuffle_item_size: Some(4),
                subblocks: Vec::new(),
            }
        );
        assert_eq!(Compression::parse("lz4hc+sh:100:2").unwrap().codec, Codec::Lz4Hc);
    }

    #[test]
    fn a_shuffled_block_must_say_how_wide_its_items_are() {
        assert!(Compression::parse("zlib+sh:30000").is_err());
        // An item size of 1 shuffles nothing, so the spec does not allow it.
        assert!(Compression::parse("zlib+sh:30000:1").is_err());
    }

    #[test]
    fn checksums_parse_both_spellings() {
        let digest = "d60477d1c651b6bc42a8aa938a6132006257d76f229a2d7481db84d23dea8b96";
        let c = Checksum::parse(&format!("sha256:{digest}")).unwrap();
        assert_eq!(c.algorithm, ChecksumAlgorithm::Sha256);
        assert_eq!(c.digest.len(), 32);
        assert_eq!(c.digest[0], 0xd6);
        assert_eq!(Checksum::parse(&format!("sha-256:{digest}")).unwrap(), c);
    }

    #[test]
    fn a_digest_of_the_wrong_length_is_rejected() {
        assert!(Checksum::parse("sha256:abcd").is_err());
        assert!(Checksum::parse("sha256:zz").is_err());
        assert!(Checksum::parse("md5:00112233445566778899aabbccddeeff").is_err());
    }
}

//! Writing monolithic XISF files.
//!
//! # The positioning problem
//!
//! An attached block is addressed by `location="attachment:position:size"`,
//! where the position is an *absolute* file offset. But the header sits before
//! the blocks, so a block's position depends on how long the header is -- and
//! the header's length depends on how many digits those positions take. A
//! naive encoder either leaves a gap or writes a position that is wrong by the
//! width of a number.
//!
//! Both PixInsight and libXISF place the first block immediately after the
//! header with no padding, so the addressing here does the same, by iterating
//! to a fixed point: render the header assuming a length, see what length it
//! actually came out, and render again if it changed. It converges because a
//! longer header can only push positions further out, which can only add
//! digits, and adding digits only lengthens the header -- a monotonic climb
//! that stops as soon as no number gains a digit. In practice that is one or
//! two rounds.

use crate::block::{ChecksumAlgorithm, Codec, Compression};
use crate::codec;
use crate::err;
use crate::error::Result;
use crate::image::Image;
use crate::layout::{PREAMBLE_LEN, SIGNATURE};
use crate::reader::{checksum_of, digest_hex};

/// How a block should be stored.
#[derive(Clone, Debug, Default)]
pub struct BlockOptions {
    /// Codec and shuffling to apply, or `None` to store the bytes as they are.
    pub compression: Option<CompressionRequest>,
    /// Hash to record, or `None` for no checksum.
    pub checksum: Option<ChecksumAlgorithm>,
}

/// A request to compress, before the compressed size is known.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CompressionRequest {
    pub codec: Codec2,
    /// Byte-shuffle in items of this width first. `None` for no shuffling.
    ///
    /// Shuffling only helps when the item width matches the data's element
    /// size, which is why this is chosen by the caller rather than guessed.
    pub shuffle_item_size: Option<u64>,
}

/// The codecs this crate can *write*.
///
/// Narrower than [`Codec`], deliberately. `ruzstd` decodes but does not
/// encode, so zstd can be read and not written; a separate type makes that a
/// compile-time fact rather than a runtime surprise.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Codec2 {
    Zlib,
    Lz4,
}

impl Codec2 {
    fn as_codec(self) -> Codec {
        match self {
            Codec2::Zlib => Codec::Zlib,
            Codec2::Lz4 => Codec::Lz4,
        }
    }
}

/// An image to write, with its pixel data.
#[derive(Clone, Debug)]
pub struct PendingImage {
    pub image: Image,
    pub data: Vec<u8>,
    pub options: BlockOptions,
}

/// Builds a monolithic XISF file.
#[derive(Clone, Debug, Default)]
pub struct Writer {
    images: Vec<PendingImage>,
    /// Extra `<Property>` elements for the `<Metadata>` block.
    metadata: Vec<(String, String, String)>,
    creator: Option<String>,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Name the software that produced the file. Written as
    /// `XISF:CreatorApplication`, which is the property the spec reserves.
    pub fn with_creator(mut self, creator: impl Into<String>) -> Self {
        self.creator = Some(creator.into());
        self
    }

    /// Add a string property to the file's `<Metadata>`.
    pub fn add_metadata(&mut self, id: impl Into<String>, value: impl Into<String>) {
        self.metadata.push((id.into(), "String".into(), value.into()));
    }

    /// Add an image, checking that its data matches the geometry it declares.
    pub fn add_image(&mut self, pending: PendingImage) -> Result<()> {
        let expected = pending
            .image
            .data_size()
            .ok_or_else(|| err!(InvalidArgument, "the image's geometry overflows"))?;
        if pending.data.len() as u64 != expected {
            return Err(err!(
                InvalidArgument,
                "the image declares {expected} bytes of pixel data but {} were given",
                pending.data.len()
            ));
        }
        self.images.push(pending);
        Ok(())
    }

    /// Serialise the whole file.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        // Each block is compressed once, up front: the stored bytes decide
        // both the sizes in the header and the checksums, so doing it inside
        // the positioning loop would be wasted work and, worse, would let a
        // non-deterministic codec change sizes between rounds.
        let stored: Vec<StoredBlock> =
            self.images.iter().map(StoredBlock::prepare).collect::<Result<_>>()?;

        // Fixed point on the header length; see the module comment.
        let mut assumed = self.render(&stored, PREAMBLE_LEN)?.len();
        let header = loop {
            let header = self.render(&stored, PREAMBLE_LEN + assumed)?;
            if header.len() == assumed {
                break header;
            }
            // Monotonic: a longer header only ever pushes positions outward.
            assumed = header.len();
        };

        let mut out = Vec::with_capacity(
            PREAMBLE_LEN + header.len() + stored.iter().map(|b| b.bytes.len()).sum::<usize>(),
        );
        out.extend_from_slice(SIGNATURE);
        out.extend_from_slice(
            &(u32::try_from(header.len()).map_err(|_| {
                err!(Unsupported, "the header is larger than the format's 32-bit length field")
            })?)
            .to_le_bytes(),
        );
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(header.as_bytes());
        for block in &stored {
            out.extend_from_slice(&block.bytes);
        }
        Ok(out)
    }

    /// Render the XML header, addressing blocks as if data begins at `data_at`.
    fn render(&self, stored: &[StoredBlock], data_at: usize) -> Result<String> {
        let mut xml = String::from(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
        xml.push_str(r#"<xisf version="1.0" xmlns="http://www.pixinsight.com/xisf" "#);
        xml.push_str(r#"xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" "#);
        xml.push_str(
            r#"xsi:schemaLocation="http://www.pixinsight.com/xisf http://pixinsight.com/xisf/xisf-1.0.xsd">"#,
        );

        let mut position = data_at;
        for (pending, block) in self.images.iter().zip(stored) {
            let image = &pending.image;
            let mut geometry: Vec<String> = image.dimensions.iter().map(u64::to_string).collect();
            geometry.push(image.channels.to_string());

            xml.push_str("<Image geometry=\"");
            xml.push_str(&geometry.join(":"));
            xml.push_str("\" sampleFormat=\"");
            xml.push_str(image.sample_format.name());
            xml.push_str("\" colorSpace=\"");
            xml.push_str(image.color_space.name());
            xml.push_str("\" pixelStorage=\"");
            xml.push_str(image.pixel_storage.name());
            xml.push('"');

            if let Some(bounds) = image.bounds {
                xml.push_str(&format!(" bounds=\"{}:{}\"", bounds.low, bounds.high));
            }
            if let Some(id) = &image.id {
                xml.push_str(&format!(" id=\"{}\"", escape_attr(id)));
            }
            if let Some(kind) = &image.image_type {
                xml.push_str(&format!(" imageType=\"{}\"", escape_attr(kind)));
            }
            if let Some(compression) = &block.compression {
                xml.push_str(&format!(" compression=\"{}\"", compression_attr(compression)));
            }
            if let Some(checksum) = &block.checksum {
                xml.push_str(&format!(
                    " checksum=\"{}:{}\"",
                    checksum.algorithm.name(),
                    digest_hex(&checksum.digest)
                ));
            }

            xml.push_str(&format!(" location=\"attachment:{position}:{}\"/>", block.bytes.len()));
            position += block.bytes.len();
        }

        // `<Metadata>` is required by the spec, and `XISF:CreationTime` and
        // `XISF:CreatorApplication` are the properties it reserves for this.
        xml.push_str("<Metadata>");
        for (id, kind, value) in &self.metadata {
            xml.push_str(&format!(
                "<Property id=\"{}\" type=\"{}\">{}</Property>",
                escape_attr(id),
                escape_attr(kind),
                escape_text(value)
            ));
        }
        if let Some(creator) = &self.creator {
            xml.push_str(&format!(
                "<Property id=\"XISF:CreatorApplication\" type=\"String\">{}</Property>",
                escape_text(creator)
            ));
        }
        xml.push_str("</Metadata>");

        xml.push_str("</xisf>");
        Ok(xml)
    }
}

/// A block after compression, ready to be addressed and written.
struct StoredBlock {
    bytes: Vec<u8>,
    compression: Option<Compression>,
    checksum: Option<crate::block::Checksum>,
}

impl StoredBlock {
    fn prepare(pending: &PendingImage) -> Result<Self> {
        let (bytes, compression) = match &pending.options.compression {
            None => (pending.data.clone(), None),
            Some(request) => {
                let uncompressed_size = pending.data.len() as u64;

                // Shuffle first, then compress: that is the order the spec
                // fixes, and the reader undoes it the other way round.
                // An item size of one shuffles nothing, and the spec does not
                // allow `+sh:...:1` as a stored form. Rather than making every
                // caller special-case eight-bit data -- the natural thing to
                // pass is the sample width, which is 1 for UInt8 -- it is
                // treated as a request for no shuffling.
                let item_size = request.shuffle_item_size.filter(|item| *item > 1);
                let staged = match item_size {
                    None => pending.data.clone(),
                    Some(item) => {
                        let item = usize::try_from(item)
                            .map_err(|_| err!(InvalidArgument, "shuffle item size is unusable"))?;
                        codec::shuffle(&pending.data, item)
                    }
                };

                let compressed = compress(&staged, request.codec)?;

                // Compression that makes a block bigger is not worth writing:
                // the file grows and every reader pays to undo it. Storing it
                // plain is always legal, since `compression` is optional.
                if compressed.len() >= pending.data.len() {
                    (pending.data.clone(), None)
                } else {
                    (
                        compressed,
                        Some(Compression {
                            codec: request.codec.as_codec(),
                            uncompressed_size,
                            shuffle_item_size: item_size,
                        }),
                    )
                }
            }
        };

        // The checksum covers the block as stored, so it is taken after
        // compression, matching what a reader can verify without decoding.
        let checksum = match pending.options.checksum {
            None => None,
            Some(algorithm) => Some(checksum_of(&bytes, algorithm)?),
        };

        Ok(StoredBlock { bytes, compression, checksum })
    }
}

fn compress(data: &[u8], codec: Codec2) -> Result<Vec<u8>> {
    match codec {
        #[cfg(feature = "zlib")]
        Codec2::Zlib => {
            use std::io::Write;
            let mut encoder =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(data).map_err(|e| err!(Compression, "zlib: {e}"))?;
            encoder.finish().map_err(|e| err!(Compression, "zlib: {e}"))
        }
        #[cfg(feature = "lz4")]
        Codec2::Lz4 => Ok(lz4_flex::block::compress(data)),
        #[allow(unreachable_patterns)]
        other => Err(err!(Unsupported, "cannot write {:?} blocks in this build", other)),
    }
}

/// Render a `compression` attribute.
fn compression_attr(compression: &Compression) -> String {
    match compression.shuffle_item_size {
        None => format!("{}:{}", compression.codec.name(), compression.uncompressed_size),
        Some(item) => {
            format!("{}+sh:{}:{}", compression.codec.name(), compression.uncompressed_size, item)
        }
    }
}

/// Escape text for an attribute value.
fn escape_attr(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// Escape character data. Quotes need no escaping here, but `]]>` would end a
/// CDATA section, so `>` is escaped along with the two that must be.
fn escape_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Reader;
    use crate::image::{ColorSpace, PixelStorage, SampleFormat};

    fn image(width: u64, height: u64, channels: u64, format: SampleFormat) -> Image {
        Image {
            dimensions: vec![width, height],
            channels,
            sample_format: format,
            color_space: if channels >= 3 { ColorSpace::Rgb } else { ColorSpace::Gray },
            pixel_storage: PixelStorage::Planar,
            bounds: None,
            id: None,
            uuid: None,
            image_type: None,
        }
    }

    fn sample(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 31 + 7) as u8).collect()
    }

    fn round_trip(options: BlockOptions, format: SampleFormat) -> Vec<u8> {
        let image = image(9, 7, 1, format);
        let data = sample(image.data_size().unwrap() as usize);

        let mut writer = Writer::new().with_creator("xisf-rs tests");
        writer.add_image(PendingImage { image, data: data.clone(), options }).expect("add_image");
        let bytes = writer.to_bytes().expect("to_bytes");

        let reader = Reader::from_bytes(bytes).expect("read back");
        let written = reader.header().images()[0];
        let read = reader.block(&written.data).expect("block").to_vec();
        assert_eq!(read, data, "the pixels changed on the way through");
        reader.bytes().to_vec()
    }

    #[test]
    fn writes_a_file_that_reads_back() {
        round_trip(BlockOptions::default(), SampleFormat::UInt8);
    }

    #[test]
    fn every_option_combination_round_trips() {
        for format in [SampleFormat::UInt8, SampleFormat::UInt16, SampleFormat::Float32] {
            for compression in [
                None,
                Some(CompressionRequest { codec: Codec2::Zlib, shuffle_item_size: None }),
                Some(CompressionRequest { codec: Codec2::Zlib, shuffle_item_size: Some(4) }),
                Some(CompressionRequest { codec: Codec2::Lz4, shuffle_item_size: None }),
                Some(CompressionRequest { codec: Codec2::Lz4, shuffle_item_size: Some(2) }),
            ] {
                for checksum in [None, Some(ChecksumAlgorithm::Sha256)] {
                    round_trip(BlockOptions { compression, checksum }, format);
                }
            }
        }
    }

    /// The positions in the header must land exactly on the bytes, with no
    /// gap and no overlap. This is the fixed point the module comment
    /// describes, and it is the thing most likely to be off by a few.
    #[test]
    fn block_positions_are_exact_and_contiguous() {
        let mut writer = Writer::new();
        for channels in [1u64, 3, 1] {
            let image = image(11, 13, channels, SampleFormat::UInt16);
            let data = sample(image.data_size().unwrap() as usize);
            writer
                .add_image(PendingImage { image, data, options: BlockOptions::default() })
                .unwrap();
        }
        let bytes = writer.to_bytes().unwrap();
        let reader = Reader::from_bytes(bytes.clone()).unwrap();

        let mut expected = crate::layout::scan(&bytes).unwrap().data_start() as u64;
        for element in reader.header().images() {
            let Some(crate::block::Location::Attachment { position, size }) = element.data.location
            else {
                panic!("expected an attachment");
            };
            assert_eq!(position, expected, "a block did not begin where the last one ended");
            expected += size;
        }
        assert_eq!(expected as usize, bytes.len(), "the file has trailing bytes");
    }

    #[test]
    fn checksums_written_are_checksums_that_verify() {
        let image = image(8, 8, 1, SampleFormat::UInt16);
        let data = sample(image.data_size().unwrap() as usize);
        let mut writer = Writer::new();
        writer
            .add_image(PendingImage {
                image,
                data,
                options: BlockOptions {
                    compression: Some(CompressionRequest {
                        codec: Codec2::Zlib,
                        shuffle_item_size: Some(2),
                    }),
                    checksum: Some(ChecksumAlgorithm::Sha512),
                },
            })
            .unwrap();

        let reader = Reader::from_bytes(writer.to_bytes().unwrap()).unwrap();
        let written = reader.header().images()[0];
        assert_eq!(
            reader.verify(&written.data).unwrap(),
            crate::ChecksumStatus::Valid,
            "a checksum we wrote ourselves did not verify"
        );
    }

    /// Compression that makes a block larger is dropped, since storing it
    /// plain is legal and strictly better.
    #[test]
    fn incompressible_data_is_stored_plain() {
        let image = image(4, 4, 1, SampleFormat::UInt8);
        // Bytes with no structure to exploit.
        let data: Vec<u8> = (0..16u32).map(|i| (i.wrapping_mul(2654435761) >> 24) as u8).collect();

        let mut writer = Writer::new();
        writer
            .add_image(PendingImage {
                image,
                data,
                options: BlockOptions {
                    compression: Some(CompressionRequest {
                        codec: Codec2::Zlib,
                        shuffle_item_size: None,
                    }),
                    checksum: None,
                },
            })
            .unwrap();

        let reader = Reader::from_bytes(writer.to_bytes().unwrap()).unwrap();
        assert!(
            reader.header().images()[0].data.compression.is_none(),
            "compression that did not help should not have been recorded"
        );
    }

    #[test]
    fn data_that_does_not_match_the_geometry_is_refused() {
        let mut writer = Writer::new();
        let err = writer
            .add_image(PendingImage {
                image: image(10, 10, 1, SampleFormat::UInt16),
                data: vec![0; 3],
                options: BlockOptions::default(),
            })
            .unwrap_err();
        assert_eq!(err.kind(), crate::ErrorKind::InvalidArgument);
    }

    #[test]
    fn special_characters_in_metadata_are_escaped() {
        let mut writer = Writer::new();
        writer.add_metadata("Test:Value", r#"a & b < c > d " e ' f"#);
        writer
            .add_image(PendingImage {
                image: image(2, 2, 1, SampleFormat::UInt8),
                data: sample(4),
                options: BlockOptions::default(),
            })
            .unwrap();

        // The proof is that it parses at all: unescaped `&` or `<` in
        // character data is not well-formed XML.
        let reader = Reader::from_bytes(writer.to_bytes().unwrap()).unwrap();
        let property = reader
            .header()
            .root
            .descendants()
            .into_iter()
            .find(|e| e.attr("id") == Some("Test:Value"))
            .expect("the property survived");
        assert_eq!(property.data.text.as_deref(), Some(r#"a & b < c > d " e ' f"#));
    }
}

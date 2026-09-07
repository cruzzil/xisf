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

use crate::block::{ChecksumAlgorithm, Codec, Compression, HEADER_DIR_TOKEN};
use crate::codec;
use crate::err;
use crate::error::Result;
use crate::image::{ColorFilterArray, DisplayFunction, Gamma, Image, Resolution, RgbWorkingSpace};
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

/// How the header names its blocks.
///
/// The two XISF forms differ in exactly this and nothing else, so the header
/// is rendered once and told how to address rather than written twice.
#[derive(Clone, Copy, Debug)]
enum Addressing<'a> {
    /// Monolithic: `attachment:position:size`, an absolute offset into the
    /// same file. Needs the position the data begins at, which is what makes
    /// the fixed-point loop necessary.
    Attached { data_at: usize },
    /// Distributed: `path(name.xisb):0xID`, naming a block in a separate data
    /// blocks file by its index identifier. No positions appear in the
    /// header, so nothing has to converge.
    Distributed { blocks_file: &'a str },
}

/// A distributed unit: an XISF header file and the data blocks file it names.
#[derive(Clone, Debug)]
pub struct DistributedUnit {
    /// The `.xish` header file's contents: the XML header and nothing else.
    pub header: Vec<u8>,
    /// The `.xisb` data blocks file's contents.
    pub blocks: Vec<u8>,
    /// The blocks file's name, as the header refers to it.
    pub blocks_file_name: String,
}

/// An image to write, with its pixel data and whatever is attached to it.
///
/// The ancillary elements are what a reader loses if a writer cannot produce
/// them: a read-modify-write cycle through a writer that only knows about
/// pixels drops the colour profile, the preview and the mosaic pattern
/// without saying so.
#[derive(Clone, Debug)]
pub struct PendingImage {
    pub image: Image,
    pub data: Vec<u8>,
    pub options: BlockOptions,
    /// Pixels per unit of length.
    pub resolution: Option<Resolution>,
    /// The colour space the pixel values are expressed in.
    pub rgb_working_space: Option<RgbWorkingSpace>,
    /// How the image should be displayed, which is not how it is stored.
    pub display_function: Option<DisplayFunction>,
    /// The mosaic pattern, for an image straight off a colour sensor.
    pub color_filter_array: Option<ColorFilterArray>,
    /// A raw ICC profile, written to its own data block.
    pub icc_profile: Option<Vec<u8>>,
    /// A preview of this image, written to its own data block.
    pub thumbnail: Option<PendingThumbnail>,
    /// FITS keywords, in the order they should appear.
    pub fits_keywords: Vec<FitsKeyword>,
}

/// A preview image attached to another image.
#[derive(Clone, Debug)]
pub struct PendingThumbnail {
    pub image: Image,
    pub data: Vec<u8>,
    pub options: BlockOptions,
}

/// One `<FITSKeyword>` element.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FitsKeyword {
    pub name: String,
    pub value: String,
    pub comment: String,
}

impl PendingImage {
    /// An image with pixel data and nothing attached to it.
    pub fn new(image: Image, data: Vec<u8>, options: BlockOptions) -> Self {
        PendingImage {
            image,
            data,
            options,
            resolution: None,
            rgb_working_space: None,
            display_function: None,
            color_filter_array: None,
            icc_profile: None,
            thumbnail: None,
            fits_keywords: Vec::new(),
        }
    }

    pub fn with_resolution(mut self, resolution: Resolution) -> Self {
        self.resolution = Some(resolution);
        self
    }

    pub fn with_rgb_working_space(mut self, space: RgbWorkingSpace) -> Self {
        self.rgb_working_space = Some(space);
        self
    }

    pub fn with_display_function(mut self, function: DisplayFunction) -> Self {
        self.display_function = Some(function);
        self
    }

    pub fn with_color_filter_array(mut self, cfa: ColorFilterArray) -> Self {
        self.color_filter_array = Some(cfa);
        self
    }

    /// Attach a raw ICC profile.
    ///
    /// The bytes are written exactly as given. An ICC profile is defined as
    /// big-endian by its own specification, so the spec forbids a `byteOrder`
    /// attribute on this block and none is written.
    pub fn with_icc_profile(mut self, profile: Vec<u8>) -> Self {
        self.icc_profile = Some(profile);
        self
    }

    /// Attach a preview image.
    ///
    /// A thumbnail must be a two-dimensional 8- or 16-bit image, which is
    /// checked when the file is written rather than here, so a caller building
    /// one up can do it in any order.
    pub fn with_thumbnail(mut self, thumbnail: PendingThumbnail) -> Self {
        self.thumbnail = Some(thumbnail);
        self
    }

    pub fn with_fits_keyword(
        mut self,
        name: impl Into<String>,
        value: impl Into<String>,
        comment: impl Into<String>,
    ) -> Self {
        self.fits_keywords.push(FitsKeyword {
            name: name.into(),
            value: value.into(),
            comment: comment.into(),
        });
        self
    }
}

impl PendingThumbnail {
    pub fn new(image: Image, data: Vec<u8>, options: BlockOptions) -> Self {
        PendingThumbnail { image, data, options }
    }
}

/// Builds a monolithic XISF file.
#[derive(Clone, Debug, Default)]
pub struct Writer {
    images: Vec<PendingImage>,
    /// Extra `<Property>` elements for the `<Metadata>` block.
    metadata: Vec<(String, String, String)>,
    creator: Option<String>,
    /// `XISF:CreationTime`, as an ISO 8601 instant. `None` means "now",
    /// resolved when the file is rendered.
    creation_time: Option<String>,
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

    /// Set `XISF:CreationTime` rather than taking it from the clock.
    ///
    /// The spec requires the property, so it is always written; this exists
    /// because a timestamp makes output non-reproducible, and a caller
    /// rebuilding a file or comparing two runs needs to be able to pin it.
    /// The value is written as given, so it must be a valid ISO 8601 instant.
    pub fn with_creation_time(mut self, instant: impl Into<String>) -> Self {
        self.creation_time = Some(instant.into());
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
        // A thumbnail is a preview meant to be cheap to display, so the spec
        // restricts it to two dimensions and to 8- or 16-bit samples. Writing
        // one that breaks those rules produces a file readers may reject, and
        // the writer is the last place that can still say so usefully.
        if let Some(thumbnail) = &pending.thumbnail {
            let expected = thumbnail
                .image
                .data_size()
                .ok_or_else(|| err!(InvalidArgument, "the thumbnail's geometry overflows"))?;
            if thumbnail.data.len() as u64 != expected {
                return Err(err!(
                    InvalidArgument,
                    "the thumbnail declares {expected} bytes but {} were given",
                    thumbnail.data.len()
                ));
            }
            if thumbnail.image.dimensions.len() != 2 {
                return Err(err!(
                    InvalidArgument,
                    "a thumbnail must be two-dimensional, got {} dimension(s)",
                    thumbnail.image.dimensions.len()
                ));
            }
            if !matches!(
                thumbnail.image.sample_format,
                crate::image::SampleFormat::UInt8 | crate::image::SampleFormat::UInt16
            ) {
                return Err(err!(
                    InvalidArgument,
                    "a thumbnail must be UInt8 or UInt16, got {}",
                    thumbnail.image.sample_format.name()
                ));
            }
            // The spec forbids `bounds` on a thumbnail outright: its range is
            // always the full width of its integer type, so a bounds
            // attribute would be either redundant or a contradiction.
            if thumbnail.image.bounds.is_some() {
                return Err(err!(
                    InvalidArgument,
                    "a thumbnail must not declare bounds; its range is its sample format's"
                ));
            }
            if !matches!(
                thumbnail.image.color_space,
                crate::image::ColorSpace::Gray | crate::image::ColorSpace::Rgb
            ) {
                return Err(err!(
                    InvalidArgument,
                    "a thumbnail must be grayscale or RGB, got {}",
                    thumbnail.image.color_space.name()
                ));
            }
        }

        for keyword in &pending.fits_keywords {
            check_fits_name(&keyword.name)?;
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
        let stored = self.prepare_blocks()?;

        // Fixed point on the header length; see the module comment.
        let mut assumed =
            self.render(&stored, Addressing::Attached { data_at: PREAMBLE_LEN })?.len();
        let header = loop {
            let header =
                self.render(&stored, Addressing::Attached { data_at: PREAMBLE_LEN + assumed })?;
            if header.len() == assumed {
                break header;
            }
            // Monotonic: a longer header only ever pushes positions outward.
            assumed = header.len();
        };

        let mut out = Vec::with_capacity(
            PREAMBLE_LEN
                + header.len()
                + stored.blocks.iter().map(|b| b.bytes.len()).sum::<usize>(),
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
        for block in &stored.blocks {
            out.extend_from_slice(&block.bytes);
        }
        Ok(out)
    }

    /// Serialise as a *distributed* unit: a header file and a blocks file.
    ///
    /// `blocks_file_name` is what the header will name, so it must be the
    /// name the blocks file is actually saved under, and the two must end up
    /// in the same directory. The reader resolves a relative locator beside
    /// the file that named it.
    ///
    /// Unlike the monolithic form this needs no fixed point: the header names
    /// blocks by identifier rather than by position, so its length does not
    /// feed back into its contents.
    pub fn to_distributed(&self, blocks_file_name: &str) -> Result<DistributedUnit> {
        if blocks_file_name.is_empty() || blocks_file_name.contains('/') {
            return Err(err!(
                InvalidArgument,
                "the blocks file name must be a plain file name, got {blocks_file_name:?}"
            ));
        }

        let stored = self.prepare_blocks()?;

        let blocks: Vec<(u64, Vec<u8>)> = stored
            .blocks
            .iter()
            .enumerate()
            .map(|(index, block)| (block_id(index), block.bytes.clone()))
            .collect();

        let header =
            self.render(&stored, Addressing::Distributed { blocks_file: blocks_file_name })?;
        Ok(DistributedUnit {
            header: header.into_bytes(),
            blocks: crate::distributed::write_blocks_file(&blocks)?,
            blocks_file_name: blocks_file_name.to_string(),
        })
    }

    /// Compress every block once, and record which belongs to which image.
    ///
    /// An image can own three blocks -- its pixels, an ICC profile and a
    /// thumbnail -- so blocks are no longer one per image and the mapping has
    /// to be kept rather than inferred from a position in the list.
    fn prepare_blocks(&self) -> Result<StoredBlocks> {
        let mut blocks = Vec::new();
        let mut owners = Vec::with_capacity(self.images.len());

        let mut push = |data: &[u8], options: &BlockOptions| -> Result<usize> {
            blocks.push(StoredBlock::prepare(data, options)?);
            Ok(blocks.len() - 1)
        };

        for pending in &self.images {
            // The order here is the order the blocks are written in, and the
            // positions are derived from it, so it must not drift from the
            // order used when the file is assembled.
            let data = push(&pending.data, &pending.options)?;
            let icc = match &pending.icc_profile {
                None => None,
                Some(profile) => Some(push(profile, &BlockOptions::default())?),
            };
            let thumbnail = match &pending.thumbnail {
                None => None,
                Some(thumb) => Some(push(&thumb.data, &thumb.options)?),
            };
            owners.push(ImageBlocks { data, icc, thumbnail });
        }
        Ok(StoredBlocks { blocks, owners })
    }

    /// Render the XML header, naming blocks as `addressing` says.
    fn render(&self, stored: &StoredBlocks, addressing: Addressing<'_>) -> Result<String> {
        let mut xml = String::from(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
        xml.push_str(r#"<xisf version="1.0" xmlns="http://www.pixinsight.com/xisf" "#);
        xml.push_str(r#"xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" "#);
        xml.push_str(
            r#"xsi:schemaLocation="http://www.pixinsight.com/xisf http://pixinsight.com/xisf/xisf-1.0.xsd">"#,
        );

        // Every block's position is settled before anything is emitted, so
        // the order elements are written in cannot drift from the order the
        // blocks are laid down in. Getting that wrong silently points an
        // element at another element's bytes.
        let base = match addressing {
            Addressing::Attached { data_at } => data_at,
            Addressing::Distributed { .. } => 0,
        };
        let mut positions = Vec::with_capacity(stored.blocks.len());
        let mut cursor = base;
        for block in &stored.blocks {
            positions.push(cursor);
            cursor += block.bytes.len();
        }

        // How one block is addressed, whichever form the unit takes.
        let locate = |index: usize| -> String {
            let block = &stored.blocks[index];
            match addressing {
                Addressing::Attached { .. } => {
                    format!("attachment:{}:{}", positions[index], block.bytes.len())
                }
                // `path(@header_dir/name):0xID`. The `@header_dir` token is
                // the only relative form the grammar defines -- a bare
                // `path(name)` is neither absolute nor documented, however
                // obvious its meaning -- and the identifier goes in
                // hexadecimal, as the spec recommends.
                Addressing::Distributed { blocks_file } => {
                    format!(
                        "path({HEADER_DIR_TOKEN}/{}):{:#x}",
                        escape_attr(blocks_file),
                        block_id(index)
                    )
                }
            }
        };

        for (pending, owned) in self.images.iter().zip(&stored.owners) {
            let block = &stored.blocks[owned.data];
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
            if let Some(uuid) = &image.uuid {
                xml.push_str(&format!(" uuid=\"{}\"", escape_attr(uuid)));
            }
            if let Some(offset) = image.offset {
                xml.push_str(&format!(" offset=\"{offset}\""));
            }
            if let Some(orientation) = image.orientation {
                xml.push_str(&format!(" orientation=\"{}\"", orientation.to_attribute()));
            }
            push_block_attrs(&mut xml, block);

            xml.push_str(&format!(" location=\"{}\">", locate(owned.data)));

            // Ancillary elements are written as children of the image they
            // belong to. The spec also allows them at the root with a `uid`
            // and a `<Reference>`, which is how a file shares one element
            // between images; a writer that owns each element outright has
            // nothing to share, so the direct form is the honest one.
            for keyword in &pending.fits_keywords {
                xml.push_str(&format!(
                    "<FITSKeyword name=\"{}\" value=\"{}\" comment=\"{}\"/>",
                    escape_attr(&keyword.name),
                    escape_attr(&keyword.value),
                    escape_attr(&keyword.comment)
                ));
            }
            if let Some(resolution) = &pending.resolution {
                xml.push_str(&format!(
                    "<Resolution horizontal=\"{}\" vertical=\"{}\" unit=\"{}\"/>",
                    resolution.horizontal,
                    resolution.vertical,
                    resolution.unit.name()
                ));
            }
            if let Some(space) = &pending.rgb_working_space {
                xml.push_str(&format!(
                    "<RGBWorkingSpace x=\"{}\" y=\"{}\" Y=\"{}\" gamma=\"{}\"",
                    join_numbers(&space.x),
                    join_numbers(&space.y),
                    join_numbers(&space.luminance),
                    match space.gamma {
                        Gamma::Srgb => "sRGB".to_string(),
                        Gamma::Exponent(exponent) => exponent.to_string(),
                    }
                ));
                if let Some(name) = &space.name {
                    xml.push_str(&format!(" name=\"{}\"", escape_attr(name)));
                }
                xml.push_str("/>");
            }
            if let Some(function) = &pending.display_function {
                xml.push_str(&format!(
                    "<DisplayFunction m=\"{}\" s=\"{}\" h=\"{}\" l=\"{}\" r=\"{}\"",
                    join_numbers(&function.midtones),
                    join_numbers(&function.shadows),
                    join_numbers(&function.highlights),
                    join_numbers(&function.low_range),
                    join_numbers(&function.high_range)
                ));
                if let Some(name) = &function.name {
                    xml.push_str(&format!(" name=\"{}\"", escape_attr(name)));
                }
                xml.push_str("/>");
            }
            if let Some(cfa) = &pending.color_filter_array {
                xml.push_str(&format!(
                    "<ColorFilterArray pattern=\"{}\" width=\"{}\" height=\"{}\"",
                    cfa.pattern_string(),
                    cfa.width,
                    cfa.height
                ));
                if let Some(name) = &cfa.name {
                    xml.push_str(&format!(" name=\"{}\"", escape_attr(name)));
                }
                xml.push_str("/>");
            }
            if let Some(index) = owned.icc {
                // No `byteOrder`: an ICC profile is big-endian by its own
                // specification, and the XISF spec forbids the attribute here.
                xml.push_str("<ICCProfile");
                push_block_attrs(&mut xml, &stored.blocks[index]);
                xml.push_str(&format!(" location=\"{}\"/>", locate(index)));
            }
            if let Some(index) = owned.thumbnail {
                let thumb = &pending.thumbnail.as_ref().expect("a thumbnail block has one").image;
                let mut geometry: Vec<String> =
                    thumb.dimensions.iter().map(u64::to_string).collect();
                geometry.push(thumb.channels.to_string());
                xml.push_str(&format!(
                    "<Thumbnail geometry=\"{}\" sampleFormat=\"{}\" colorSpace=\"{}\"",
                    geometry.join(":"),
                    thumb.sample_format.name(),
                    thumb.color_space.name()
                ));
                push_block_attrs(&mut xml, &stored.blocks[index]);
                xml.push_str(&format!(" location=\"{}\"/>", locate(index)));
            }

            xml.push_str("</Image>");
        }

        // `<Metadata>` is required by the spec, and it *must* carry both
        // `XISF:CreationTime` and `XISF:CreatorApplication`, so neither is
        // conditional on the caller having said anything.
        xml.push_str("<Metadata>");
        xml.push_str(&format!(
            "<Property id=\"XISF:CreationTime\" type=\"TimePoint\" value=\"{}\"/>",
            match &self.creation_time {
                Some(instant) => escape_attr(instant),
                None => escape_attr(&now_iso8601()),
            }
        ));
        for (id, kind, value) in &self.metadata {
            xml.push_str(&format!(
                "<Property id=\"{}\" type=\"{}\">{}</Property>",
                escape_attr(id),
                escape_attr(kind),
                escape_text(value)
            ));
        }
        xml.push_str(&format!(
            "<Property id=\"XISF:CreatorApplication\" type=\"String\">{}</Property>",
            escape_text(self.creator.as_deref().unwrap_or("xisf-rs"))
        ));
        xml.push_str("</Metadata>");

        xml.push_str("</xisf>");
        Ok(xml)
    }
}

/// The current instant as `YYYY-MM-DDThh:mm:ssZ`, which is what a
/// `TimePoint` property holds.
///
/// Written out rather than taken from a date library: this is the only date
/// arithmetic in the workspace, and a dependency for it would be carried by
/// every user of the crate for one line of output. A clock before the epoch
/// is not something to fail a write over, so it reads as the epoch itself.
fn now_iso8601() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    iso8601_from_unix(seconds)
}

/// Format a Unix timestamp, in seconds, as an ISO 8601 instant in UTC.
fn iso8601_from_unix(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let time = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        time / 3600,
        (time % 3600) / 60,
        time % 60
    )
}

/// The civil date `days` after 1970-01-01, by Howard Hinnant's algorithm.
///
/// It works by shifting the epoch to March 1st, which puts the leap day at
/// the end of the year and so removes February's irregular length from every
/// other calculation.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
    let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// Write the `compression` and `checksum` attributes a block needs, if any.
fn push_block_attrs(xml: &mut String, block: &StoredBlock) {
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
}

/// Check a FITS keyword name against the FITS 3.0 grammar the spec cites.
///
/// "The keyword name shall be a left justified, 8-character, space-filled,
/// ASCII string with no embedded spaces", using digits, upper case `A`-`Z`,
/// underscore and hyphen only. The point of a `FITSKeyword` element is to be
/// a compatibility layer with FITS, so a name FITS itself would reject makes
/// the element useless for the one job it has -- and the file is checked
/// where it is built rather than by whoever eventually reads it.
fn check_fits_name(name: &str) -> Result<()> {
    if name.len() > 8 {
        return Err(err!(
            InvalidArgument,
            "the FITS keyword {name:?} is {} characters; the limit is 8",
            name.len()
        ));
    }
    if let Some(bad) = name
        .chars()
        .find(|c| !(c.is_ascii_digit() || c.is_ascii_uppercase() || *c == '_' || *c == '-'))
    {
        return Err(err!(
            InvalidArgument,
            "the FITS keyword {name:?} contains {bad:?}; only A-Z, 0-9, underscore and \
             hyphen are permitted"
        ));
    }
    Ok(())
}

/// Colon-separated numbers, the form every multi-valued XISF attribute uses.
fn join_numbers(values: &[f64]) -> String {
    values.iter().map(f64::to_string).collect::<Vec<_>>().join(":")
}

/// Every block in a unit, and which image each one belongs to.
struct StoredBlocks {
    /// In the order they are written, which is what fixes their positions.
    blocks: Vec<StoredBlock>,
    /// One entry per image, in the order the images were added.
    owners: Vec<ImageBlocks>,
}

/// Indices into [`StoredBlocks::blocks`] for one image.
struct ImageBlocks {
    data: usize,
    icc: Option<usize>,
    thumbnail: Option<usize>,
}

/// The identifier the `n`th block is given in a data blocks file.
///
/// One-based: zero is a legal identifier, but it is also what an uninitialised
/// field reads as, so starting at one makes a mistake visible.
fn block_id(index: usize) -> u64 {
    index as u64 + 1
}

/// A block after compression, ready to be addressed and written.
struct StoredBlock {
    bytes: Vec<u8>,
    compression: Option<Compression>,
    checksum: Option<crate::block::Checksum>,
}

impl StoredBlock {
    fn prepare(data: &[u8], options: &BlockOptions) -> Result<Self> {
        let (bytes, compression) = match &options.compression {
            None => (data.to_vec(), None),
            Some(request) => {
                let uncompressed_size = data.len() as u64;

                // Shuffle first, then compress: that is the order the spec
                // fixes, and the reader undoes it the other way round.
                // An item size of one shuffles nothing, and the spec does not
                // allow `+sh:...:1` as a stored form. Rather than making every
                // caller special-case eight-bit data -- the natural thing to
                // pass is the sample width, which is 1 for UInt8 -- it is
                // treated as a request for no shuffling.
                let item_size = request.shuffle_item_size.filter(|item| *item > 1);
                let staged = match item_size {
                    None => data.to_vec(),
                    Some(item) => {
                        let item = usize::try_from(item)
                            .map_err(|_| err!(InvalidArgument, "shuffle item size is unusable"))?;
                        codec::shuffle(data, item)
                    }
                };

                let compressed = compress(&staged, request.codec)?;

                // Compression that makes a block bigger is not worth writing:
                // the file grows and every reader pays to undo it. Storing it
                // plain is always legal, since `compression` is optional.
                if compressed.len() >= data.len() {
                    (data.to_vec(), None)
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
        let checksum = match options.checksum {
            None => None,
            Some(algorithm) => Some(checksum_of(&bytes, algorithm)?),
        };

        Ok(StoredBlock { bytes, compression, checksum })
    }
}

// As `codec::decompress`: with no codec feature enabled every arm below is
// compiled out, leaving `data` unread. That is right for such a build.
#[cfg_attr(not(any(feature = "zlib", feature = "lz4")), allow(unused_variables))]
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
            offset: None,
            orientation: None,
        }
    }

    fn sample(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 31 + 7) as u8).collect()
    }

    fn round_trip(options: BlockOptions, format: SampleFormat) -> Vec<u8> {
        let image = image(9, 7, 1, format);
        let data = sample(image.data_size().unwrap() as usize);

        let mut writer = Writer::new().with_creator("xisf-rs tests");
        writer.add_image(PendingImage::new(image, data.clone(), options)).expect("add_image");
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

    /// Only over what this build can actually write, so the matrix stays
    /// honest under `--no-default-features` rather than asserting that an
    /// absent codec works.
    fn writable_compressions() -> Vec<Option<CompressionRequest>> {
        let mut out = vec![None];
        if cfg!(feature = "zlib") {
            out.push(Some(CompressionRequest { codec: Codec2::Zlib, shuffle_item_size: None }));
            out.push(Some(CompressionRequest { codec: Codec2::Zlib, shuffle_item_size: Some(4) }));
        }
        if cfg!(feature = "lz4") {
            out.push(Some(CompressionRequest { codec: Codec2::Lz4, shuffle_item_size: None }));
            out.push(Some(CompressionRequest { codec: Codec2::Lz4, shuffle_item_size: Some(2) }));
        }
        out
    }

    fn writable_checksums() -> Vec<Option<ChecksumAlgorithm>> {
        if cfg!(feature = "checksums") {
            vec![None, Some(ChecksumAlgorithm::Sha256)]
        } else {
            vec![None]
        }
    }

    #[test]
    fn every_option_combination_round_trips() {
        for format in [SampleFormat::UInt8, SampleFormat::UInt16, SampleFormat::Float32] {
            for compression in writable_compressions() {
                for checksum in writable_checksums() {
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
            writer.add_image(PendingImage::new(image, data, BlockOptions::default())).unwrap();
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

    #[cfg(all(feature = "zlib", feature = "checksums"))]
    #[test]
    fn checksums_written_are_checksums_that_verify() {
        let image = image(8, 8, 1, SampleFormat::UInt16);
        let data = sample(image.data_size().unwrap() as usize);
        let mut writer = Writer::new();
        writer
            .add_image(PendingImage::new(
                image,
                data,
                BlockOptions {
                    compression: Some(CompressionRequest {
                        codec: Codec2::Zlib,
                        shuffle_item_size: Some(2),
                    }),
                    checksum: Some(ChecksumAlgorithm::Sha512),
                },
            ))
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
    #[cfg(feature = "zlib")]
    #[test]
    fn incompressible_data_is_stored_plain() {
        let image = image(4, 4, 1, SampleFormat::UInt8);
        // Bytes with no structure to exploit.
        let data: Vec<u8> = (0..16u32).map(|i| (i.wrapping_mul(2654435761) >> 24) as u8).collect();

        let mut writer = Writer::new();
        writer
            .add_image(PendingImage::new(
                image,
                data,
                BlockOptions {
                    compression: Some(CompressionRequest {
                        codec: Codec2::Zlib,
                        shuffle_item_size: None,
                    }),
                    checksum: None,
                },
            ))
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
            .add_image(PendingImage::new(
                image(10, 10, 1, SampleFormat::UInt16),
                vec![0; 3],
                BlockOptions::default(),
            ))
            .unwrap_err();
        assert_eq!(err.kind(), crate::ErrorKind::InvalidArgument);
    }

    #[test]
    fn special_characters_in_metadata_are_escaped() {
        let mut writer = Writer::new();
        writer.add_metadata("Test:Value", r#"a & b < c > d " e ' f"#);
        writer
            .add_image(PendingImage::new(
                image(2, 2, 1, SampleFormat::UInt8),
                sample(4),
                BlockOptions::default(),
            ))
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

#[cfg(test)]
mod time_tests {
    use super::iso8601_from_unix;

    /// Dates are the classic place for an off-by-one that only shows up on a
    /// leap day or a century boundary, so the cases are chosen to hit those
    /// rather than to sample a comfortable middle.
    #[test]
    fn unix_timestamps_format_as_iso8601() {
        for (seconds, expected) in [
            (0, "1970-01-01T00:00:00Z"),
            (86_399, "1970-01-01T23:59:59Z"),
            (86_400, "1970-01-02T00:00:00Z"),
            (68_169_600, "1972-02-29T00:00:00Z"),  // a leap day
            (951_782_400, "2000-02-29T00:00:00Z"), // 2000 is a leap year
            (4_107_456_000, "2100-02-28T00:00:00Z"), // 2100 is not a leap year,
            (4_107_542_400, "2100-03-01T00:00:00Z"), // so the day after is March
            (1_418_128_695, "2014-12-09T12:38:15Z"), // the spec's own example
        ] {
            assert_eq!(iso8601_from_unix(seconds), expected, "for {seconds}");
        }
    }
}

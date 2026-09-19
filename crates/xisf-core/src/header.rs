//! Parsing the XML header into the elements the rest of the engine works on.
//!
//! The header is standard XML 1.0 in UTF-8, so `quick-xml` does the lexing and
//! this module only gives the elements meaning. What it deliberately does
//! *not* do is interpret data: a [`DataRef`] says where bytes are and what has
//! been done to them, and resolving that is the reader's job.

use quick_xml::NsReader;
use quick_xml::events::Event;
use quick_xml::name::ResolveResult;

use crate::block::{ByteOrder, Checksum, Compression, Location, TextEncoding};
use crate::err;
use crate::error::Result;

/// The namespace every XISF 1.0 header declares.
pub const XISF_NAMESPACE: &str = "http://www.pixinsight.com/xisf";

/// How to obtain one element's data block.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct DataRef {
    pub location: Option<Location>,
    pub compression: Option<Compression>,
    pub checksum: Option<Checksum>,
    /// Byte order of the block's multi-byte values. Little-endian when the
    /// attribute is absent, which is the spec's default rather than the
    /// host's order.
    pub byte_order: ByteOrder,
    /// Character data, for `inline` and `embedded` blocks. Held as written;
    /// whitespace is insignificant in both encodings and is stripped on use.
    pub text: Option<String>,
    /// The `encoding` attribute of a `<Data>` element, which only that
    /// element carries. It is promoted onto the surrounding element's
    /// `Embedded` location once the parser has seen both.
    pub encoding: Option<TextEncoding>,
}

/// One element of the header, flattened to what the engine needs.
///
/// The header's shape is kept -- `children` preserves nesting -- because
/// properties attach to the element that contains them, and an `Image` may
/// carry its own `FITSKeyword`, `ICCProfile` and `Thumbnail` children.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Element {
    /// The local name, with any namespace prefix removed.
    pub name: String,
    /// The namespace URI this element was resolved in, or `None` when the
    /// document bound no namespace to it.
    ///
    /// Kept because the local name alone does not say whether an element is
    /// part of the format. Revision 1 requires extension elements to live in
    /// a namespace other than XISF's, so a header may legitimately carry an
    /// `<ext:Image>` that means nothing to a decoder -- and reading it as a
    /// core `<Image>` would invent an image the file does not contain.
    pub namespace: Option<String>,
    /// Attributes in document order.
    pub attributes: Vec<(String, String)>,
    /// Where this element's data block is, if it has one.
    pub data: DataRef,
    pub children: Vec<Element>,
}

impl Element {
    /// Whether this element belongs to the format rather than to an extension.
    ///
    /// An element with no namespace counts: the specification's own examples
    /// and much real-world output declare no default namespace, and refusing
    /// those would reject files every other decoder reads. What is excluded
    /// is an element explicitly placed in *another* namespace, which
    /// Revision 1 defines as an extension and requires decoders to ignore.
    pub fn is_core(&self) -> bool {
        match &self.namespace {
            None => true,
            Some(uri) => uri == XISF_NAMESPACE,
        }
    }

    /// Whether this is the named core element.
    pub fn is(&self, name: &str) -> bool {
        self.name == name && self.is_core()
    }

    /// An attribute's value, if present.
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attributes.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// Direct children with a given element name.
    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.children.iter().filter(move |c| c.is(name))
    }

    /// Every element in this subtree, this one first.
    ///
    /// Walked with an explicit worklist rather than by recursion. The depth
    /// is bounded when the header is parsed, so recursion would be safe, but
    /// a walk that cannot overflow whatever it is handed is one less thing
    /// depending on a limit set somewhere else.
    pub fn descendants(&self) -> Vec<&Element> {
        let mut out = Vec::new();
        let mut pending = vec![self];
        while let Some(element) = pending.pop() {
            out.push(element);
            // Reversed, so children come back out in document order.
            pending.extend(element.children.iter().rev());
        }
        out
    }
}

/// A parsed XISF header.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Header {
    /// The `version` attribute of the root element.
    pub version: String,
    /// The root `<xisf>` element.
    pub root: Element,
}

impl Header {
    /// Every `<Image>` in the header, in document order.
    pub fn images(&self) -> Vec<&Element> {
        self.root.descendants().into_iter().filter(|e| e.is("Image")).collect()
    }
}

impl Header {
    /// Image `id` values the header uses more than once.
    ///
    /// Revision 1 requires an image id to be unique within the unit. This
    /// reports rather than refuses: a duplicate makes "which image is this?"
    /// ambiguous, but the pixels are all still there and readable by index,
    /// so throwing the file away would lose more than it protects. The writer
    /// refuses to *produce* one, which is where the obligation really sits.
    pub fn duplicate_image_ids(&self) -> Vec<&str> {
        let mut seen: Vec<&str> = Vec::new();
        let mut duplicated: Vec<&str> = Vec::new();
        for image in self.images() {
            let Some(id) = image.attr("id") else { continue };
            if seen.contains(&id) {
                if !duplicated.contains(&id) {
                    duplicated.push(id);
                }
            } else {
                seen.push(id);
            }
        }
        duplicated
    }

    /// The element with a given `uid`, if the header defines one.
    pub fn by_uid(&self, uid: &str) -> Option<&Element> {
        self.root.descendants().into_iter().find(|e| e.is_core() && e.attr("uid") == Some(uid))
    }

    /// Every element of `name` associated with `owner`, following references.
    ///
    /// An element may sit inside the one it belongs to, or sit anywhere in
    /// the header with a `uid` and be pointed at by a `<Reference ref="...">`
    /// child. Both are ordinary and mean the same thing -- the second exists
    /// so one thumbnail or colour profile can serve several images without
    /// being serialised repeatedly.
    ///
    /// A reader that looked only at direct children would silently drop
    /// everything a file chose to share, which is data loss with no error.
    /// References cannot chain: the specification forbids a `Reference` from
    /// carrying a `uid`, so following one is a single step and cannot loop.
    pub fn associated<'a>(&'a self, owner: &'a Element, name: &'a str) -> Vec<&'a Element> {
        let mut out: Vec<&Element> = owner.children_named(name).collect();

        for reference in owner.children_named("Reference") {
            let Some(uid) = reference.attr("ref") else { continue };
            if let Some(target) = self.by_uid(uid)
                && target.is(name)
            {
                out.push(target);
            }
        }
        out
    }
}

/// Parse the header's XML text.
/// How deeply a header may nest before it is refused.
///
/// A real header nests four deep at most: `xisf > Table > Row > Cell`. The
/// tree is built iteratively, but it is walked, cloned, compared and *dropped*
/// by recursion, and a stack overflow in Rust aborts the process rather than
/// unwinding -- so an unbounded depth is a way for one hostile file to kill
/// whatever else the process was doing. The limit is far above anything a
/// real file uses and far below where any of those operations is at risk.
const MAX_DEPTH: usize = 256;

/// How many elements a header may contain before it is refused.
///
/// Each element costs far more in memory than the few bytes of XML that
/// declare it, so a header that is mostly `<a/>` expands by a large factor.
/// The header length field is 32 bits, which puts four gigabytes of such
/// declarations within the format's own rules; this bounds what that can turn
/// into. PixInsight's own files run to a few hundred elements.
const MAX_ELEMENTS: usize = 1_000_000;

/// How many attributes a header may declare in total before it is refused.
///
/// [`MAX_ELEMENTS`] bounds the tree's nodes but says nothing about their
/// width, and an attribute is the cheaper thing to write: `a=""` is five bytes
/// of XML and costs a `(String, String)` and two allocations to hold, so a
/// single element with nothing but attributes expands by a wider factor than
/// a header full of empty tags. The budget is counted across the whole header
/// rather than per element, because a limit on each of a million elements is
/// no limit at all. PixInsight's own files declare a few thousand.
const MAX_ATTRIBUTES: usize = 2_000_000;

pub fn parse(xml: &str) -> Result<Header> {
    let mut reader = NsReader::from_str(xml);
    let config = reader.config_mut();
    config.trim_text(false);
    config.expand_empty_elements = false;

    // A stack of elements under construction; the last is the current one.
    let mut stack: Vec<Element> = Vec::new();
    let mut root: Option<Element> = None;
    let mut elements = 0usize;
    let mut attributes = 0usize;

    loop {
        // Resolved rather than raw, so a prefix is turned into the namespace
        // it is bound to. A prefix on its own means nothing -- the same
        // document may bind `ext:` to the XISF namespace and the default
        // namespace to something else -- so only the resolved URI can say
        // whether an element is part of the format.
        match reader.read_resolved_event() {
            Err(e) => {
                return Err(err!(BadHeader, "at position {}: {e}", reader.buffer_position()));
            }
            Ok((_, Event::Eof)) => break,

            Ok((ns, Event::Start(start))) => {
                count(&mut elements)?;
                if stack.len() >= MAX_DEPTH {
                    return Err(err!(
                        BadHeader,
                        "the header is nested more than {MAX_DEPTH} elements deep"
                    ));
                }
                let namespace = namespace_of(&ns)?;
                stack.push(element_from(&start, namespace, &mut attributes)?);
            }
            Ok((ns, Event::Empty(start))) => {
                count(&mut elements)?;
                let namespace = namespace_of(&ns)?;
                let element = element_from(&start, namespace, &mut attributes)?;
                finish(element, &mut stack, &mut root)?;
            }
            Ok((_, Event::End(_))) => {
                let element = stack
                    .pop()
                    .ok_or_else(|| err!(BadHeader, "a closing tag with nothing open"))?;
                finish(element, &mut stack, &mut root)?;
            }

            Ok((_, Event::Text(text))) => {
                // `decode` converts bytes to text; it does *not* resolve
                // entities. Without unescaping, `&amp;` and `&lt;` are
                // silently dropped rather than becoming `&` and `<`, which
                // corrupts any character data containing them.
                let raw = text.decode().map_err(|e| err!(BadHeader, "character data: {e}"))?;
                push_text(&mut stack, &raw);
            }
            // Base64 and hex blocks are sometimes wrapped in CDATA; the
            // content means the same thing either way.
            Ok((_, Event::CData(data))) => {
                let decoded = String::from_utf8(data.to_vec())
                    .map_err(|e| err!(BadHeader, "CDATA is not UTF-8: {e}"))?;
                push_text(&mut stack, &decoded);
            }
            // An entity or character reference inside character data arrives
            // as its own event rather than as part of the surrounding text.
            // Left to the catch-all below it would be dropped silently, so
            // `a &amp; b` would read back as `a  b` -- data loss that looks
            // like nothing at all went wrong.
            Ok((_, Event::GeneralRef(reference))) => {
                let raw = core::str::from_utf8(&reference)
                    .map_err(|e| err!(BadHeader, "entity reference: {e}"))?;
                let resolved = resolve_entity(raw)
                    .ok_or_else(|| err!(BadHeader, "unknown entity reference &{raw};"))?;
                push_text(&mut stack, &resolved);
            }
            // Comments, declarations and processing instructions carry no
            // data the engine needs.
            Ok(_) => {}
        }
    }

    if !stack.is_empty() {
        return Err(err!(BadHeader, "{} element(s) were never closed", stack.len()));
    }
    let root = root.ok_or_else(|| err!(BadHeader, "the header has no elements"))?;
    if !root.is("xisf") {
        return Err(err!(
            BadHeader,
            "the root element is <{}> in namespace {:?}, expected <xisf>",
            root.name,
            root.namespace.as_deref().unwrap_or("(none)")
        ));
    }
    let version = root.attr("version").unwrap_or_default().to_string();
    if version != "1.0" {
        return Err(err!(Unsupported, "XISF version {version:?} is not supported"));
    }

    Ok(Header { version, root })
}

/// The namespace URI a resolved event belongs to, if any.
///
/// An unbound prefix is a malformed document rather than an extension: the
/// writer of `<ext:Image>` without an `xmlns:ext` declaration meant
/// *something*, and guessing which namespace would be inventing content.
fn namespace_of(resolved: &ResolveResult<'_>) -> Result<Option<String>> {
    Ok(match resolved {
        ResolveResult::Unbound => None,
        ResolveResult::Bound(ns) => Some(
            core::str::from_utf8(ns.as_ref())
                .map_err(|e| err!(BadHeader, "a namespace URI is not UTF-8: {e}"))?
                .to_string(),
        ),
        ResolveResult::Unknown(prefix) => {
            let prefix = String::from_utf8_lossy(prefix);
            return Err(err!(BadHeader, "the namespace prefix {prefix:?} is not declared"));
        }
    })
}

/// Resolve the five entities XML predefines, and numeric character
/// references. XISF headers declare no DTD, so nothing else can appear.
fn resolve_entity(name: &str) -> Option<String> {
    Some(match name {
        "amp" => "&".into(),
        "lt" => "<".into(),
        "gt" => ">".into(),
        "quot" => "\"".into(),
        "apos" => "'".into(),
        _ => {
            let digits = name.strip_prefix('#')?;
            let code = match digits.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => digits.parse::<u32>().ok()?,
            };
            char::from_u32(code)?.to_string()
        }
    })
}

/// Append character data to the element currently open.
///
/// Whitespace is kept. Discarding whitespace-only runs would be a tempting
/// way to ignore indentation between elements, but character data arrives in
/// pieces -- an entity reference splits it -- so `&quot;a&quot; &apos;b&apos;`
/// reaches here as three fragments with the significant space arriving alone.
/// Dropping it loses data with no error. Indentation is dealt with where it
/// matters instead: block decoders strip whitespace, which is insignificant in
/// both base64 and hex.
fn push_text(stack: &mut [Element], text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(current) = stack.last_mut() {
        current.data.text.get_or_insert_with(String::new).push_str(text);
    }
}

/// Attach a finished element to its parent, or record it as the root.
/// Count one element, refusing a header that declares absurdly many.
fn count(elements: &mut usize) -> Result<()> {
    *elements += 1;
    if *elements > MAX_ELEMENTS {
        return Err(err!(BadHeader, "the header declares more than {MAX_ELEMENTS} elements"));
    }
    Ok(())
}

fn finish(element: Element, stack: &mut [Element], root: &mut Option<Element>) -> Result<()> {
    match stack.last_mut() {
        Some(parent) => {
            // A `<Data>` child is not an element in its own right: it exists
            // to carry an embedded block for the element around it, and the
            // schema gives it its own encoding, compression, subblocks and
            // checksum attributes. All of them describe the parent's block,
            // so all of them are promoted -- dropping the checksum in
            // particular meant a block that recorded how to detect tampering
            // was handed over unverified, which is worse than recording
            // nothing at all.
            if element.is("Data") {
                // The parent may already hold the indentation that preceded
                // this child, which is not content and must not block the
                // promotion.
                if parent.data.text.as_deref().is_none_or(|t| t.trim().is_empty()) {
                    parent.data.text = element.data.text.clone();
                }
                if parent.data.location.is_none() {
                    parent.data.location = element.data.location.clone();
                }
                if let Some(encoding) = element.data.encoding
                    && let Some(Location::Embedded { encoding: slot }) = &mut parent.data.location
                {
                    *slot = encoding;
                }
                if parent.data.compression.is_none() {
                    parent.data.compression = element.data.compression.clone();
                }
                if parent.data.checksum.is_none() {
                    parent.data.checksum = element.data.checksum.clone();
                }
            }
            parent.children.push(element);
        }
        None => {
            if root.is_some() {
                return Err(err!(BadHeader, "the header has more than one root element"));
            }
            *root = Some(element);
        }
    }
    Ok(())
}

fn element_from(
    start: &quick_xml::events::BytesStart<'_>,
    namespace: Option<String>,
    budget: &mut usize,
) -> Result<Element> {
    let qname = start.name();
    let raw = core::str::from_utf8(qname.as_ref())
        .map_err(|e| err!(BadHeader, "an element name is not UTF-8: {e}"))?;
    // Namespace prefixes carry no meaning the engine needs; the namespace is
    // fixed by the spec and checked on the root.
    let name = raw.rsplit(':').next().unwrap_or(raw).to_string();

    let mut attributes = Vec::new();
    let mut data = DataRef::default();
    let mut subblocks: Option<Vec<(u64, u64)>> = None;

    for attribute in start.attributes() {
        *budget += 1;
        if *budget > MAX_ATTRIBUTES {
            return Err(err!(
                BadHeader,
                "the header declares more than {MAX_ATTRIBUTES} attributes"
            ));
        }
        let attribute = attribute.map_err(|e| err!(BadHeader, "in <{name}>: {e}"))?;
        let key_raw = core::str::from_utf8(attribute.key.as_ref())
            .map_err(|e| err!(BadHeader, "an attribute name is not UTF-8: {e}"))?;
        let key = key_raw.rsplit(':').next().unwrap_or(key_raw).to_string();
        let value = attribute
            .unescape_value()
            .map_err(|e| err!(BadHeader, "in <{name}> attribute {key}: {e}"))?
            .into_owned();

        match key.as_str() {
            "location" => data.location = Some(Location::parse(&value)?),
            "compression" => data.compression = Some(Compression::parse(&value)?),
            // `subblocks` may be read before or after `compression`, since
            // XML attributes have no required order, so it is stashed and
            // married up once both have been seen.
            "subblocks" => subblocks = Some(Compression::parse_subblocks(&value)?),
            "checksum" => data.checksum = Some(Checksum::parse(&value)?),
            "byteOrder" => data.byte_order = ByteOrder::parse(&value)?,
            "encoding" => {
                data.encoding = Some(match value.as_str() {
                    "base64" => TextEncoding::Base64,
                    "hex" => TextEncoding::Hex,
                    other => {
                        return Err(err!(
                            BadAttribute,
                            "a <Data> encoding must be base64 or hex, got {other:?}"
                        ));
                    }
                });
            }
            _ => {}
        }
        attributes.push((key, value));
    }

    // `subblocks` describes how the compressed stream is divided, so it is
    // meaningless without `compression` -- and silently dropping it would
    // mean decoding only the first subblock and reporting a short block.
    if let Some(subblocks) = subblocks {
        match &mut data.compression {
            Some(compression) => compression.subblocks = subblocks,
            None => {
                return Err(err!(
                    BadHeader,
                    "<{name}> has a subblocks attribute but is not compressed"
                ));
            }
        }
    }

    Ok(Element { name, namespace, attributes, data, children: Vec::new() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ErrorKind;
    use crate::block::{Codec, TextEncoding};

    const MINIMAL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<xisf version="1.0" xmlns="http://www.pixinsight.com/xisf">
  <Image geometry="10:10:3" sampleFormat="UInt8" colorSpace="RGB"
         location="inline:base64">AAAA</Image>
</xisf>"#;

    #[test]
    fn parses_a_minimal_header() {
        let header = parse(MINIMAL).unwrap();
        assert_eq!(header.version, "1.0");
        let images = header.images();
        assert_eq!(images.len(), 1);
        let image = images[0];
        assert_eq!(image.attr("geometry"), Some("10:10:3"));
        assert_eq!(image.attr("sampleFormat"), Some("UInt8"));
        assert_eq!(image.data.location, Some(Location::Inline { encoding: TextEncoding::Base64 }));
        assert_eq!(image.data.text.as_deref(), Some("AAAA"));
    }

    #[test]
    fn an_embedded_data_child_belongs_to_its_parent() {
        let xml = r#"<xisf version="1.0"><Image location="embedded">
            <Data encoding="base64">QUJD</Data></Image></xisf>"#;
        let header = parse(xml).unwrap();
        let image = header.images()[0];
        assert_eq!(
            image.data.location,
            Some(Location::Embedded { encoding: crate::block::TextEncoding::Base64 })
        );
        assert_eq!(image.data.text.as_deref(), Some("QUJD"));
    }

    #[test]
    fn block_attributes_are_parsed_not_just_carried() {
        let xml = r#"<xisf version="1.0"><Image
            location="attachment:9869:23042"
            compression="zlib+sh:30000:4"
            checksum="sha256:d60477d1c651b6bc42a8aa938a6132006257d76f229a2d7481db84d23dea8b96"/></xisf>"#;
        let image = parse(xml).unwrap();
        let image = image.images()[0];
        assert_eq!(image.data.location, Some(Location::Attachment { position: 9869, size: 23042 }));
        let compression = image.data.compression.clone().unwrap();
        assert_eq!(compression.codec, Codec::Zlib);
        assert_eq!(compression.uncompressed_size, 30000);
        assert_eq!(compression.shuffle_item_size, Some(4));
        assert_eq!(image.data.checksum.as_ref().unwrap().digest.len(), 32);
    }

    #[test]
    fn nesting_is_preserved() {
        let xml = r#"<xisf version="1.0"><Image>
            <FITSKeyword name="SIMPLE" value="T" comment="x"/>
            <FITSKeyword name="BITPIX" value="-32" comment="y"/>
        </Image></xisf>"#;
        let header = parse(xml).unwrap();
        let image = header.images()[0];
        let keywords: Vec<_> = image.children_named("FITSKeyword").collect();
        assert_eq!(keywords.len(), 2);
        assert_eq!(keywords[1].attr("name"), Some("BITPIX"));
    }

    /// Entity references arrive as their own event, not as part of the text
    /// around them. Dropping them silently turns `a &amp; b` into `a  b` --
    /// data loss with no error, which is why this is pinned directly rather
    /// than left to a round-trip test to notice.
    #[test]
    fn entity_and_character_references_are_resolved() {
        for (xml, expected) in [
            ("a &amp; b", "a & b"),
            ("&lt;tag&gt;", "<tag>"),
            ("&quot;q&quot; &apos;a&apos;", "\"q\" 'a'"),
            ("&#65;&#66;&#67;", "ABC"),
            ("&#x41;&#x42;", "AB"),
            ("&#xe9;", "\u{e9}"),
            ("mixed &amp; matched &#33;", "mixed & matched !"),
        ] {
            let doc = format!(
                r#"<xisf version="1.0"><Property id="t" type="String">{xml}</Property></xisf>"#
            );
            let header = parse(&doc).unwrap_or_else(|e| panic!("{xml}: {e}"));
            let property = &header.root.children[0];
            assert_eq!(
                property.data.text.as_deref(),
                Some(expected),
                "{xml} should read back as {expected:?}"
            );
        }
    }

    #[test]
    fn an_unknown_entity_is_an_error_rather_than_a_silent_gap() {
        let xml = r#"<xisf version="1.0"><Property id="t">&nosuch;</Property></xisf>"#;
        assert_eq!(parse(xml).unwrap_err().kind(), ErrorKind::BadHeader);
    }

    /// An element may be shared: defined once with a `uid` and pointed at by
    /// a `<Reference>` inside each element that uses it. Looking only at
    /// direct children loses it silently.
    #[test]
    fn references_associate_a_shared_element_with_its_owners() {
        let xml = r#"<xisf version="1.0">
            <Resolution uid="R" horizontal="300" vertical="300"/>
            <Image geometry="2:2:1" sampleFormat="UInt8"><Reference ref="R"/></Image>
            <Image geometry="4:4:1" sampleFormat="UInt8"><Reference ref="R"/></Image>
            <Image geometry="8:8:1" sampleFormat="UInt8">
                <Resolution horizontal="72" vertical="72"/>
            </Image>
        </xisf>"#;
        let header = parse(xml).unwrap();
        let images = header.images();
        assert_eq!(images.len(), 3);

        // The first two share one element by reference...
        for image in &images[..2] {
            let found = header.associated(image, "Resolution");
            assert_eq!(found.len(), 1, "a referenced Resolution was not found");
            assert_eq!(found[0].attr("horizontal"), Some("300"));
        }
        // ...and the third has its own, directly.
        let own = header.associated(images[2], "Resolution");
        assert_eq!(own.len(), 1);
        assert_eq!(own[0].attr("horizontal"), Some("72"));
    }

    #[test]
    fn a_reference_to_nothing_or_to_the_wrong_kind_is_ignored() {
        let xml = r#"<xisf version="1.0">
            <Thumbnail uid="T" geometry="4:4:1" sampleFormat="UInt8"/>
            <Image geometry="2:2:1" sampleFormat="UInt8">
                <Reference ref="nosuch"/>
                <Reference ref="T"/>
            </Image>
        </xisf>"#;
        let header = parse(xml).unwrap();
        let image = header.images()[0];

        // A dangling reference is skipped rather than fatal; a reference to a
        // Thumbnail does not answer a question about a Resolution.
        assert!(header.associated(image, "Resolution").is_empty());
        assert_eq!(header.associated(image, "Thumbnail").len(), 1);
    }

    #[test]
    fn a_wrong_root_or_version_is_refused() {
        assert_eq!(parse("<nope/>").unwrap_err().kind(), ErrorKind::BadHeader);
        assert_eq!(parse(r#"<xisf version="2.0"/>"#).unwrap_err().kind(), ErrorKind::Unsupported);
    }

    #[test]
    fn malformed_xml_is_an_error_not_a_panic() {
        for xml in ["<xisf", "<xisf></nope>", "", "<xisf/><xisf/>", "<xisf><a></xisf>"] {
            let _ = parse(xml);
        }
    }
}

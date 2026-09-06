//! Parsing the XML header into the elements the rest of the engine works on.
//!
//! The header is standard XML 1.0 in UTF-8, so `quick-xml` does the lexing and
//! this module only gives the elements meaning. What it deliberately does
//! *not* do is interpret data: a [`DataRef`] says where bytes are and what has
//! been done to them, and resolving that is the reader's job.

use quick_xml::Reader;
use quick_xml::events::Event;

use crate::block::{Checksum, Compression, Location};
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
    /// Character data, for `inline` and `embedded` blocks. Held as written;
    /// whitespace is insignificant in both encodings and is stripped on use.
    pub text: Option<String>,
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
    /// Attributes in document order.
    pub attributes: Vec<(String, String)>,
    /// Where this element's data block is, if it has one.
    pub data: DataRef,
    pub children: Vec<Element>,
}

impl Element {
    /// An attribute's value, if present.
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attributes.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// Direct children with a given element name.
    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }

    /// Every element in this subtree, this one first.
    pub fn descendants(&self) -> Vec<&Element> {
        let mut out = vec![self];
        for child in &self.children {
            out.extend(child.descendants());
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
        self.root.descendants().into_iter().filter(|e| e.name == "Image").collect()
    }
}

/// Parse the header's XML text.
pub fn parse(xml: &str) -> Result<Header> {
    let mut reader = Reader::from_str(xml);
    let config = reader.config_mut();
    config.trim_text(false);
    config.expand_empty_elements = false;

    // A stack of elements under construction; the last is the current one.
    let mut stack: Vec<Element> = Vec::new();
    let mut root: Option<Element> = None;

    loop {
        match reader.read_event() {
            Err(e) => {
                return Err(err!(BadHeader, "at position {}: {e}", reader.buffer_position()));
            }
            Ok(Event::Eof) => break,

            Ok(Event::Start(start)) => stack.push(element_from(&start)?),
            Ok(Event::Empty(start)) => {
                let element = element_from(&start)?;
                finish(element, &mut stack, &mut root)?;
            }
            Ok(Event::End(_)) => {
                let element = stack
                    .pop()
                    .ok_or_else(|| err!(BadHeader, "a closing tag with nothing open"))?;
                finish(element, &mut stack, &mut root)?;
            }

            Ok(Event::Text(text)) => {
                let decoded = text.decode().map_err(|e| err!(BadHeader, "character data: {e}"))?;
                push_text(&mut stack, &decoded);
            }
            // Base64 and hex blocks are sometimes wrapped in CDATA; the
            // content means the same thing either way.
            Ok(Event::CData(data)) => {
                let decoded = String::from_utf8(data.to_vec())
                    .map_err(|e| err!(BadHeader, "CDATA is not UTF-8: {e}"))?;
                push_text(&mut stack, &decoded);
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
    if root.name != "xisf" {
        return Err(err!(BadHeader, "the root element is <{}>, expected <xisf>", root.name));
    }
    let version = root.attr("version").unwrap_or_default().to_string();
    if version != "1.0" {
        return Err(err!(Unsupported, "XISF version {version:?} is not supported"));
    }

    Ok(Header { version, root })
}

/// Append character data to the element currently open, ignoring whitespace
/// between elements.
fn push_text(stack: &mut [Element], text: &str) {
    if text.trim().is_empty() {
        return;
    }
    if let Some(current) = stack.last_mut() {
        current.data.text.get_or_insert_with(String::new).push_str(text);
    }
}

/// Attach a finished element to its parent, or record it as the root.
fn finish(element: Element, stack: &mut [Element], root: &mut Option<Element>) -> Result<()> {
    match stack.last_mut() {
        Some(parent) => {
            // An `<Data>` child is not an element in its own right: it exists
            // to carry an embedded block's text for the element around it.
            if element.name == "Data" {
                if parent.data.text.is_none() {
                    parent.data.text = element.data.text.clone();
                }
                if parent.data.location.is_none() {
                    parent.data.location = element.data.location.clone();
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

fn element_from(start: &quick_xml::events::BytesStart<'_>) -> Result<Element> {
    let qname = start.name();
    let raw = std::str::from_utf8(qname.as_ref())
        .map_err(|e| err!(BadHeader, "an element name is not UTF-8: {e}"))?;
    // Namespace prefixes carry no meaning the engine needs; the namespace is
    // fixed by the spec and checked on the root.
    let name = raw.rsplit(':').next().unwrap_or(raw).to_string();

    let mut attributes = Vec::new();
    let mut data = DataRef::default();

    for attribute in start.attributes() {
        let attribute = attribute.map_err(|e| err!(BadHeader, "in <{name}>: {e}"))?;
        let key_raw = std::str::from_utf8(attribute.key.as_ref())
            .map_err(|e| err!(BadHeader, "an attribute name is not UTF-8: {e}"))?;
        let key = key_raw.rsplit(':').next().unwrap_or(key_raw).to_string();
        let value = attribute
            .unescape_value()
            .map_err(|e| err!(BadHeader, "in <{name}> attribute {key}: {e}"))?
            .into_owned();

        match key.as_str() {
            "location" => data.location = Some(Location::parse(&value)?),
            "compression" => data.compression = Some(Compression::parse(&value)?),
            "checksum" => data.checksum = Some(Checksum::parse(&value)?),
            _ => {}
        }
        attributes.push((key, value));
    }

    Ok(Element { name, attributes, data, children: Vec::new() })
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
        assert_eq!(image.data.location, Some(Location::Embedded));
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
        let compression = image.data.compression.unwrap();
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

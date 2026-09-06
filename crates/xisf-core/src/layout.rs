//! The fixed part of a monolithic XISF file: signature, header length, header.

use crate::err;
use crate::error::Result;

/// The eight bytes every monolithic XISF file starts with.
pub const SIGNATURE: &[u8; 8] = b"XISF0100";

/// Signature, length and reserved field, before the XML begins.
pub const PREAMBLE_LEN: usize = 16;

/// Where the header is, and where the data after it begins.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Layout {
    /// Byte range of the XML header within the file.
    pub header: (usize, usize),
}

impl Layout {
    /// The offset at which attached data blocks are addressed from.
    ///
    /// `location="attachment:position:size"` gives an absolute file offset,
    /// so this is only the earliest position a block may legally occupy.
    pub fn data_start(&self) -> usize {
        self.header.1
    }
}

/// Read the preamble and locate the header.
pub fn scan(bytes: &[u8]) -> Result<Layout> {
    if bytes.len() < PREAMBLE_LEN {
        return Err(err!(
            Truncated,
            "a monolithic XISF file needs at least {PREAMBLE_LEN} bytes, this has {}",
            bytes.len()
        ));
    }
    if &bytes[..8] != SIGNATURE {
        return Err(err!(
            NotXisf,
            "expected the signature {:?}, found {:?}",
            String::from_utf8_lossy(SIGNATURE),
            String::from_utf8_lossy(&bytes[..8])
        ));
    }

    // Little-endian by specification, whatever the host happens to be.
    let header_len = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;

    // The reserved field must be zero. A non-zero value means either a
    // future revision or a damaged file, and guessing which is worse than
    // saying so.
    if bytes[12..16] != [0, 0, 0, 0] {
        return Err(err!(
            BadHeader,
            "the reserved field must be zero, found {:02x?}",
            &bytes[12..16]
        ));
    }

    let end = PREAMBLE_LEN.checked_add(header_len).ok_or_else(|| {
        err!(Truncated, "the declared header length {header_len} overflows this platform")
    })?;
    if end > bytes.len() {
        return Err(err!(
            Truncated,
            "the header claims {header_len} bytes, which runs {} past the end of the file",
            end - bytes.len()
        ));
    }

    Ok(Layout { header: (PREAMBLE_LEN, end) })
}

/// The header's XML text.
pub fn header_str<'a>(bytes: &'a [u8], layout: &Layout) -> Result<&'a str> {
    let raw = &bytes[layout.header.0..layout.header.1];
    std::str::from_utf8(raw).map_err(|e| err!(BadHeader, "the header is not valid UTF-8: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ErrorKind;

    fn file_with(header: &str) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(SIGNATURE);
        out.extend_from_slice(&(header.len() as u32).to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(header.as_bytes());
        out
    }

    #[test]
    fn scans_a_well_formed_preamble() {
        let bytes = file_with("<xisf/>");
        let layout = scan(&bytes).unwrap();
        assert_eq!(layout.header, (16, 23));
        assert_eq!(header_str(&bytes, &layout).unwrap(), "<xisf/>");
        assert_eq!(layout.data_start(), 23);
    }

    #[test]
    fn rejects_a_file_that_is_not_xisf() {
        assert_eq!(scan(b"not an xisf file at all").unwrap_err().kind(), ErrorKind::NotXisf);
    }

    #[test]
    fn rejects_a_header_running_past_the_end() {
        let mut bytes = file_with("<xisf/>");
        bytes[8] = 0xff;
        assert_eq!(scan(&bytes).unwrap_err().kind(), ErrorKind::Truncated);
    }

    #[test]
    fn rejects_a_non_zero_reserved_field() {
        let mut bytes = file_with("<xisf/>");
        bytes[12] = 1;
        assert_eq!(scan(&bytes).unwrap_err().kind(), ErrorKind::BadHeader);
    }

    #[test]
    fn truncation_at_every_length_is_an_error_not_a_panic() {
        let bytes = file_with("<xisf version=\"1.0\"/>");
        for n in 0..bytes.len() {
            let _ = scan(&bytes[..n]);
        }
    }
}

//! The XISF engine: reading and writing the format itself.
//!
//! Written from the [XISF 1.0 specification][spec], which states that anyone
//! may implement it freely. Nothing here derives from libXISF (GPL-3.0) or
//! from the PixInsight Class Library, whose licence forbids this use; see
//! `docs/PLAN.md`.
//!
//! [spec]: https://pixinsight.com/doc/docs/XISF-1.0-spec/XISF-1.0-spec.html
//!
//! A monolithic XISF file is a signature, a header length, the XML header,
//! and then whatever data blocks the header points at:
//!
//! ```text
//! "XISF0100"        8 bytes
//! header length     u32, little-endian
//! reserved          4 bytes, zero
//! XISF header       XML 1.0, UTF-8
//! [data blocks]     addressed by `location="attachment:position:size"`
//! ```

pub mod block;
pub mod codec;
pub mod distributed;
pub mod error;
pub mod header;
pub mod image;
pub mod layout;
pub mod property;
pub mod reader;
pub mod writer;

pub use error::{Error, ErrorKind, Result};
pub use reader::{ChecksumStatus, Reader};

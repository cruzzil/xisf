//! The `.xisb` data blocks file: its signature, and the linked list that
//! indexes it.
//!
//! The index is a singly linked list whose nodes may sit anywhere in the file,
//! which makes it the one structure here that a hostile file can tangle:
//! cycles, overlapping nodes, and element counts that do not fit the space
//! they claim. The in-memory walk and the seeking one are separate code with
//! the same job, so both are driven -- an invariant enforced in one and
//! forgotten in the other is exactly the kind of thing this catches.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(index) = xisf_core::distributed::parse_blocks_file(data) {
        for element in index.occupied() {
            let _ = index.get(element.id);
        }
    }

    // The seeking walk, over the same bytes.
    let mut cursor = std::io::Cursor::new(data);
    let len = data.len() as u64;
    for id in [0u64, 1, u64::MAX] {
        let _ = xisf_core::distributed::read_block_from(&mut cursor, len, id);
    }
});

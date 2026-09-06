//! Read every file libXISF wrote.
//!
//! `corpus/generated/` is produced by `tools/corpus-gen`, which links libXISF
//! and writes a matrix of sample formats against compression codecs. That
//! matters more than the file count suggests: these bytes were laid out by an
//! implementation that is not ours and was not consulted while writing this
//! one, so agreeing with them is evidence about the *format* rather than about
//! our own assumptions.
//!
//! What is checked is deliberately not "it parsed". The generator fills every
//! image with a deterministic pattern, so this recomputes that pattern and
//! compares byte for byte -- a reader that transposed the geometry, dropped a
//! channel or unshuffled with the wrong item size would parse happily and fail
//! here.

use std::path::PathBuf;

use xisf_core::{ErrorKind, Reader};

fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/generated")
}

/// The pattern `tools/corpus-gen/generate.cpp` fills images with.
fn expected_bytes(len: usize, bytes_per_sample: usize) -> Vec<u8> {
    (0..len).map(|i| ((i * 37 + (i / bytes_per_sample) * 11 + 1) & 0xff) as u8).collect()
}

fn bytes_per_sample(sample_format: &str) -> Option<usize> {
    Some(match sample_format {
        "UInt8" => 1,
        "UInt16" => 2,
        "UInt32" | "Float32" => 4,
        "UInt64" | "Float64" | "Complex32" => 8,
        "Complex64" => 16,
        _ => return None,
    })
}

#[test]
fn every_generated_file_reads_back_exactly() {
    let dir = corpus();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        eprintln!("skipping: {} not generated; see tools/corpus-gen", dir.display());
        return;
    };

    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "xisf"))
        .collect();
    files.sort();

    if files.is_empty() {
        eprintln!("skipping: no generated corpus files");
        return;
    }

    let (mut matched, mut unsupported) = (0usize, Vec::new());

    for path in &files {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let reader = Reader::open(path).unwrap_or_else(|e| panic!("{name}: open: {e}"));
        let image = *reader
            .header()
            .images()
            .first()
            .unwrap_or_else(|| panic!("{name}: no <Image> element"));

        let geometry = image.attr("geometry").unwrap_or_else(|| panic!("{name}: no geometry"));
        let sample_format =
            image.attr("sampleFormat").unwrap_or_else(|| panic!("{name}: no sampleFormat"));
        let width = bytes_per_sample(sample_format)
            .unwrap_or_else(|| panic!("{name}: unknown sampleFormat {sample_format}"));

        let samples: usize =
            geometry.split(':').map(|d| d.parse::<usize>().expect("geometry field")).product();
        let expected_len = samples * width;

        // A codec we do not implement must say so plainly rather than
        // producing wrong bytes. libXISF can write ZSTD, which XISF 1.0 does
        // not list among its standard codecs.
        let data = match reader.block(&image.data) {
            Ok(data) => data,
            Err(e) if e.kind() == ErrorKind::Unsupported => {
                unsupported.push((name.clone(), e.message().to_string()));
                continue;
            }
            Err(e) => panic!("{name}: block: {e}"),
        };

        assert_eq!(data.len(), expected_len, "{name}: wrong length for {geometry} {sample_format}");
        assert_eq!(
            &*data,
            &expected_bytes(expected_len, width)[..],
            "{name}: the bytes differ from what the generator wrote"
        );

        assert!(!reader.verify(&image.data).unwrap().is_failure(), "{name}: checksum mismatch");
        matched += 1;
    }

    eprintln!("generated corpus: {matched} of {} files matched byte for byte", files.len());
    for (name, why) in &unsupported {
        eprintln!("  unsupported: {name} -- {why}");
    }

    assert!(matched > 0, "no generated file was read");
    // Every codec XISF 1.0 lists as standard must be readable; only
    // non-standard ones may land in `unsupported`.
    for (name, _) in &unsupported {
        assert!(
            name.contains("zstd"),
            "{name} uses a standard XISF 1.0 codec and should have been read"
        );
    }
}

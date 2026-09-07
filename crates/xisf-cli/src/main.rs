//! `xisf` -- a command-line tool for inspecting XISF files.
//!
//! Argument parsing is hand-rolled rather than pulled from a crate. The whole
//! workspace is free of C dependencies and light on Rust ones, and a tool with
//! four subcommands and six flags does not justify reversing that.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use xisf::{Location, XisfFile};

const USAGE: &str = "\
xisftool -- inspect XISF (Extensible Image Serialization Format) files

USAGE:
    xisftool <command> [options] <file>...

COMMANDS:
    info      Summarise a file: its images, geometry and metadata
    header    Print the raw XML header
    verify    Check every recorded checksum
    dump      Write an image's pixel data to standard output

OPTIONS:
    -i, --image <n>   Which image to act on, for `dump` (default 0)
    -v, --verbose     Show more, including FITS keywords and properties
    -h, --help        Print this
    -V, --version     Print the version
";

fn main() -> ExitCode {
    // A tool whose output is routinely piped into `head` or `less` must treat
    // a closed pipe as the ordinary end of its work. Rust's default is to
    // ignore SIGPIPE so that writes fail as errors, which `println!` turns
    // into a panic -- so `xisftool header big.xisf | head` printed a panic
    // where it should have printed nothing. Restoring the default disposition
    // makes the process die quietly on the signal, as every other Unix tool
    // does.
    #[cfg(unix)]
    // SAFETY: setting a signal disposition before any thread is spawned and
    // before any output is written.
    unsafe {
        libc_signal_default();
    }

    match run() {
        Ok(code) => code,
        Err(message) => {
            eprintln!("xisftool: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Restore the default disposition for `SIGPIPE`.
///
/// Declared here rather than taken from `libc`, which this workspace does
/// without: it is one call with a stable ABI, and a dependency for it would
/// be carried by everyone building the tool.
#[cfg(unix)]
unsafe fn libc_signal_default() {
    unsafe extern "C" {
        fn signal(sig: i32, handler: usize) -> usize;
    }
    const SIGPIPE: i32 = 13;
    const SIG_DFL: usize = 0;
    unsafe {
        signal(SIGPIPE, SIG_DFL);
    }
}

struct Options {
    command: String,
    files: Vec<PathBuf>,
    image: usize,
    verbose: bool,
}

fn run() -> Result<ExitCode, String> {
    let mut args = std::env::args().skip(1);
    let mut command: Option<String> = None;
    let mut files = Vec::new();
    let mut image = 0usize;
    let mut verbose = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(ExitCode::SUCCESS);
            }
            "-V" | "--version" => {
                println!("xisftool {}", env!("CARGO_PKG_VERSION"));
                return Ok(ExitCode::SUCCESS);
            }
            "-v" | "--verbose" => verbose = true,
            "-i" | "--image" => {
                let value = args.next().ok_or("--image needs a number")?;
                image = value.parse().map_err(|_| format!("{value:?} is not an image index"))?;
            }
            other if other.starts_with('-') && other.len() > 1 => {
                return Err(format!("unknown option {other:?}; try --help"));
            }
            other if command.is_none() => command = Some(other.to_string()),
            other => files.push(PathBuf::from(other)),
        }
    }

    let Some(command) = command else {
        print!("{USAGE}");
        return Ok(ExitCode::FAILURE);
    };
    if files.is_empty() {
        return Err(format!("`{command}` needs at least one file"));
    }

    let options = Options { command, files, image, verbose };
    match options.command.as_str() {
        "info" => info(&options),
        "header" => header(&options),
        "verify" => verify(&options),
        "dump" => dump(&options),
        other => Err(format!("unknown command {other:?}; try --help")),
    }
}

fn open(path: &Path) -> Result<XisfFile, String> {
    XisfFile::open(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn info(options: &Options) -> Result<ExitCode, String> {
    for (index, path) in options.files.iter().enumerate() {
        if index > 0 {
            println!();
        }
        let file = open(path)?;
        println!("{}", path.display());

        let images = file.images();
        println!("  {} image(s)", images.len());
        for (n, image) in images.iter().enumerate() {
            let geometry: Vec<String> = image.geometry().iter().map(u64::to_string).collect();
            println!(
                "  [{n}] {}, {} channel(s), {}, {}",
                geometry.join(" x "),
                image.channels(),
                image.sample_format().name(),
                image.color_space().name()
            );

            let stored = match image.location() {
                Some(Location::Attachment { position, size }) => {
                    format!("attached at {position}, {size} bytes")
                }
                Some(Location::Embedded) => "embedded".to_string(),
                Some(Location::Inline { encoding }) => format!("inline, {encoding:?}"),
                Some(Location::Path { path, .. }) => format!("external file {path}"),
                Some(Location::Url { url, .. }) => format!("external URL {url}"),
                None => "no data block".to_string(),
            };
            println!("       {stored}");
            println!(
                "       {} byte order{}{}",
                if image.byte_order() == xisf::ByteOrder::Big {
                    "big-endian"
                } else {
                    "little-endian"
                },
                if image.is_compressed() { ", compressed" } else { "" },
                image.bounds().map_or(String::new(), |b| format!(", bounds {}:{}", b.low, b.high))
            );

            if let Some(offset) = image.attributes().offset {
                println!("       pedestal offset {offset}");
            }
            if let Some(orientation) = image.attributes().orientation
                && !orientation.is_identity()
            {
                println!("       orientation {}", orientation.to_attribute());
            }
            if let Some(resolution) = image.resolution() {
                let (x, y) = resolution.per_inch();
                println!(
                    "       resolution {} x {} per {}{}",
                    resolution.horizontal,
                    resolution.vertical,
                    resolution.unit.name(),
                    if resolution.unit == xisf::ResolutionUnit::Inch {
                        String::new()
                    } else {
                        format!(" ({x:.1} x {y:.1} ppi)")
                    }
                );
            }
            if let Some(thumbnail) = image.thumbnail() {
                let geometry: Vec<String> =
                    thumbnail.geometry().iter().map(u64::to_string).collect();
                println!(
                    "       thumbnail {}, {} channel(s), {}",
                    geometry.join(" x "),
                    thumbnail.channels(),
                    thumbnail.sample_format().name()
                );
            }
            if let Some(cfa) = image.color_filter_array() {
                println!(
                    "       CFA {}x{} {}{}",
                    cfa.width,
                    cfa.height,
                    cfa.pattern_string(),
                    cfa.name.as_deref().map_or(String::new(), |n| format!(" ({n})"))
                );
            }
            if let Some(profile) = image.icc_profile() {
                match profile {
                    Ok(bytes) => println!("       ICC profile, {} bytes", bytes.len()),
                    Err(e) => println!("       ICC profile: unreadable ({e})"),
                }
            }
            if let Some(space) = image.rgb_working_space() {
                let gamma = match space.gamma {
                    xisf::Gamma::Srgb => "sRGB".to_string(),
                    xisf::Gamma::Exponent(g) => format!("gamma {g}"),
                };
                println!(
                    "       RGBWS {}{gamma}",
                    space.name.as_deref().map_or(String::new(), |n| format!("{n}, "))
                );
            }
            if let Some(df) = image.display_function() {
                println!(
                    "       display function{}{}",
                    df.name.as_deref().map_or(String::new(), |n| format!(" {n}")),
                    if df.is_identity() { " (identity)" } else { "" }
                );
            }
            for table in image.tables() {
                println!(
                    "       table {}, {} row(s) x {} column(s)",
                    table.id,
                    table.rows.len(),
                    table.structure.fields.len()
                );
            }

            if options.verbose {
                for (name, value, comment) in image.fits_keywords() {
                    let comment =
                        if comment.is_empty() { String::new() } else { format!("  / {comment}") };
                    println!("       FITS {name:<10} {value}{comment}");
                }
            }
        }

        let properties = file.properties();
        if !properties.is_empty() {
            println!(
                "  {} propert{}",
                properties.len(),
                if properties.len() == 1 { "y" } else { "ies" }
            );
            if options.verbose {
                for property in &properties {
                    println!(
                        "       {} ({}) = {}",
                        property.id(),
                        property.kind().name(),
                        describe(property)
                    );
                }
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// A one-line rendering of a property's value.
///
/// A vector or matrix has no textual form, and a long string may be a data
/// block rather than character data -- one file in the corpus carries several
/// kilobytes of base64 processing history. Printing those in full turns `info`
/// into a dump, so their shape is shown instead and `xisftool dump` is there for
/// anyone who wants the bytes.
fn describe(property: &xisf::PropertyRef<'_>) -> String {
    const LIMIT: usize = 80;

    match property.as_str() {
        Some(text) if text.chars().count() <= LIMIT => text.to_string(),
        Some(text) => {
            let head: String = text.chars().take(LIMIT).collect();
            format!("{head}... ({} characters)", text.chars().count())
        }
        None => property
            .attributes()
            .component_count()
            .map_or_else(|| "<binary>".to_string(), |n| format!("<{n} components>")),
    }
}

fn header(options: &Options) -> Result<ExitCode, String> {
    for path in &options.files {
        let file = open(path)?;
        let text = file.reader().header_text().map_err(|e| format!("{}: {e}", path.display()))?;
        println!("{text}");
    }
    Ok(ExitCode::SUCCESS)
}

fn verify(options: &Options) -> Result<ExitCode, String> {
    let mut failures = 0usize;
    let mut checked = 0usize;
    let mut absent = 0usize;

    for path in &options.files {
        let file = open(path)?;
        for (n, image) in file.images().iter().enumerate() {
            match image.verify() {
                Ok(xisf::ChecksumStatus::Valid) => {
                    checked += 1;
                    if options.verbose {
                        println!("{}: [{n}] ok", path.display());
                    }
                }
                Ok(xisf::ChecksumStatus::Absent) => {
                    absent += 1;
                    if options.verbose {
                        println!("{}: [{n}] no checksum recorded", path.display());
                    }
                }
                Ok(xisf::ChecksumStatus::Invalid) => {
                    failures += 1;
                    println!("{}: [{n}] CHECKSUM MISMATCH", path.display());
                }
                Err(e) => {
                    failures += 1;
                    println!("{}: [{n}] could not verify: {e}", path.display());
                }
            }
        }
    }

    // A file with no checksums is not a pass, and saying "ok" would imply
    // something was checked. The counts say what actually happened.
    println!("{checked} verified, {absent} without a checksum, {failures} failed");
    Ok(if failures == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}

fn dump(options: &Options) -> Result<ExitCode, String> {
    let path = &options.files[0];
    let file = open(path)?;
    let images = file.images();
    let image = images
        .get(options.image)
        .ok_or_else(|| format!("{} has no image {}", path.display(), options.image))?;

    let data = image.bytes().map_err(|e| format!("{}: {e}", path.display()))?;
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(&data).map_err(|e| e.to_string())?;
    stdout.flush().map_err(|e| e.to_string())?;
    Ok(ExitCode::SUCCESS)
}

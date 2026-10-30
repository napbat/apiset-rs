//! Download each `apisetschema.dll` of Windows 10 and Windows 11.
//!
//! The tool reads the Winbindex index of `apisetschema.dll`, downloads each file from the
//! Microsoft symbol server, and verifies its SHA-256. With `--rebuild`, it rebuilds the files
//! that the symbol server does not have from update packages and install media. It writes the
//! files and a `manifest.json` into the output directory. A second run downloads only the
//! missing files.
//!
//! ```text
//! cargo run --manifest-path tools/apiset-fetch/Cargo.toml -- --out apiset-cache --check
//! cargo run --manifest-path tools/apiset-fetch/Cargo.toml -- --release 11-24H2 --rebuild
//! cargo run --manifest-path tools/apiset-fetch/Cargo.toml -- --list
//! ```

mod base;
mod catalog;
mod download;
mod http;
mod package;
mod rebuild;
mod winbindex;

use download::{Check, Outcome};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use winbindex::Record;

/// Result type of the tool.
pub type Result<T, E = Box<dyn std::error::Error + Send + Sync>> = std::result::Result<T, E>;

/// Return `bytes` as lowercase hex.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
            // Writing to a `String` cannot fail.
            let _ = write!(out, "{byte:02x}");
            out
        })
}

/// The usage text.
const USAGE: &str = "\
Usage: apiset-fetch [options]

Download each apisetschema.dll that Winbindex lists from the Microsoft symbol server.

Options:
  --out DIR          Write the files and manifest.json to DIR (default: apiset-cache)
  --release NAME     Download only files of this Windows release, for example 1607 or
                     11-24H2. Use the option more than one time for more releases.
  --jobs N           Download N files at the same time (default: 8)
  --check            Parse the schema in each file, and record its version and entry count
  --rebuild          Rebuild the files that the symbol server does not have from update
                     packages (Microsoft Update Catalog) and install media (UUP dump)
  --list             Print the files and their releases. Do not download.
  --index-url URL    Read the Winbindex index from URL
  -h, --help         Print this text";

/// The command-line options.
#[derive(Debug)]
struct Options {
    out: PathBuf,
    releases: Vec<String>,
    jobs: usize,
    check: bool,
    rebuild: bool,
    list: bool,
    index_url: String,
}

/// Parse the command-line arguments. Return `None` for `--help`.
fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Option<Options>> {
    let mut options = Options {
        out: PathBuf::from("apiset-cache"),
        releases: Vec::new(),
        jobs: 8,
        check: false,
        rebuild: false,
        list: false,
        index_url: winbindex::DEFAULT_INDEX_URL.to_owned(),
    };
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--out" => options.out = PathBuf::from(value()?),
            "--release" => options.releases.push(value()?),
            "--jobs" => {
                options.jobs = value()?.parse()?;
                if options.jobs == 0 {
                    return Err("--jobs must be at least 1".into());
                }
            }
            "--check" => options.check = true,
            "--rebuild" => options.rebuild = true,
            "--list" => options.list = true,
            "--index-url" => options.index_url = value()?,
            "-h" | "--help" => return Ok(None),
            _ => return Err(format!("unknown argument {arg:?}").into()),
        }
    }
    Ok(Some(options))
}

/// One line of `manifest.json`.
#[derive(Debug, Serialize)]
struct ManifestEntry<'a> {
    file: String,
    #[serde(flatten)]
    record: &'a Record,
    #[serde(flatten)]
    outcome: Outcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    check: Option<Check>,
    #[serde(skip_serializing_if = "Option::is_none")]
    check_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rebuild_error: Option<String>,
}

impl ManifestEntry<'_> {
    /// Return `true` if the download, the hash check or the schema check failed.
    fn failed(&self) -> bool {
        matches!(self.outcome, Outcome::Failed(_)) || self.check_error.is_some()
    }
}

/// Print each record and its releases.
fn print_list(records: &[&Record]) {
    for record in records {
        let mut releases: Vec<&str> = record.sources.iter().map(|s| s.release.as_str()).collect();
        releases.dedup();
        let available = if record.symbol_server_url().is_some() {
            ""
        } else {
            " (not downloadable)"
        };
        println!("{}  {}{available}", record.file_name(), releases.join(", "));
    }
}

/// Download, and optionally check, each record with `jobs` threads.
fn process<'a>(
    agent: &ureq::Agent,
    records: &[&'a Record],
    options: &Options,
) -> Vec<ManifestEntry<'a>> {
    let next = AtomicUsize::new(0);
    let total = records.len();
    let mut entries: Vec<(usize, ManifestEntry<'a>)> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..options.jobs.min(total.max(1)))
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(&record) = records.get(index) else {
                            break;
                        };
                        let entry = process_one(agent, record, &options.out, options.check);
                        print_entry(index + 1, total, &entry);
                        done.push((index, entry));
                    }
                    done
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().unwrap_or_default())
            .collect()
    });
    entries.sort_by_key(|(index, _)| *index);
    entries.into_iter().map(|(_, entry)| entry).collect()
}

/// Download and optionally check one record.
fn process_one<'a>(
    agent: &ureq::Agent,
    record: &'a Record,
    dir: &Path,
    check: bool,
) -> ManifestEntry<'a> {
    let outcome = download::fetch(agent, record, dir);
    let present = matches!(outcome, Outcome::Downloaded | Outcome::Cached);
    let (check, check_error) = if check && present {
        match download::check(record, dir) {
            Ok(summary) => (Some(summary), None),
            Err(e) => (None, Some(e.to_string())),
        }
    } else {
        (None, None)
    };
    ManifestEntry {
        file: record.file_name(),
        record,
        outcome,
        check,
        check_error,
        rebuild_error: None,
    }
}

/// Read each file in `dir` into a map by SHA-256.
fn cached_files(dir: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut files = BTreeMap::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("dll"))
        {
            let bytes = std::fs::read(&path)?;
            files.insert(download::sha256_hex(&bytes), bytes);
        }
    }
    Ok(files)
}

/// Rebuild the files that the symbol server does not have, and update their entries.
fn rebuild_missing(
    agent: &ureq::Agent,
    records: &[Record],
    entries: &mut [ManifestEntry<'_>],
    options: &Options,
) -> Result<()> {
    let targets: Vec<String> = entries
        .iter()
        .filter(|e| matches!(e.outcome, Outcome::Unavailable(_)))
        .map(|e| e.record.sha256.clone())
        .collect();
    if targets.is_empty() {
        return Ok(());
    }
    println!("rebuilding {} file(s)", targets.len());
    let known = cached_files(&options.out)?;
    let mut rebuilder = rebuild::Rebuilder::new(agent, &options.out, records, known);
    let results = rebuilder.run(&targets);
    for entry in entries.iter_mut() {
        match results.get(&entry.record.sha256) {
            Some(rebuild::Rebuilt::Rebuilt) => {
                entry.outcome = Outcome::Rebuilt;
                if options.check {
                    match download::check(entry.record, &options.out) {
                        Ok(summary) => entry.check = Some(summary),
                        Err(e) => entry.check_error = Some(e.to_string()),
                    }
                }
                print_entry(0, 0, entry);
            }
            Some(rebuild::Rebuilt::Failed(reason)) => entry.rebuild_error = Some(reason.clone()),
            None => {}
        }
    }
    Ok(())
}

/// Print the progress line of one entry.
fn print_entry(number: usize, total: usize, entry: &ManifestEntry<'_>) {
    let status = match &entry.outcome {
        Outcome::Downloaded => "downloaded".to_owned(),
        Outcome::Cached => "cached".to_owned(),
        Outcome::Unavailable(reason) => format!("unavailable: {reason}"),
        Outcome::Rebuilt => "rebuilt".to_owned(),
        Outcome::Failed(reason) => format!("FAILED: {reason}"),
    };
    let check = match (&entry.check, &entry.check_error) {
        (Some(c), _) => format!(" (schema v{}, {} entries)", c.schema_version, c.entries),
        (None, Some(e)) => format!(" (CHECK FAILED: {e})"),
        (None, None) => String::new(),
    };
    println!("[{number}/{total}] {} {status}{check}", entry.file);
}

/// Run the tool.
fn run(options: &Options) -> Result<bool> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(120)))
        .user_agent("apiset-fetch (https://github.com/napbat/apiset-rs)")
        .build()
        .into();

    let records = winbindex::fetch_index(&agent, &options.index_url)?;
    let selected: Vec<&Record> = records
        .iter()
        .filter(|record| {
            options.releases.is_empty() || options.releases.iter().any(|r| record.is_in_release(r))
        })
        .collect();
    if options.list {
        print_list(&selected);
        return Ok(true);
    }
    if selected.is_empty() {
        return Err("no file matches the selected releases".into());
    }

    std::fs::create_dir_all(&options.out)?;
    let mut entries = process(&agent, &selected, options);
    if options.rebuild {
        rebuild_missing(&agent, &records, &mut entries, options)?;
    }
    let manifest = options.out.join("manifest.json");
    std::fs::write(&manifest, serde_json::to_vec_pretty(&entries)?)?;

    let failed = entries.iter().filter(|e| e.failed()).count();
    let count = |status: &Outcome| entries.iter().filter(|e| &e.outcome == status).count();
    let unavailable = entries
        .iter()
        .filter(|e| matches!(e.outcome, Outcome::Unavailable(_)))
        .count();
    println!(
        "{} downloaded, {} cached, {} rebuilt, {unavailable} unavailable, {failed} failed. \
         Manifest: {}",
        count(&Outcome::Downloaded),
        count(&Outcome::Cached),
        count(&Outcome::Rebuilt),
        manifest.display()
    );
    Ok(failed == 0)
}

fn main() -> ExitCode {
    let options = match parse_args(std::env::args().skip(1)) {
        Ok(Some(options)) => options,
        Ok(None) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("error: {e}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    match run(&options) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

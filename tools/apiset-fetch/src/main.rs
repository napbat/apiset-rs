//! Download each `apisetschema.dll` of Windows 10 and Windows 11.
//!
//! The tool reads the Winbindex index of `apisetschema.dll`, downloads each file from the
//! Microsoft symbol server, and verifies its SHA-256. It writes the files and a
//! `manifest.json` into the output directory. A second run downloads only the missing files.
//!
//! ```text
//! cargo run --manifest-path tools/apiset-fetch/Cargo.toml -- --out apiset-cache --check
//! cargo run --manifest-path tools/apiset-fetch/Cargo.toml -- --release 11-24H2 --release 1607
//! cargo run --manifest-path tools/apiset-fetch/Cargo.toml -- --list
//! ```

mod download;
mod winbindex;

use download::{Check, Outcome};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use winbindex::Record;

/// Result type of the tool.
pub type Result<T, E = Box<dyn std::error::Error + Send + Sync>> = std::result::Result<T, E>;

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
    }
}

/// Print the progress line of one entry.
fn print_entry(number: usize, total: usize, entry: &ManifestEntry<'_>) {
    let status = match &entry.outcome {
        Outcome::Downloaded => "downloaded".to_owned(),
        Outcome::Cached => "cached".to_owned(),
        Outcome::Unavailable(reason) => format!("unavailable: {reason}"),
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
    let entries = process(&agent, &selected, options);
    let manifest = options.out.join("manifest.json");
    std::fs::write(&manifest, serde_json::to_vec_pretty(&entries)?)?;

    let failed = entries.iter().filter(|e| e.failed()).count();
    let count = |status: &Outcome| entries.iter().filter(|e| &e.outcome == status).count();
    let unavailable = entries
        .iter()
        .filter(|e| matches!(e.outcome, Outcome::Unavailable(_)))
        .count();
    println!(
        "{} downloaded, {} cached, {unavailable} unavailable, {failed} failed. Manifest: {}",
        count(&Outcome::Downloaded),
        count(&Outcome::Cached),
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

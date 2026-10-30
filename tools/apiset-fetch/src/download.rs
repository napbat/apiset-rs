//! Download a file from the symbol server, verify its hash, and keep it in the cache.

use crate::Result;
use crate::winbindex::Record;
use apiset_rs::ApiSetSchema;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

/// The largest file that the tool accepts. `apisetschema.dll` is smaller than 1 MiB.
const MAX_FILE_SIZE: u64 = 16 * 1024 * 1024;

/// The result of one file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "status", content = "reason")]
pub enum Outcome {
    /// The tool downloaded the file.
    Downloaded,
    /// The cache already had the file with the correct hash.
    Cached,
    /// The symbol server does not have the file.
    Unavailable(String),
    /// The tool rebuilt the file from an update package (`--rebuild`).
    Rebuilt,
    /// The download or the hash check failed.
    Failed(String),
}

/// The schema summary of a downloaded file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    /// The schema version.
    pub schema_version: u32,
    /// The number of entries.
    pub entries: usize,
}

/// Return the lowercase hex SHA-256 of `data`.
pub fn sha256_hex(data: &[u8]) -> String {
    crate::hex(&Sha256::digest(data))
}

/// Download one record into `dir`, or keep the cached file.
pub fn fetch(agent: &ureq::Agent, record: &Record, dir: &Path) -> Outcome {
    let path = dir.join(record.file_name());
    if let Ok(cached) = fs::read(&path)
        && sha256_hex(&cached) == record.sha256
    {
        return Outcome::Cached;
    }
    let Some(url) = record.symbol_server_url() else {
        return Outcome::Unavailable(
            "Winbindex has no TimeDateStamp and SizeOfImage for the file".into(),
        );
    };
    match download(agent, &url) {
        Ok(data) => {
            let actual = sha256_hex(&data);
            if actual != record.sha256 {
                // Different builds can have the same TimeDateStamp and SizeOfImage. The symbol
                // server keeps one file for each key, so it can return the file of a
                // different build.
                return Outcome::Unavailable(format!(
                    "the symbol server has a different file for the key (SHA-256 {actual})"
                ));
            }
            match write_atomic(&path, &data) {
                Ok(()) => Outcome::Downloaded,
                Err(e) => Outcome::Failed(format!("{}: {e}", path.display())),
            }
        }
        Err(ureq::Error::StatusCode(404)) => {
            Outcome::Unavailable("the symbol server has no file for the key".into())
        }
        Err(e) => Outcome::Failed(format!("{url}: {e}")),
    }
}

/// Download the body of `url`.
fn download(agent: &ureq::Agent, url: &str) -> Result<Vec<u8>, ureq::Error> {
    agent
        .get(url)
        .call()?
        .body_mut()
        .with_config()
        .limit(MAX_FILE_SIZE)
        .read_to_vec()
}

/// Write `data` to a temporary file, then rename it, so that the cache never has a part file.
fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let partial = path.with_extension("part");
    fs::write(&partial, data)?;
    fs::rename(&partial, path)
}

/// Parse the schema in a cached file.
///
/// # Errors
///
/// Returns an error if the file cannot be read or the schema does not parse.
pub fn check(record: &Record, dir: &Path) -> Result<Check> {
    let file = fs::read(dir.join(record.file_name()))?;
    let schema = ApiSetSchema::from_pe(&file)?;
    Ok(Check {
        schema_version: schema.version().as_u32(),
        entries: schema.entries().len(),
    })
}

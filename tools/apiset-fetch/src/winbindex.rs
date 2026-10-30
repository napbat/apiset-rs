//! Read the Winbindex index of `apisetschema.dll` files.
//!
//! Winbindex (<https://winbindex.m417z.com>) lists each `apisetschema.dll` that Windows 10 and
//! Windows 11 shipped, with the Windows releases and the updates that contain it. For most
//! files, it also has the PE `TimeDateStamp` and `SizeOfImage`. The Microsoft symbol server
//! stores each such file under these two values.

use crate::Result;
use serde::Serialize;
use serde_json::Value;
use std::io::Read;

/// The URL of the compressed Winbindex index for `apisetschema.dll`.
pub const DEFAULT_INDEX_URL: &str =
    "https://winbindex.m417z.com/data/by_filename_compressed/apisetschema.dll.json.gz";

/// The base URL of the Microsoft symbol server.
pub const SYMBOL_SERVER: &str = "https://msdl.microsoft.com/download/symbols";

/// One Windows update that contains a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Source {
    /// The Windows release, for example `1607` or `11-24H2`.
    pub release: String,
    /// The update: a KB number, or `BASE` for the release media.
    pub update: String,
    /// The OS build of the update, for example `14393.3204`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os_build: Option<String>,
    /// The release date of the update or of the release media.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    /// The names of the assemblies that contain the file. The name tells the edition, for
    /// example `Microsoft-Windows-ApiSetSchema-server`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub assemblies: Vec<String>,
    /// The component directories of the assemblies, for example
    /// `amd64_microsoft-windows-apisetschema-wnc_31bf3856ad364e35_10.0.26100.9549_none_6d8461b9bafd9b1b`.
    /// An update payload stores the file below this directory.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<String>,
}

/// One distinct `apisetschema.dll` file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Record {
    /// The SHA-256 of the file, in lowercase hex.
    pub sha256: String,
    /// The file version, for example `10.0.14393.3204`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The COFF `Machine` value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub machine: Option<u16>,
    /// The file size, in bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// The PE `TimeDateStamp`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<u32>,
    /// The PE `SizeOfImage`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub virtual_size: Option<u32>,
    /// The Windows updates that contain the file.
    pub sources: Vec<Source>,
}

impl Record {
    /// Return the symbol server URL of the file, or `None` if Winbindex does not know the
    /// `TimeDateStamp` and the `SizeOfImage`.
    ///
    /// The symbol server key is the `TimeDateStamp` as 8 uppercase hex digits, followed by
    /// `SizeOfImage` as lowercase hex digits with no padding.
    pub fn symbol_server_url(&self) -> Option<String> {
        let (timestamp, size) = (self.timestamp?, self.virtual_size?);
        Some(format!(
            "{SYMBOL_SERVER}/apisetschema.dll/{timestamp:08X}{size:x}/apisetschema.dll"
        ))
    }

    /// Return the file name in the cache: the file version and the start of the SHA-256.
    ///
    /// Two editions can ship different files with the same version, so the name contains the
    /// hash.
    pub fn file_name(&self) -> String {
        let version: String = self
            .version
            .as_deref()
            .unwrap_or("unknown")
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
            .collect();
        let hash = self.sha256.get(..12).unwrap_or(&self.sha256);
        format!("apisetschema-{version}-{hash}.dll")
    }

    /// Return `true` if one of the sources is from `release`.
    pub fn is_in_release(&self, release: &str) -> bool {
        self.sources
            .iter()
            .any(|source| source.release.eq_ignore_ascii_case(release))
    }
}

/// Download the compressed index and parse it.
///
/// # Errors
///
/// Returns an error if the download, the decompression or the parse fails.
pub fn fetch_index(agent: &ureq::Agent, url: &str) -> Result<Vec<Record>> {
    let compressed = agent
        .get(url)
        .call()?
        .body_mut()
        .with_config()
        .limit(64 * 1024 * 1024)
        .read_to_vec()?;
    let mut json = Vec::new();
    flate2::read::GzDecoder::new(compressed.as_slice()).read_to_end(&mut json)?;
    parse_index(&json)
}

/// Parse the JSON of a Winbindex index. The records are sorted by version and hash.
///
/// # Errors
///
/// Returns an error if the JSON is not an object of records.
pub fn parse_index(json: &[u8]) -> Result<Vec<Record>> {
    let index: Value = serde_json::from_slice(json)?;
    let Value::Object(files) = index else {
        return Err("the Winbindex index is not a JSON object".into());
    };
    let mut records: Vec<Record> = files
        .iter()
        .map(|(sha256, record)| parse_record(sha256, record))
        .collect();
    records.sort_by(|a, b| {
        version_key(a.version.as_deref())
            .cmp(&version_key(b.version.as_deref()))
            .then_with(|| a.sha256.cmp(&b.sha256))
    });
    Ok(records)
}

/// Return the numbers of a dotted version, so that `10.0.9` sorts before `10.0.10`. A record
/// with no version sorts first.
fn version_key(version: Option<&str>) -> Vec<u64> {
    version.map_or_else(Vec::new, |version| {
        version
            .split('.')
            .map(|part| part.parse().unwrap_or(u64::MAX))
            .collect()
    })
}

/// Return a field of a JSON object as `u32`.
fn u32_field(object: &Value, name: &str) -> Option<u32> {
    object.get(name)?.as_u64()?.try_into().ok()
}

/// Return a field of a JSON object as a string.
fn str_field(object: &Value, name: &str) -> Option<String> {
    object.get(name)?.as_str().map(str::to_owned)
}

/// Parse one record. The key of the record is the SHA-256 of the file.
fn parse_record(sha256: &str, record: &Value) -> Record {
    let info = &record["fileInfo"];
    let mut sources = Vec::new();
    if let Some(releases) = record["windowsVersions"].as_object() {
        for (release, updates) in releases {
            let Some(updates) = updates.as_object() else {
                continue;
            };
            for (update, details) in updates {
                sources.push(parse_source(release, update, details));
            }
        }
    }
    Record {
        sha256: sha256.to_ascii_lowercase(),
        version: str_field(info, "version")
            .and_then(|v| v.split_whitespace().next().map(str::to_owned)),
        machine: info["machineType"].as_u64().and_then(|m| m.try_into().ok()),
        size: info["size"].as_u64(),
        timestamp: u32_field(info, "timestamp"),
        virtual_size: u32_field(info, "virtualSize"),
        sources,
    }
}

/// Parse one update of one release.
fn parse_source(release: &str, update: &str, details: &Value) -> Source {
    let update_info = &details["updateInfo"];
    let entries = details["assemblies"].as_object();
    let mut assemblies: Vec<String> = entries
        .into_iter()
        .flat_map(|assemblies| assemblies.values())
        .filter_map(|assembly| str_field(&assembly["assemblyIdentity"], "name"))
        .collect();
    assemblies.sort();
    assemblies.dedup();
    let mut components: Vec<String> = entries
        .into_iter()
        .flat_map(|assemblies| assemblies.keys().cloned())
        .collect();
    components.sort();
    Source {
        release: release.to_owned(),
        update: update.to_owned(),
        os_build: str_field(update_info, "releaseVersion"),
        date: str_field(update_info, "releaseDate")
            .or_else(|| str_field(&details["windowsVersionInfo"], "releaseDate")),
        assemblies,
        components,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "BBBB0000000000000000": {
            "fileInfo": {
                "machineType": 34404,
                "size": 116256,
                "timestamp": 1950489339,
                "version": "10.0.14393.3204 (rs1_release.190830-1500)",
                "virtualSize": 118784
            },
            "windowsVersions": {
                "1607": {
                    "KB4516044": {
                        "assemblies": {
                            "amd64_server": {
                                "assemblyIdentity": {
                                    "name": "Microsoft-Windows-ApiSetSchema-server"
                                }
                            }
                        },
                        "updateInfo": {
                            "releaseDate": "2019-09-10",
                            "releaseVersion": "14393.3204"
                        }
                    }
                }
            }
        },
        "aaaa0000000000000000": {
            "fileInfo": { "size": 148864, "timestamp": 5 },
            "windowsVersions": {
                "1511": {
                    "BASE": { "windowsVersionInfo": { "releaseDate": "2015-11-10" } }
                }
            }
        },
        "cccc0000000000000000": {
            "fileInfo": { "version": "10.0.9600.1 (x)", "timestamp": 1, "virtualSize": 2 },
            "windowsVersions": {}
        }
    }"#;

    #[test]
    fn records_have_the_symbol_server_key_and_the_sources() {
        let records = parse_index(SAMPLE.as_bytes()).unwrap();
        let versions: Vec<_> = records.iter().map(|r| r.version.as_deref()).collect();
        assert_eq!(
            versions,
            [None, Some("10.0.9600.1"), Some("10.0.14393.3204")],
            "the records sort by the numbers of the version"
        );

        let server = &records[2];
        assert_eq!(server.sha256, "bbbb0000000000000000");
        assert_eq!(
            server.symbol_server_url().as_deref(),
            Some(
                "https://msdl.microsoft.com/download/symbols/apisetschema.dll/74421AFB1d000/apisetschema.dll"
            )
        );
        assert_eq!(
            server.file_name(),
            "apisetschema-10.0.14393.3204-bbbb00000000.dll"
        );
        assert_eq!(
            server.sources,
            [Source {
                release: "1607".into(),
                update: "KB4516044".into(),
                os_build: Some("14393.3204".into()),
                date: Some("2019-09-10".into()),
                assemblies: vec!["Microsoft-Windows-ApiSetSchema-server".into()],
                components: vec!["amd64_server".into()],
            }]
        );
        assert!(server.is_in_release("1607"));
        assert!(!server.is_in_release("1511"));

        // The symbol server key pads the timestamp to 8 digits.
        assert_eq!(
            records[1].symbol_server_url().as_deref(),
            Some(
                "https://msdl.microsoft.com/download/symbols/apisetschema.dll/000000012/apisetschema.dll"
            )
        );

        let base = &records[0];
        assert_eq!(base.symbol_server_url(), None);
        assert_eq!(base.file_name(), "apisetschema-unknown-aaaa00000000.dll");
        assert_eq!(base.sources[0].date.as_deref(), Some("2015-11-10"));
    }
}

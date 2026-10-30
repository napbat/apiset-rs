//! Find the release (`BASE`) version of a file in the install media.
//!
//! An update delta changes the file of an earlier build. The first build of each release is
//! not in an update, and the symbol server does not have all of them. The UUP dump service
//! (<https://uupdump.net>) lists the Microsoft download URLs of the install media of a build:
//! a set of ESD files (solid LZMS WIM files). The tool reads only the directory of each ESD
//! file and the data of the `apisetschema.dll` files in it.

use crate::Result;
use crate::http::HttpFile;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::thread::sleep;
use std::time::Duration;
use winupdate::Wim;

/// The UUP dump API.
const UUP_API: &str = "https://api.uupdump.net";

/// Return the build numbers whose install media can hold the files of a Winbindex release.
///
/// The first number is the build of the release. A release that is an enablement package
/// also lists the build of the release that it enables, because UUP dump can list its media
/// under either number.
pub fn media_builds(release: &str) -> Vec<u32> {
    match release {
        "1507" => vec![10240],
        "1511" => vec![10586],
        "1607" => vec![14393],
        "1703" => vec![15063],
        "1709" => vec![16299],
        "1803" => vec![17134],
        "1809" => vec![17763],
        "1903" => vec![18362],
        "1909" => vec![18363, 18362],
        "2004" => vec![19041],
        "20H2" => vec![19042, 19041],
        "21H1" => vec![19043, 19041],
        "21H2" => vec![19044, 19041],
        "22H2" => vec![19045, 19041],
        "11-21H2" => vec![22000],
        "11-22H2" => vec![22621],
        "11-23H2" => vec![22631, 22621],
        "11-24H2" => vec![26100],
        "11-25H2" => vec![26200, 26100],
        "11-26H1" => vec![28000],
        _ => Vec::new(),
    }
}

/// Call the UUP dump API. Wait and retry when the service limits the request rate.
fn uup(agent: &ureq::Agent, path: &str, query: &[(&str, &str)]) -> Result<Value> {
    let mut wait = Duration::from_secs(5);
    for _ in 0..6 {
        // The service allows a small number of requests per minute.
        sleep(Duration::from_secs(2));
        let mut request = agent.get(format!("{UUP_API}/{path}"));
        for (key, value) in query {
            request = request.query(*key, *value);
        }
        match request.call() {
            Ok(mut response) => {
                let value: Value = serde_json::from_slice(&response.body_mut().read_to_vec()?)?;
                return Ok(value);
            }
            Err(ureq::Error::StatusCode(429)) => {
                sleep(wait);
                wait *= 2;
            }
            Err(e) => return Err(e.into()),
        }
    }
    Err(format!("{path}: the UUP dump service limits the request rate").into())
}

/// The kind of install media.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Media {
    /// Client media, for example "Windows 11, version 24H2 (...)".
    Client,
    /// Server media, for example "Windows Server 2025 (...)". Server, Nano Server and utility
    /// VM editions of a file are only in these.
    Server,
}

/// Return the UUP IDs of the amd64 install media of a build: the best client media and the
/// best server media.
fn media_ids(agent: &ureq::Agent, build: u32) -> Result<Vec<(Media, String)>> {
    let response = uup(agent, "listid.php", &[("search", &build.to_string())])?;
    let builds = &response["response"]["builds"];
    let entries: Vec<&Value> = match builds {
        Value::Object(map) => map.values().collect(),
        Value::Array(list) => list.iter().collect(),
        _ => Vec::new(),
    };
    let prefix = format!("{build}.");
    let mut candidates: Vec<(Media, bool, String)> = entries
        .into_iter()
        .filter(|entry| entry["arch"] == "amd64")
        .filter(|entry| {
            entry["build"]
                .as_str()
                .is_some_and(|b| b.starts_with(&prefix))
        })
        .filter_map(|entry| {
            let title = entry["title"].as_str()?;
            // Updates have other titles, for example "Cumulative Update for ..." or
            // "Update for Microsoft server operating system (...)".
            let media = if title.starts_with("Windows Server") {
                Media::Server
            } else if title.starts_with("Windows 1") || title.starts_with("Feature update") {
                Media::Client
            } else {
                return None;
            };
            Some((
                media,
                title.contains("Insider"),
                entry["uuid"].as_str()?.to_owned(),
            ))
        })
        .collect();
    // Prefer retail media to Insider media, and keep one media set of each kind.
    candidates.sort();
    candidates.dedup_by_key(|(media, _, _)| *media);
    Ok(candidates
        .into_iter()
        .map(|(media, _, id)| (media, id))
        .collect())
}

/// Return the ESD files of a UUP ID as (name, URL), the most likely holders first.
fn esd_files(agent: &ureq::Agent, media: Media, id: &str) -> Result<Vec<(String, String)>> {
    // The client media of one edition has the shared packages. Server media lists all its
    // editions.
    let mut query = vec![("id", id), ("lang", "en-us")];
    if media == Media::Client {
        query.push(("edition", "professional"));
    }
    let response = uup(agent, "get.php", &query)?;
    let Some(files) = response["response"]["files"].as_object() else {
        return Ok(Vec::new());
    };
    let mut esds: Vec<(u8, u64, String, String)> = files
        .iter()
        .filter(|(name, _)| name.to_ascii_lowercase().ends_with(".esd"))
        .filter_map(|(name, file)| {
            let url = file["url"].as_str()?.to_owned();
            let size = file["size"]
                .as_str()
                .and_then(|s| s.parse().ok())
                .or_else(|| file["size"].as_u64())
                .unwrap_or(u64::MAX);
            let lower = name.to_ascii_lowercase();
            let rank = if lower.contains("desktop-required") || lower.contains("onecore") {
                0
            } else if lower.starts_with("professional") || lower.contains("core") {
                1
            } else {
                2
            };
            Some((rank, size, name.clone(), url))
        })
        .collect();
    esds.sort();
    Ok(esds
        .into_iter()
        .map(|(_, _, name, url)| (name, url))
        .collect())
}

/// Read each `apisetschema.dll` in the client and server install media of `release`.
///
/// The result maps the SHA-256 (lowercase hex) of each file to its bytes.
///
/// # Errors
///
/// Returns an error if the UUP dump service or a download fails.
pub fn scan(agent: &ureq::Agent, release: &str) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut found = BTreeMap::new();
    for build in media_builds(release) {
        let ids = media_ids(agent, build)?;
        if ids.is_empty() {
            continue;
        }
        for (media, id) in ids {
            let esds = match esd_files(agent, media, &id) {
                Ok(esds) => esds,
                Err(e) => {
                    eprintln!("  UUP {id}: {e}");
                    continue;
                }
            };
            for (name, url) in esds {
                if let Err(e) = scan_esd(agent, &name, &url, &mut found) {
                    eprintln!("  {name}: {e}");
                }
            }
        }
        break;
    }
    Ok(found)
}

/// Add each `apisetschema.dll` of one ESD file to `found`.
fn scan_esd(
    agent: &ureq::Agent,
    name: &str,
    url: &str,
    found: &mut BTreeMap<String, Vec<u8>>,
) -> Result<()> {
    let file = HttpFile::open(agent, url)?;
    let wim = Wim::open(&file)?;
    for index in 1..=wim.image_count() {
        let image = wim.image(index)?;
        // A file whose data is in another ESD of the set is read from that ESD.
        let candidates = image.files().iter().filter(|f| {
            !f.is_directory()
                && f.stored
                && f.path
                    .rsplit('\\')
                    .next()
                    .is_some_and(|n| n.eq_ignore_ascii_case("apisetschema.dll"))
        });
        for candidate in wim.storage_order(candidates) {
            let data = wim.read(candidate)?;
            let hash = crate::hex(&Sha256::digest(&data));
            if let std::collections::btree_map::Entry::Vacant(slot) = found.entry(hash) {
                eprintln!("  {name}: {} ({:.12})", candidate.path, slot.key());
                slot.insert(data);
            }
        }
    }
    Ok(())
}

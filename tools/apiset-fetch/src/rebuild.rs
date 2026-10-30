//! Rebuild files that the symbol server does not have, from update packages.
//!
//! Most updates store a changed file as a forward delta against an earlier version of the
//! file. The delta header has the SHA-256 of that source. The tool rebuilds a file in these
//! steps:
//!
//! 1. Find an update that contains the file (Winbindex), and its package URL (Microsoft
//!    Update Catalog).
//! 2. Read the payload of the file from the package. It is a full file or a delta.
//! 3. For a delta, get its source: a file in the cache, a file that the tool rebuilds first,
//!    or a release file from the install media.
//! 4. Apply the delta with `msdelta`, and check the SHA-256 of the result.
//!
//! The tool repeats these steps until no more files can be rebuilt.

use crate::catalog;
use crate::package::{self, Payload};
use crate::winbindex::Record;
use crate::{Result, hex};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

/// The result of a rebuild of one file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "status", content = "reason")]
pub enum Rebuilt {
    /// The tool rebuilt the file.
    Rebuilt,
    /// The tool could not rebuild the file.
    Failed(String),
}

/// A package of one update for one release.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PackageKey {
    release: String,
    update: String,
}

/// The state of a rebuild run.
pub struct Rebuilder<'a> {
    agent: &'a ureq::Agent,
    dir: &'a Path,
    records: BTreeMap<&'a str, &'a Record>,
    /// The files that the cache has, by SHA-256.
    known: BTreeMap<String, Vec<u8>>,
    /// The deltas that wait for their source, by target SHA-256.
    deltas: BTreeMap<String, Vec<u8>>,
    /// The packages that the tool already read, and the resolved package URLs.
    packages: BTreeMap<PackageKey, Option<String>>,
    /// The releases whose install media the tool already read.
    media: BTreeSet<String>,
    /// The files of the install media of each release, by SHA-256.
    media_files: BTreeMap<String, BTreeSet<String>>,
    /// The (target, source) pairs that failed for deltas without a source hash.
    tried: BTreeSet<(String, String)>,
    /// The reason for each failure, by SHA-256.
    failures: BTreeMap<String, String>,
}

impl<'a> Rebuilder<'a> {
    /// Create a rebuilder. `known` has the files that the cache already has.
    pub fn new(
        agent: &'a ureq::Agent,
        dir: &'a Path,
        records: &'a [Record],
        known: BTreeMap<String, Vec<u8>>,
    ) -> Self {
        Self {
            agent,
            dir,
            records: records.iter().map(|r| (r.sha256.as_str(), r)).collect(),
            known,
            deltas: BTreeMap::new(),
            packages: BTreeMap::new(),
            media: BTreeSet::new(),
            media_files: BTreeMap::new(),
            tried: BTreeSet::new(),
            failures: BTreeMap::new(),
        }
    }

    /// Rebuild the `targets` (SHA-256 values) and return the result of each.
    pub fn run(&mut self, targets: &[String]) -> BTreeMap<String, Rebuilt> {
        let mut pending: BTreeSet<String> = targets
            .iter()
            .filter(|t| !self.known.contains_key(*t))
            .cloned()
            .collect();
        loop {
            let mut progress = self.apply_ready();
            pending.retain(|t| !self.known.contains_key(t));
            if pending.is_empty() {
                break;
            }
            // Ask for the sources of waiting deltas too.
            let sources = self.missing_sources();
            for source in &sources {
                if self.records.contains_key(source.as_str()) {
                    progress |= pending.insert(source.clone());
                }
            }
            // Media first when a delta waits for its source: one media set can give the source
            // of many deltas, and a package is large.
            if let Some(release) = self.next_media(&sources, &pending) {
                self.read_media(&release);
                progress = true;
            } else if let Some(key) = self.next_package(&pending) {
                self.read_package(&key, &pending);
                progress = true;
            }
            if !progress {
                break;
            }
        }
        targets
            .iter()
            .map(|t| {
                let result = if self.known.contains_key(t) {
                    Rebuilt::Rebuilt
                } else {
                    Rebuilt::Failed(self.reason(t))
                };
                (t.clone(), result)
            })
            .collect()
    }

    /// Return the failure reason of a target.
    fn reason(&self, target: &str) -> String {
        if let Some(reason) = self.failures.get(target) {
            return reason.clone();
        }
        if let Some(delta) = self.deltas.get(target) {
            return source_hash(delta).map_or_else(
                || {
                    let tried = self.tried.iter().filter(|(t, _)| t == target).count();
                    format!(
                        "the delta has no source hash, and none of the {tried} known files of \
                         its release is its source"
                    )
                },
                |source| format!("the source {source:.16} of the delta is not available"),
            );
        }
        "no package with the file is available".to_owned()
    }

    /// Apply each delta whose source is known. Return `true` if a file was rebuilt.
    ///
    /// A PA30 delta has no source hash. The tool tries the files of the same release as the
    /// source. The target hash of the delta rejects a wrong source.
    fn apply_ready(&mut self) -> bool {
        let mut progress = false;
        let targets: Vec<String> = self.deltas.keys().cloned().collect();
        for target in targets {
            let Some(delta) = self.deltas.get(&target).cloned() else {
                continue;
            };
            let candidates: Vec<String> = match source_hash(&delta) {
                Some(source) if self.known.contains_key(&source) => vec![source],
                Some(_) => continue,
                None => self
                    .candidates(&target)
                    .into_iter()
                    .filter(|c| !self.tried.contains(&(target.clone(), c.clone())))
                    .collect(),
            };
            let with_hash = source_hash(&delta).is_some();
            for source in candidates {
                match msdelta::apply(&self.known[&source], &delta) {
                    Ok(bytes) => {
                        self.deltas.remove(&target);
                        progress |= self.add_file(&target, bytes);
                        break;
                    }
                    // A wrong candidate for a delta without a source hash is not a failure.
                    Err(e) if with_hash => {
                        self.failures
                            .insert(target.clone(), format!("msdelta: {e}"));
                    }
                    Err(_) => {
                        self.tried.insert((target.clone(), source));
                    }
                }
            }
        }
        progress
    }

    /// Return the releases of a file.
    fn releases_of(&self, sha256: &str) -> BTreeSet<&str> {
        self.records
            .get(sha256)
            .map(|record| record.sources.iter().map(|s| s.release.as_str()).collect())
            .unwrap_or_default()
    }

    /// Return the possible sources of a delta without a source hash: the known files of the
    /// releases of its target, other than the target.
    fn candidates(&self, target: &str) -> Vec<String> {
        let releases = self.releases_of(target);
        let mut found: BTreeSet<String> = self
            .known
            .keys()
            .filter(|sha| *sha != target)
            .filter(|sha| !self.releases_of(sha).is_disjoint(&releases))
            .cloned()
            .collect();
        for release in &releases {
            if let Some(files) = self.media_files.get(*release) {
                found.extend(files.iter().cloned());
            }
        }
        found.into_iter().collect()
    }

    /// Add a rebuilt or found file after a hash check. Return `true` if it is new.
    fn add_file(&mut self, expected: &str, bytes: Vec<u8>) -> bool {
        let actual = hex(&Sha256::digest(&bytes));
        if actual != expected {
            self.failures.insert(
                expected.to_owned(),
                format!("SHA-256 mismatch: got {actual}"),
            );
            return false;
        }
        if self.known.contains_key(&actual) {
            return false;
        }
        let name = self.records.get(actual.as_str()).map_or_else(
            || format!("apisetschema-base-{}.dll", &actual[..12]),
            |record| record.file_name(),
        );
        if let Err(e) = fs::write(self.dir.join(name), &bytes) {
            self.failures.insert(actual, format!("write: {e}"));
            return false;
        }
        eprintln!("  rebuilt {actual:.12}");
        self.known.insert(actual, bytes);
        true
    }

    /// Return the sources of the waiting deltas that are not known.
    fn missing_sources(&self) -> BTreeSet<String> {
        self.deltas
            .values()
            .filter_map(|delta| source_hash(delta))
            .filter(|source| !self.known.contains_key(source))
            .collect()
    }

    /// Return the next package to read: the newest unread update of a pending file.
    fn next_package(&self, pending: &BTreeSet<String>) -> Option<PackageKey> {
        let mut best: Option<(&str, PackageKey)> = None;
        for target in pending {
            if self.deltas.contains_key(target) {
                continue;
            }
            let Some(record) = self.records.get(target.as_str()) else {
                continue;
            };
            for source in &record.sources {
                let key = PackageKey {
                    release: source.release.clone(),
                    update: source.update.clone(),
                };
                if source.update == "BASE" || self.packages.contains_key(&key) {
                    continue;
                }
                let date = source.date.as_deref().unwrap_or("");
                if best.as_ref().is_none_or(|(best_date, _)| date > *best_date) {
                    best = Some((date, key));
                }
            }
        }
        best.map(|(_, key)| key)
    }

    /// Read the payloads of the pending files from one package.
    fn read_package(&mut self, key: &PackageKey, pending: &BTreeSet<String>) {
        let url = match self.package_url(key) {
            Ok(url) => url,
            Err(e) => {
                eprintln!("{} {}: {e}", key.release, key.update);
                None
            }
        };
        self.packages.insert(key.clone(), url.clone());
        let Some(url) = url else {
            return;
        };

        // Map each component directory of this update to its file.
        let mut components: BTreeMap<String, String> = BTreeMap::new();
        for target in pending {
            let Some(record) = self.records.get(target.as_str()) else {
                continue;
            };
            for source in record
                .sources
                .iter()
                .filter(|s| s.release == key.release && s.update == key.update)
            {
                for component in &source.components {
                    components.insert(component.clone(), target.clone());
                }
            }
        }
        if components.is_empty() {
            return;
        }
        eprintln!(
            "{} {}: {} file(s) from {url}",
            key.release,
            key.update,
            components.len()
        );
        let names: Vec<String> = components.keys().cloned().collect();
        match package::read_payloads(self.agent, &url, &names) {
            Ok(payloads) => {
                for (component, payload) in payloads {
                    let Some(target) = components.get(&component).cloned() else {
                        continue;
                    };
                    match payload {
                        Payload::Full(bytes) => {
                            self.add_file(&target, bytes);
                        }
                        Payload::Delta(delta) => {
                            self.deltas.insert(target, delta);
                        }
                    }
                }
            }
            Err(e) => eprintln!("  {url}: {e}"),
        }
    }

    /// Return the package URL of an update, from the Microsoft Update Catalog.
    fn package_url(&self, key: &PackageKey) -> Result<Option<String>> {
        let updates = catalog::search(self.agent, &key.update)?;
        let Some(update) = catalog::pick(&updates, &key.release) else {
            return Ok(None);
        };
        let kb = key.update.to_ascii_lowercase();
        let urls = catalog::download_urls(self.agent, &update.id)?;
        Ok(urls
            .into_iter()
            .find(|url| url.to_ascii_lowercase().contains(&kb)))
    }

    /// Return a release whose install media can have a missing source.
    fn next_media(&self, sources: &BTreeSet<String>, pending: &BTreeSet<String>) -> Option<String> {
        // The release of a delta target is the release of its source.
        let mut releases = BTreeSet::new();
        for (target, delta) in &self.deltas {
            let waits = source_hash(delta).is_none_or(|s| sources.contains(&s));
            if waits && let Some(record) = self.records.get(target.as_str()) {
                releases.extend(record.sources.iter().map(|s| s.release.clone()));
            }
        }
        // A pending file with only a `BASE` source is in the install media too.
        for target in pending {
            if let Some(record) = self.records.get(target.as_str())
                && record.sources.iter().all(|s| s.update == "BASE")
            {
                releases.extend(record.sources.iter().map(|s| s.release.clone()));
            }
        }
        releases.into_iter().find(|r| !self.media.contains(r))
    }

    /// Read each `apisetschema.dll` of the install media of a release.
    fn read_media(&mut self, release: &str) {
        self.media.insert(release.to_owned());
        eprintln!("{release}: reading the install media");
        match crate::base::scan(self.agent, release) {
            Ok(files) => {
                let hashes = files.keys().cloned().collect();
                self.media_files.insert(release.to_owned(), hashes);
                for (hash, bytes) in files {
                    self.add_file(&hash, bytes);
                }
            }
            Err(e) => eprintln!("  {release}: {e}"),
        }
    }
}

/// Return the source SHA-256 (lowercase hex) of a delta, if its header has one.
fn source_hash(delta: &[u8]) -> Option<String> {
    let header = msdelta::DeltaHeader::parse(delta).ok()?;
    let pa31 = header.pa31?;
    (!pa31.source_hash.is_empty()).then(|| hex(&pa31.source_hash))
}

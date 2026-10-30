//! Resolve API set names to hosts, as the Windows loader does.
//!
//! The functions follow `ApiSetResolveToHost` and its helpers in `ntdll.dll` for each schema
//! version:
//!
//! - Versions 2 and 3 accept names that start with `api-` (version 3 also `ext-`). They remove
//!   the prefix and a `.dll` suffix, and binary-search the entries by name.
//! - Version 4 does the same as version 3.
//! - Version 6 removes the text from the last hyphen, for example the version `-0.dll`. It
//!   hashes the rest, binary-searches the hash table, and compares the hashed part of the
//!   entry name. Thus any version of an API set name finds the entry.
//!
//! In all versions, an entry with more than one value can have a host for the importing
//! module. The loader binary-searches the values after the first value by importer name. If no
//! value matches, it uses the first value.
//!
//! All name comparisons ignore case. The searches use the same middle index as the loader, so
//! that the result is the same also for data that is not sorted.

use super::{ApiSetEntry, ApiSetSchema, ApiSetValue, SchemaVersion};
use crate::utf16::{compare_ignore_case, hash};
use core::cmp::Ordering;

/// The result of [`ApiSetSchema::resolve`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution<'a> {
    /// The name is not an API set name for this schema version. The loader loads it as a
    /// normal DLL.
    NotApiSet,
    /// The name is an API set name, but the schema has no entry for it.
    NotFound,
    /// The schema has an entry, but the entry has no host for the importer. The entry has no
    /// values, or the selected value has an empty host.
    NoHost(&'a ApiSetEntry),
    /// The schema has a host for the name.
    Host {
        /// The entry that matched the name.
        entry: &'a ApiSetEntry,
        /// The value that the importer selected.
        value: &'a ApiSetValue,
    },
}

impl<'a> Resolution<'a> {
    /// Return the host DLL name, or `None` if the name has no host.
    #[must_use]
    pub fn host(&self) -> Option<&'a str> {
        match self {
            Self::Host { value, .. } => Some(value.host.as_str()),
            _ => None,
        }
    }

    /// Return the entry that matched the name, or `None` if no entry matched.
    #[must_use]
    pub fn entry(&self) -> Option<&'a ApiSetEntry> {
        match self {
            Self::NoHost(entry) | Self::Host { entry, .. } => Some(entry),
            Self::NotApiSet | Self::NotFound => None,
        }
    }
}

/// Return `true` if `name` starts with `api-`, or with `ext-` if `allow_ext` is `true`.
///
/// The loader clears bit 5 of the first three code units and compares them with `API` and
/// `EXT`. The fourth code unit must be a hyphen.
fn has_api_set_prefix(name: &str, allow_ext: bool) -> bool {
    let Some(&[a, b, c, hyphen]) = name.as_bytes().first_chunk::<4>() else {
        return false;
    };
    let upper = [a & !0x20, b & !0x20, c & !0x20];
    hyphen == b'-' && (upper == *b"API" || (allow_ext && upper == *b"EXT"))
}

/// Remove a 4-code-unit extension, as the loader does for versions 2, 3 and 4.
///
/// The loader checks only that the fourth code unit from the end is a period. It does not
/// check the letters `dll`.
fn strip_extension(name: &str) -> &str {
    let mut units = 0;
    for (index, c) in name.char_indices().rev() {
        units += c.len_utf16();
        if units >= 4 {
            return if units == 4 && c == '.' {
                &name[..index]
            } else {
                name
            };
        }
    }
    name
}

/// Binary-search `low..count` with the middle index of the loader.
///
/// `compare` gets an index and returns the order of the search key relative to the item at
/// that index.
fn loader_search(
    low: usize,
    count: usize,
    mut compare: impl FnMut(usize) -> Ordering,
) -> Option<usize> {
    let mut low = i64::try_from(low).ok()?;
    let mut high = i64::try_from(count).ok()? - 1;
    while high >= low {
        let middle = (low + high) >> 1;
        let index = usize::try_from(middle).ok()?;
        match compare(index) {
            Ordering::Less => high = middle - 1,
            Ordering::Greater => low = middle + 1,
            Ordering::Equal => return Some(index),
        }
    }
    None
}

/// Select the value for an importer, as `ApiSetpSearchForApiSetHost` does.
fn select_value<'a>(values: &'a [ApiSetValue], importer: Option<&str>) -> Option<&'a ApiSetValue> {
    let first = values.first()?;
    if values.len() > 1
        && let Some(importer) = importer
        && let Some(index) = loader_search(1, values.len(), |index| {
            values.get(index).map_or(Ordering::Less, |value| {
                compare_ignore_case(importer, &value.importer)
            })
        })
    {
        return values.get(index);
    }
    Some(first)
}

impl ApiSetSchema {
    /// Return `true` if the loader treats `name` as an API set name for this schema version.
    ///
    /// Version 2 accepts names that start with `api-`. Later versions also accept `ext-`. The
    /// check ignores case.
    #[must_use]
    pub fn is_api_set_name(&self, name: &str) -> bool {
        has_api_set_prefix(name, self.version != SchemaVersion::V2)
    }

    /// Find the index of the entry for `name`, or `None` if no entry matches.
    fn find_index(&self, name: &str) -> Option<usize> {
        if !self.is_api_set_name(name) {
            return None;
        }
        if self.version == SchemaVersion::V6 {
            return self.find_index_v6(name);
        }
        let key = strip_extension(&name[4..]);
        loader_search(0, self.entries.len(), |index| {
            self.entries.get(index).map_or(Ordering::Less, |entry| {
                compare_ignore_case(key, &entry.name)
            })
        })
    }

    /// Find the index of the entry for `name` in a version 6 schema.
    fn find_index_v6(&self, name: &str) -> Option<usize> {
        // The prefix check makes sure that index 3 is a hyphen.
        let key = &name[..name.rfind('-')?];
        let key_hash = hash(key, self.hash_factor);
        let slot = loader_search(0, self.hash_table.len(), |index| {
            self.hash_table
                .get(index)
                .map_or(Ordering::Less, |entry| key_hash.cmp(&entry.hash))
        })?;
        let index = usize::try_from(self.hash_table.get(slot)?.index).ok()?;
        let hashed_name = self.hashed_name(index)?;
        (compare_ignore_case(key, hashed_name) == Ordering::Equal).then_some(index)
    }

    /// Find the entry for an API set name, as the loader does.
    ///
    /// `name` is a DLL name, for example `api-ms-win-core-synch-l1-2-0.dll`. The `.dll` suffix
    /// is optional. The function returns `None` if `name` is not an API set name or if the
    /// schema has no entry for it.
    #[must_use]
    pub fn find(&self, name: &str) -> Option<&ApiSetEntry> {
        self.entries.get(self.find_index(name)?)
    }

    /// Resolve an API set name to its host DLL, as `ApiSetResolveToHost` does.
    ///
    /// `name` is a DLL name, for example `api-ms-win-core-synch-l1-2-0.dll`. `importer` is the
    /// base name of the importing module, for example `kernel32.dll`. An entry can have a
    /// different host for one importer.
    ///
    /// # Example
    ///
    /// ```
    /// use apiset_rs::{ApiSetEntry, ApiSetSchema, ApiSetSchemaBuilder, Resolution, SchemaVersion};
    ///
    /// let bytes = ApiSetSchemaBuilder::new(SchemaVersion::V6)
    ///     .entry(
    ///         ApiSetEntry::new("api-ms-win-core-file-l1-2-4", "kernelbase.dll")
    ///             .with_importer("kernelbase.dll", "kernel32.dll"),
    ///     )
    ///     .try_build()?;
    /// let schema = ApiSetSchema::parse(&bytes)?;
    ///
    /// // Version 6 ignores the last version number.
    /// let any_minor = schema.resolve("API-MS-WIN-CORE-FILE-L1-2-0.DLL", None);
    /// assert_eq!(any_minor.host(), Some("kernelbase.dll"));
    ///
    /// // An importer can have its own host.
    /// let own = schema.resolve("api-ms-win-core-file-l1-2-4.dll", Some("KernelBase.dll"));
    /// assert_eq!(own.host(), Some("kernel32.dll"));
    ///
    /// assert_eq!(schema.resolve("kernel32.dll", None), Resolution::NotApiSet);
    /// # Ok::<(), apiset_rs::Error>(())
    /// ```
    #[must_use]
    pub fn resolve(&self, name: &str, importer: Option<&str>) -> Resolution<'_> {
        if !self.is_api_set_name(name) {
            return Resolution::NotApiSet;
        }
        let Some(entry) = self.find(name) else {
            return Resolution::NotFound;
        };
        match select_value(&entry.values, importer) {
            Some(value) if !value.host.is_empty() => Resolution::Host { entry, value },
            _ => Resolution::NoHost(entry),
        }
    }
}

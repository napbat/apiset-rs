//! Build API set schema bytes.
//!
//! [`ApiSetSchemaBuilder`] writes a schema of any supported version. The builder sorts the
//! entries and the values, so that the binary searches of the loader find them:
//!
//! - It sorts the entries by name, and ignores case.
//! - It keeps the first value of each entry first, because the first value is the default host.
//!   It sorts the other values by importer name, and ignores case.
//! - For version 6, it calculates `HashedLength` from the name, and writes a hash table that is
//!   sorted by hash.
//!
//! The builder writes each distinct string one time. The output is not byte-for-byte equal to
//! a schema from Windows, but [`crate::ApiSetSchema::parse`] gives the same entries in the
//! sorted order.
//!
//! # Example
//!
//! ```
//! use apiset_rs::{ApiSetEntry, ApiSetSchema, ApiSetSchemaBuilder, SchemaVersion};
//!
//! let bytes = ApiSetSchemaBuilder::new(SchemaVersion::V2)
//!     .entry(ApiSetEntry::new("ms-win-core-console-l1-1-0", "kernel32.dll"))
//!     .try_build()?;
//! let schema = ApiSetSchema::parse(&bytes)?;
//! assert_eq!(schema.version(), SchemaVersion::V2);
//! assert_eq!(schema.entries()[0].name, "ms-win-core-console-l1-1-0");
//! # Ok::<(), apiset_rs::Error>(())
//! ```

use crate::prelude::*;
use crate::raw::{
    HashEntryV6, NamespaceEntryV2, NamespaceEntryV4, NamespaceEntryV6, NamespaceHeaderV2,
    NamespaceHeaderV4, NamespaceHeaderV6, ValueArrayHeaderV2, ValueArrayHeaderV4, ValueEntryV2,
    ValueEntryV4,
};
use crate::utf16::{compare_ignore_case, hash};
use crate::{ApiSetEntry, ApiSetSchema, ApiSetValue, Error, Result, SchemaVersion};
use alloc::collections::BTreeMap;
use core::cmp::Ordering;

/// Build the bytes of an API set schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiSetSchemaBuilder {
    version: SchemaVersion,
    flags: u32,
    hash_factor: u32,
    entries: Vec<ApiSetEntry>,
}

impl ApiSetSchemaBuilder {
    /// The hash factor of the schemas of Windows 10 and later.
    pub const DEFAULT_HASH_FACTOR: u32 = 0x1f;

    /// Create an empty builder for a schema version.
    #[must_use]
    pub fn new(version: SchemaVersion) -> Self {
        Self {
            version,
            flags: 0,
            hash_factor: Self::DEFAULT_HASH_FACTOR,
            entries: Vec::new(),
        }
    }

    /// Create a builder with the version, the flags, the hash factor and the entries of a
    /// parsed schema.
    #[must_use]
    pub fn from_schema(schema: &ApiSetSchema) -> Self {
        Self {
            version: schema.version(),
            flags: schema.flags(),
            hash_factor: schema.hash_factor().unwrap_or(Self::DEFAULT_HASH_FACTOR),
            entries: schema.entries().to_vec(),
        }
    }

    /// Set the schema flags (V4 and V6, see [`crate::raw::flags`]).
    #[must_use]
    pub fn flags(mut self, flags: u32) -> Self {
        self.flags = flags;
        self
    }

    /// Set the hash factor (V6 only).
    #[must_use]
    pub fn hash_factor(mut self, factor: u32) -> Self {
        self.hash_factor = factor;
        self
    }

    /// Add an entry.
    #[must_use]
    pub fn entry(mut self, entry: ApiSetEntry) -> Self {
        self.entries.push(entry);
        self
    }

    /// Add entries.
    #[must_use]
    pub fn entries(mut self, entries: impl IntoIterator<Item = ApiSetEntry>) -> Self {
        self.entries.extend(entries);
        self
    }

    /// Return the entries in insertion order, for change.
    ///
    /// Use this function to change the entries of a builder from [`Self::from_schema`].
    pub fn entries_mut(&mut self) -> &mut Vec<ApiSetEntry> {
        &mut self.entries
    }

    /// Build the schema bytes.
    ///
    /// # Panics
    ///
    /// Panics if the input is not valid. Use [`Self::try_build`] to get a `Result`.
    #[must_use]
    pub fn build(&self) -> Vec<u8> {
        self.try_build()
            .expect("invalid API set schema input: use try_build() for fallible builds")
    }

    /// Build the schema bytes.
    ///
    /// # Errors
    ///
    /// Returns an `InvalidInput` error in these conditions:
    ///
    /// - An entry has an empty name.
    /// - Two entries have the same name. For version 6, two entries have the same hashed part
    ///   of the name, or the hashes of two names are equal.
    /// - A version 6 name has no hyphen after the fourth character.
    /// - A value after the first value has no importer, or two such values have the same
    ///   importer.
    /// - The schema is larger than 4 GiB.
    pub fn try_build(&self) -> Result<Vec<u8>> {
        let entries = self.ordered_entries()?;
        match self.version {
            SchemaVersion::V2 | SchemaVersion::V3 => build_v2(self.version, &entries),
            SchemaVersion::V4 => build_v4(self.flags, &entries),
            SchemaVersion::V6 => build_v6(self.flags, self.hash_factor, &entries),
        }
    }

    /// Sort and check the entries and their values.
    fn ordered_entries(&self) -> Result<Vec<Ordered<'_>>> {
        let mut entries = self
            .entries
            .iter()
            .map(Ordered::new)
            .collect::<Result<Vec<_>>>()?;
        entries.sort_by(|a, b| compare_ignore_case(&a.entry.name, &b.entry.name));
        for pair in entries.windows(2) {
            if let [a, b] = pair
                && compare_ignore_case(&a.entry.name, &b.entry.name) == Ordering::Equal
            {
                return Err(Error::invalid_input(format!(
                    "duplicate entry names {:?} and {:?}",
                    a.entry.name, b.entry.name
                )));
            }
        }
        Ok(entries)
    }
}

/// An entry with its values in output order.
struct Ordered<'a> {
    entry: &'a ApiSetEntry,
    values: Vec<&'a ApiSetValue>,
}

impl<'a> Ordered<'a> {
    /// Check an entry and sort its values.
    fn new(entry: &'a ApiSetEntry) -> Result<Self> {
        if entry.name.is_empty() {
            return Err(Error::invalid_input("an entry has an empty name"));
        }
        let mut values: Vec<&ApiSetValue> = entry.values.iter().collect();
        if let Some(rest) = values.get_mut(1..) {
            rest.sort_by(|a, b| compare_ignore_case(&a.importer, &b.importer));
            if rest.iter().any(|value| value.importer.is_empty()) {
                return Err(Error::invalid_input(format!(
                    "entry {:?}: a value after the first value has no importer",
                    entry.name
                )));
            }
            for pair in rest.windows(2) {
                if let [a, b] = pair
                    && compare_ignore_case(&a.importer, &b.importer) == Ordering::Equal
                {
                    return Err(Error::invalid_input(format!(
                        "entry {:?}: duplicate importer {:?}",
                        entry.name, a.importer
                    )));
                }
            }
        }
        Ok(Self { entry, values })
    }
}

/// Return the error for a schema that is larger than 4 GiB.
fn too_large() -> Error {
    Error::invalid_input("the schema is larger than 4 GiB")
}

/// Return `base + count * size`, or an error if the result does not fit in `u32`.
fn offset_after(base: u32, count: usize, size: usize) -> Result<u32> {
    count
        .checked_mul(size)
        .and_then(|len| u32::try_from(len).ok())
        .and_then(|len| base.checked_add(len))
        .ok_or_else(too_large)
}

/// Return the size of a fixed record as `u32`.
fn size32(size: usize) -> Result<u32> {
    u32::try_from(size).map_err(|_| too_large())
}

/// Collect the strings of the schema. Each distinct string is stored one time.
struct StringPool<'a> {
    base: u32,
    bytes: Vec<u8>,
    offsets: BTreeMap<&'a str, (u32, u32)>,
}

impl<'a> StringPool<'a> {
    /// Create a pool that starts at offset `base` in the schema.
    fn new(base: u32) -> Self {
        Self {
            base,
            bytes: Vec::new(),
            offsets: BTreeMap::new(),
        }
    }

    /// Add a string and return its offset and its length in bytes.
    ///
    /// An empty string gets offset 0 and length 0.
    fn add(&mut self, s: &'a str) -> Result<(u32, u32)> {
        if s.is_empty() {
            return Ok((0, 0));
        }
        if let Some(&found) = self.offsets.get(s) {
            return Ok(found);
        }
        let offset = offset_after(self.base, self.bytes.len(), 1)?;
        for unit in s.encode_utf16() {
            self.bytes.extend_from_slice(&unit.to_le_bytes());
        }
        let length = offset_after(self.base, self.bytes.len(), 1)? - offset;
        self.offsets.insert(s, (offset, length));
        Ok((offset, length))
    }

    /// Return the end offset of the pool.
    fn end(&self) -> Result<u32> {
        offset_after(self.base, self.bytes.len(), 1)
    }
}

/// Build a version 2 or version 3 schema.
fn build_v2(version: SchemaVersion, entries: &[Ordered<'_>]) -> Result<Vec<u8>> {
    let header_size = size32(NamespaceHeaderV2::SIZE)?;
    let mut values_offset = offset_after(header_size, entries.len(), NamespaceEntryV2::SIZE)?;
    let mut strings_offset = values_offset;
    for ordered in entries {
        strings_offset = offset_after(strings_offset, ordered.values.len(), ValueEntryV2::SIZE)?;
        strings_offset = offset_after(strings_offset, 1, ValueArrayHeaderV2::SIZE)?;
    }

    let mut pool = StringPool::new(strings_offset);
    let mut entry_bytes = Vec::new();
    let mut value_bytes = Vec::new();
    for ordered in entries {
        let (name_offset, name_length) = pool.add(&ordered.entry.name)?;
        entry_bytes.extend_from_slice(
            &NamespaceEntryV2 {
                name_offset,
                name_length,
                data_offset: values_offset,
            }
            .to_bytes(),
        );
        let count = u32::try_from(ordered.values.len()).map_err(|_| too_large())?;
        value_bytes.extend_from_slice(&ValueArrayHeaderV2 { count }.to_bytes());
        for value in &ordered.values {
            let (name_offset, name_length) = pool.add(&value.importer)?;
            let (value_offset, value_length) = pool.add(&value.host)?;
            value_bytes.extend_from_slice(
                &ValueEntryV2 {
                    name_offset,
                    name_length,
                    value_offset,
                    value_length,
                }
                .to_bytes(),
            );
        }
        values_offset = offset_after(values_offset, ordered.values.len(), ValueEntryV2::SIZE)?;
        values_offset = offset_after(values_offset, 1, ValueArrayHeaderV2::SIZE)?;
    }

    let count = u32::try_from(entries.len()).map_err(|_| too_large())?;
    let header = NamespaceHeaderV2 {
        version: version.as_u32(),
        count,
    };
    Ok(assemble(
        &[&header.to_bytes(), &entry_bytes, &value_bytes],
        &pool,
    ))
}

/// Build a version 4 schema.
fn build_v4(flags: u32, entries: &[Ordered<'_>]) -> Result<Vec<u8>> {
    let header_size = size32(NamespaceHeaderV4::SIZE)?;
    let mut values_offset = offset_after(header_size, entries.len(), NamespaceEntryV4::SIZE)?;
    let mut strings_offset = values_offset;
    for ordered in entries {
        strings_offset = offset_after(strings_offset, ordered.values.len(), ValueEntryV4::SIZE)?;
        strings_offset = offset_after(strings_offset, 1, ValueArrayHeaderV4::SIZE)?;
    }

    let mut pool = StringPool::new(strings_offset);
    let mut entry_bytes = Vec::new();
    let mut value_bytes = Vec::new();
    for ordered in entries {
        let (name_offset, name_length) = pool.add(&ordered.entry.name)?;
        let (alias_offset, alias_length) = pool.add(&ordered.entry.alias)?;
        entry_bytes.extend_from_slice(
            &NamespaceEntryV4 {
                flags: ordered.entry.flags,
                name_offset,
                name_length,
                alias_offset,
                alias_length,
                data_offset: values_offset,
            }
            .to_bytes(),
        );
        let count = u32::try_from(ordered.values.len()).map_err(|_| too_large())?;
        let array = ValueArrayHeaderV4 {
            flags: ordered.entry.value_flags,
            count,
        };
        value_bytes.extend_from_slice(&array.to_bytes());
        write_values_v4(&mut pool, &mut value_bytes, &ordered.values)?;
        values_offset = offset_after(values_offset, ordered.values.len(), ValueEntryV4::SIZE)?;
        values_offset = offset_after(values_offset, 1, ValueArrayHeaderV4::SIZE)?;
    }

    let count = u32::try_from(entries.len()).map_err(|_| too_large())?;
    let header = NamespaceHeaderV4 {
        version: SchemaVersion::V4.as_u32(),
        size: pool.end()?,
        flags,
        count,
    };
    Ok(assemble(
        &[&header.to_bytes(), &entry_bytes, &value_bytes],
        &pool,
    ))
}

/// Write the `API_SET_VALUE_ENTRY` records of one entry (V4 and V6).
fn write_values_v4<'a>(
    pool: &mut StringPool<'a>,
    out: &mut Vec<u8>,
    values: &[&'a ApiSetValue],
) -> Result<()> {
    for value in values {
        let (name_offset, name_length) = pool.add(&value.importer)?;
        let (value_offset, value_length) = pool.add(&value.host)?;
        out.extend_from_slice(
            &ValueEntryV4 {
                flags: value.flags,
                name_offset,
                name_length,
                value_offset,
                value_length,
            }
            .to_bytes(),
        );
    }
    Ok(())
}

/// Return the hashed part of a version 6 name: the text before the last hyphen.
fn hashed_part(name: &str) -> Result<&str> {
    match name.rfind('-') {
        Some(index) if index >= 3 => Ok(&name[..index]),
        _ => Err(Error::invalid_input(format!(
            "version 6 entry name {name:?} needs a hyphen after the fourth character"
        ))),
    }
}

/// Build a version 6 schema.
fn build_v6(flags: u32, hash_factor: u32, entries: &[Ordered<'_>]) -> Result<Vec<u8>> {
    let entry_offset = size32(NamespaceHeaderV6::SIZE)?;
    let values_offset = offset_after(entry_offset, entries.len(), NamespaceEntryV6::SIZE)?;
    let value_count = entries.iter().map(|ordered| ordered.values.len()).sum();
    let hash_offset = offset_after(values_offset, value_count, ValueEntryV4::SIZE)?;
    let strings_offset = offset_after(hash_offset, entries.len(), HashEntryV6::SIZE)?;

    let mut pool = StringPool::new(strings_offset);
    let mut entry_bytes = Vec::new();
    let mut value_bytes = Vec::new();
    let mut slots = Vec::with_capacity(entries.len());
    let mut value_cursor = values_offset;
    for (index, ordered) in entries.iter().enumerate() {
        let name = &ordered.entry.name;
        let hashed = hashed_part(name)?;
        let hashed_length = offset_after(0, hashed.encode_utf16().count(), 2)?;
        let (name_offset, name_length) = pool.add(name)?;
        let count = u32::try_from(ordered.values.len()).map_err(|_| too_large())?;
        entry_bytes.extend_from_slice(
            &NamespaceEntryV6 {
                flags: ordered.entry.flags,
                name_offset,
                name_length,
                hashed_length,
                value_offset: value_cursor,
                value_count: count,
            }
            .to_bytes(),
        );
        write_values_v4(&mut pool, &mut value_bytes, &ordered.values)?;
        value_cursor = offset_after(value_cursor, ordered.values.len(), ValueEntryV4::SIZE)?;
        let index = u32::try_from(index).map_err(|_| too_large())?;
        slots.push((hash(hashed, hash_factor), index, hashed));
    }

    slots.sort_by_key(|&(hash, _, _)| hash);
    for pair in slots.windows(2) {
        if let [(left_hash, _, left), (right_hash, _, right)] = pair
            && left_hash == right_hash
        {
            let reason = if compare_ignore_case(left, right) == Ordering::Equal {
                "have the same hashed name"
            } else {
                "have the same hash: use a different hash factor"
            };
            return Err(Error::invalid_input(format!(
                "the entries {left:?} and {right:?} {reason}"
            )));
        }
    }
    let mut hash_bytes = Vec::with_capacity(slots.len() * HashEntryV6::SIZE);
    for (hash, index, _) in slots {
        hash_bytes.extend_from_slice(&HashEntryV6 { hash, index }.to_bytes());
    }

    let count = u32::try_from(entries.len()).map_err(|_| too_large())?;
    let header = NamespaceHeaderV6 {
        version: SchemaVersion::V6.as_u32(),
        size: pool.end()?,
        flags,
        count,
        entry_offset,
        hash_offset,
        hash_factor,
    };
    Ok(assemble(
        &[&header.to_bytes(), &entry_bytes, &value_bytes, &hash_bytes],
        &pool,
    ))
}

/// Concatenate the record parts and the string pool.
fn assemble(parts: &[&[u8]], pool: &StringPool<'_>) -> Vec<u8> {
    let len = parts.iter().map(|part| part.len()).sum::<usize>() + pool.bytes.len();
    let mut out = Vec::with_capacity(len);
    for part in parts {
        out.extend_from_slice(part);
    }
    out.extend_from_slice(&pool.bytes);
    out
}

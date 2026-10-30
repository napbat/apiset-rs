//! Parse each schema version into an `ApiSetSchema`.

use super::{ApiSetEntry, ApiSetSchema, ApiSetValue, SchemaVersion};
use crate::prelude::*;
use crate::raw::{
    HashEntryV6, NamespaceEntryV2, NamespaceEntryV4, NamespaceEntryV6, NamespaceHeaderV2,
    NamespaceHeaderV4, NamespaceHeaderV6, ValueArrayHeaderV2, ValueArrayHeaderV4, ValueEntryV2,
    ValueEntryV4,
};
use crate::utf16::{range, read_string};
use crate::{Error, Result};

/// Convert a record size to `u64`.
fn size64(size: usize) -> u64 {
    u64::try_from(size).unwrap_or(u64::MAX)
}

/// Return the bytes of a table of `count` records of `record_size` bytes at `offset`.
fn table<'a>(
    data: &'a [u8],
    offset: u64,
    count: u32,
    record_size: usize,
    structure: &str,
) -> Result<&'a [u8]> {
    let len = u64::from(count).saturating_mul(size64(record_size));
    range(data, offset, len, structure)
}

/// Limit `data` to the `Size` field of a V4 or V6 header.
fn limit_to_size<'a>(
    data: &'a [u8],
    size: u32,
    header_size: usize,
    structure: &str,
) -> Result<&'a [u8]> {
    let size = usize::try_from(size).unwrap_or(usize::MAX);
    if size < header_size {
        return Err(Error::invalid_schema(format!(
            "Size {size} is smaller than the {header_size}-byte header"
        ))
        .in_structure(structure));
    }
    data.get(..size)
        .ok_or_else(|| Error::buffer_too_small(size, data.len()).in_structure(structure))
}

/// Parse a version 2 or version 3 schema.
pub(super) fn parse_v2(data: &[u8], version: SchemaVersion) -> Result<ApiSetSchema> {
    const STRUCTURE: &str = "API_SET_NAMESPACE_ENTRY_V2";
    let header = NamespaceHeaderV2::parse(data)?;
    let records = table(
        data,
        size64(NamespaceHeaderV2::SIZE),
        header.count,
        NamespaceEntryV2::SIZE,
        STRUCTURE,
    )?;

    let mut entries = Vec::with_capacity(records.len() / NamespaceEntryV2::SIZE);
    for record in records.as_chunks::<{ NamespaceEntryV2::SIZE }>().0 {
        let raw = NamespaceEntryV2::parse(record)?;
        let array_offset = u64::from(raw.data_offset);
        let array = ValueArrayHeaderV2::parse(range(
            data,
            array_offset,
            size64(ValueArrayHeaderV2::SIZE),
            "API_SET_VALUE_ARRAY_V2",
        )?)?;
        let values_offset = array_offset.saturating_add(size64(ValueArrayHeaderV2::SIZE));
        entries.push(ApiSetEntry {
            name: read_string(data, raw.name_offset, raw.name_length, STRUCTURE)?,
            values: parse_values_v2(data, values_offset, array.count)?,
            ..ApiSetEntry::default()
        });
    }

    Ok(ApiSetSchema {
        version,
        flags: 0,
        hash_factor: 0,
        entries,
        hashed_lengths: Vec::new(),
        hash_table: Vec::new(),
    })
}

/// Parse the value entries of a version 2 or version 3 entry.
fn parse_values_v2(data: &[u8], offset: u64, count: u32) -> Result<Vec<ApiSetValue>> {
    const STRUCTURE: &str = "API_SET_VALUE_ENTRY_V2";
    let records = table(data, offset, count, ValueEntryV2::SIZE, STRUCTURE)?;
    records
        .as_chunks::<{ ValueEntryV2::SIZE }>()
        .0
        .iter()
        .map(|record| {
            let raw = ValueEntryV2::parse(record)?;
            Ok(ApiSetValue {
                importer: read_string(data, raw.name_offset, raw.name_length, STRUCTURE)?,
                host: read_string(data, raw.value_offset, raw.value_length, STRUCTURE)?,
                flags: 0,
            })
        })
        .collect()
}

/// Parse the value entries of a version 4 or version 6 entry.
fn parse_values_v4(data: &[u8], offset: u64, count: u32) -> Result<Vec<ApiSetValue>> {
    const STRUCTURE: &str = "API_SET_VALUE_ENTRY";
    let records = table(data, offset, count, ValueEntryV4::SIZE, STRUCTURE)?;
    records
        .as_chunks::<{ ValueEntryV4::SIZE }>()
        .0
        .iter()
        .map(|record| {
            let raw = ValueEntryV4::parse(record)?;
            Ok(ApiSetValue {
                importer: read_string(data, raw.name_offset, raw.name_length, STRUCTURE)?,
                host: read_string(data, raw.value_offset, raw.value_length, STRUCTURE)?,
                flags: raw.flags,
            })
        })
        .collect()
}

/// Parse a version 4 schema.
pub(super) fn parse_v4(data: &[u8]) -> Result<ApiSetSchema> {
    const STRUCTURE: &str = "API_SET_NAMESPACE_ENTRY_V4";
    let header = NamespaceHeaderV4::parse(data)?;
    let data = limit_to_size(
        data,
        header.size,
        NamespaceHeaderV4::SIZE,
        "API_SET_NAMESPACE_ARRAY_V4",
    )?;
    let records = table(
        data,
        size64(NamespaceHeaderV4::SIZE),
        header.count,
        NamespaceEntryV4::SIZE,
        STRUCTURE,
    )?;

    let mut entries = Vec::with_capacity(records.len() / NamespaceEntryV4::SIZE);
    for record in records.as_chunks::<{ NamespaceEntryV4::SIZE }>().0 {
        let raw = NamespaceEntryV4::parse(record)?;
        let array_offset = u64::from(raw.data_offset);
        let array = ValueArrayHeaderV4::parse(range(
            data,
            array_offset,
            size64(ValueArrayHeaderV4::SIZE),
            "API_SET_VALUE_ARRAY_V4",
        )?)?;
        let values_offset = array_offset.saturating_add(size64(ValueArrayHeaderV4::SIZE));
        entries.push(ApiSetEntry {
            name: read_string(data, raw.name_offset, raw.name_length, STRUCTURE)?,
            flags: raw.flags,
            alias: read_string(data, raw.alias_offset, raw.alias_length, STRUCTURE)?,
            value_flags: array.flags,
            values: parse_values_v4(data, values_offset, array.count)?,
        });
    }

    Ok(ApiSetSchema {
        version: SchemaVersion::V4,
        flags: header.flags,
        hash_factor: 0,
        entries,
        hashed_lengths: Vec::new(),
        hash_table: Vec::new(),
    })
}

/// Return the UTF-8 length of the prefix of `name` that has `units` UTF-16 code units.
///
/// The function returns `None` if `units` is longer than the name or splits a character.
fn utf8_len_of_prefix(name: &str, units: usize) -> Option<usize> {
    let mut counted = 0;
    for (index, c) in name.char_indices() {
        if counted == units {
            return Some(index);
        }
        counted += c.len_utf16();
    }
    (counted == units).then_some(name.len())
}

/// Parse a version 6 schema.
pub(super) fn parse_v6(data: &[u8]) -> Result<ApiSetSchema> {
    const STRUCTURE: &str = "API_SET_NAMESPACE_ENTRY_V6";
    let header = NamespaceHeaderV6::parse(data)?;
    let data = limit_to_size(
        data,
        header.size,
        NamespaceHeaderV6::SIZE,
        "API_SET_NAMESPACE_V6",
    )?;
    let records = table(
        data,
        u64::from(header.entry_offset),
        header.count,
        NamespaceEntryV6::SIZE,
        STRUCTURE,
    )?;
    let hash_records = table(
        data,
        u64::from(header.hash_offset),
        header.count,
        HashEntryV6::SIZE,
        "API_SET_HASH_ENTRY_V6",
    )?;

    let capacity = records.len() / NamespaceEntryV6::SIZE;
    let mut entries = Vec::with_capacity(capacity);
    let mut hashed_lengths = Vec::with_capacity(capacity);
    for record in records.as_chunks::<{ NamespaceEntryV6::SIZE }>().0 {
        let raw = NamespaceEntryV6::parse(record)?;
        let name = read_string(data, raw.name_offset, raw.name_length, STRUCTURE)?;
        let hashed_len = usize::try_from(raw.hashed_length / 2)
            .ok()
            .filter(|_| raw.hashed_length.is_multiple_of(2) && raw.hashed_length <= raw.name_length)
            .and_then(|units| utf8_len_of_prefix(&name, units))
            .ok_or_else(|| {
                Error::invalid_schema(format!(
                    "HashedLength {} does not fit in the name of {} bytes",
                    raw.hashed_length, raw.name_length
                ))
                .at_offset(u64::from(raw.name_offset))
                .in_structure(STRUCTURE)
            })?;
        hashed_lengths.push(hashed_len);
        entries.push(ApiSetEntry {
            name,
            flags: raw.flags,
            values: parse_values_v4(data, u64::from(raw.value_offset), raw.value_count)?,
            ..ApiSetEntry::default()
        });
    }

    let hash_table = hash_records
        .as_chunks::<{ HashEntryV6::SIZE }>()
        .0
        .iter()
        .map(|record| {
            let raw = HashEntryV6::parse(record)?;
            if raw.index >= header.count {
                return Err(Error::invalid_schema(format!(
                    "hash entry index {} is not less than the entry count {}",
                    raw.index, header.count
                ))
                .in_structure("API_SET_HASH_ENTRY_V6"));
            }
            Ok(raw)
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(ApiSetSchema {
        version: SchemaVersion::V6,
        flags: header.flags,
        hash_factor: header.hash_factor,
        entries,
        hashed_lengths,
        hash_table,
    })
}

//! Reject malformed schemas with errors, and never panic.

use apiset_rs::raw::{
    HashEntryV6, NamespaceEntryV2, NamespaceEntryV6, NamespaceHeaderV2, NamespaceHeaderV6,
    ValueEntryV2,
};
use apiset_rs::{ApiSetEntry, ApiSetSchema, ApiSetSchemaBuilder, ErrorKind, SchemaVersion};

/// Build a schema with two entries and importer hosts.
fn sample(version: SchemaVersion) -> Vec<u8> {
    let prefix = if version == SchemaVersion::V6 {
        "api-"
    } else {
        ""
    };
    ApiSetSchemaBuilder::new(version)
        .entry(ApiSetEntry::new(
            format!("{prefix}ms-win-core-synch-l1-2-0"),
            "kernelbase.dll",
        ))
        .entry(
            ApiSetEntry::new(format!("{prefix}ms-win-core-file-l1-2-4"), "kernelbase.dll")
                .with_importer("kernel32.dll", "kernel32.dll"),
        )
        .build()
}

/// Return the error kind of a parse that must fail.
fn parse_error(bytes: &[u8]) -> ErrorKind {
    ApiSetSchema::parse(bytes).unwrap_err().kind
}

#[test]
fn every_truncated_schema_is_an_error() {
    for version in [
        SchemaVersion::V2,
        SchemaVersion::V3,
        SchemaVersion::V4,
        SchemaVersion::V6,
    ] {
        let bytes = sample(version);
        for len in 0..bytes.len() {
            assert!(
                ApiSetSchema::parse(&bytes[..len]).is_err(),
                "{version:?} truncated to {len} bytes"
            );
        }
    }
}

#[test]
fn unsupported_versions_are_errors() {
    for version in [0u32, 1, 5, 7, u32::MAX] {
        let mut bytes = sample(SchemaVersion::V6);
        bytes[..4].copy_from_slice(&version.to_le_bytes());
        assert!(matches!(
            parse_error(&bytes),
            ErrorKind::UnsupportedVersion(v) if v == version
        ));
    }
}

#[test]
fn a_huge_count_is_an_error_without_a_huge_allocation() {
    let mut v2 = sample(SchemaVersion::V2);
    v2[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(
        parse_error(&v2),
        ErrorKind::OffsetOutOfBounds { .. }
    ));

    let mut v6 = sample(SchemaVersion::V6);
    v6[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(
        parse_error(&v6),
        ErrorKind::OffsetOutOfBounds { .. }
    ));
}

#[test]
fn the_size_field_limits_the_schema() {
    let bytes = sample(SchemaVersion::V6);
    let header = NamespaceHeaderV6::parse(&bytes).unwrap();

    // Extra bytes after the declared size do not matter.
    let mut padded = bytes.clone();
    padded.extend_from_slice(&[0xcc; 64]);
    assert_eq!(
        ApiSetSchema::parse(&padded).unwrap(),
        ApiSetSchema::parse(&bytes).unwrap()
    );

    // A string after the declared size is out of bounds.
    let mut short = bytes.clone();
    let size = header.size - 2;
    short[4..8].copy_from_slice(&size.to_le_bytes());
    assert!(matches!(
        parse_error(&short),
        ErrorKind::OffsetOutOfBounds { .. }
    ));

    // A size that is smaller than the header is not valid.
    let mut tiny = bytes;
    tiny[4..8].copy_from_slice(&8u32.to_le_bytes());
    assert!(matches!(parse_error(&tiny), ErrorKind::InvalidSchema(_)));
}

/// Return the offset of the first `API_SET_NAMESPACE_ENTRY_V6` and the record.
fn first_entry_v6(bytes: &[u8]) -> (usize, NamespaceEntryV6) {
    let header = NamespaceHeaderV6::parse(bytes).unwrap();
    let offset = usize::try_from(header.entry_offset).unwrap();
    (offset, NamespaceEntryV6::parse(&bytes[offset..]).unwrap())
}

#[test]
fn invalid_strings_are_errors() {
    let bytes = sample(SchemaVersion::V6);
    let (offset, entry) = first_entry_v6(&bytes);

    let mut odd = bytes.clone();
    let odd_entry = NamespaceEntryV6 {
        name_length: entry.name_length - 1,
        hashed_length: 0,
        ..entry
    };
    odd[offset..offset + NamespaceEntryV6::SIZE].copy_from_slice(&odd_entry.to_bytes());
    assert!(matches!(parse_error(&odd), ErrorKind::InvalidUtf16));

    // An unpaired high surrogate.
    let mut surrogate = bytes;
    let name = usize::try_from(entry.name_offset).unwrap();
    surrogate[name..name + 2].copy_from_slice(&0xd800u16.to_le_bytes());
    assert!(matches!(parse_error(&surrogate), ErrorKind::InvalidUtf16));
}

#[test]
fn a_hashed_length_outside_the_name_is_an_error() {
    let bytes = sample(SchemaVersion::V6);
    let (offset, entry) = first_entry_v6(&bytes);
    for hashed_length in [entry.name_length + 2, 3] {
        let mut broken = bytes.clone();
        let record = NamespaceEntryV6 {
            hashed_length,
            ..entry
        };
        broken[offset..offset + NamespaceEntryV6::SIZE].copy_from_slice(&record.to_bytes());
        assert!(
            matches!(parse_error(&broken), ErrorKind::InvalidSchema(_)),
            "HashedLength {hashed_length}"
        );
    }
}

#[test]
fn a_hash_entry_index_outside_the_entries_is_an_error() {
    let mut bytes = sample(SchemaVersion::V6);
    let header = NamespaceHeaderV6::parse(&bytes).unwrap();
    let offset = usize::try_from(header.hash_offset).unwrap();
    let slot = HashEntryV6::parse(&bytes[offset..]).unwrap();
    let broken = HashEntryV6 {
        index: header.count,
        ..slot
    };
    bytes[offset..offset + HashEntryV6::SIZE].copy_from_slice(&broken.to_bytes());
    assert!(matches!(parse_error(&bytes), ErrorKind::InvalidSchema(_)));
}

#[test]
fn an_empty_string_does_not_read_its_offset() {
    let mut bytes = sample(SchemaVersion::V2);
    let schema = ApiSetSchema::parse(&bytes).unwrap();
    // The default value of the first entry has an empty importer name. Point the name far out
    // of bounds. The value array header (4 bytes) comes before the value entry.
    let entry = NamespaceEntryV2::parse(&bytes[NamespaceHeaderV2::SIZE..]).unwrap();
    let value_offset = usize::try_from(entry.data_offset).unwrap() + 4;
    let value = ValueEntryV2::parse(&bytes[value_offset..]).unwrap();
    assert_eq!(value.name_length, 0);
    let moved = ValueEntryV2 {
        name_offset: u32::MAX,
        ..value
    };
    bytes[value_offset..value_offset + ValueEntryV2::SIZE].copy_from_slice(&moved.to_bytes());
    assert_eq!(ApiSetSchema::parse(&bytes).unwrap(), schema);
}

#[test]
fn errors_name_the_structure() {
    let bytes = sample(SchemaVersion::V6);
    let error = ApiSetSchema::parse(&bytes[..bytes.len() - 1]).unwrap_err();
    let context = error.context().unwrap();
    assert_eq!(context.structure.as_deref(), Some("API_SET_NAMESPACE_V6"));
    assert!(error.to_string().contains("API_SET_NAMESPACE_V6"));
}

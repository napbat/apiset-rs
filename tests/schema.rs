//! Build schemas of each version, and parse them again.

use apiset_rs::raw::{NamespaceHeaderV6, flags};
use apiset_rs::{
    ApiSetEntry, ApiSetSchema, ApiSetSchemaBuilder, ApiSetValue, ErrorKind, SchemaVersion,
};

/// Return entries in a mixed order, with importer hosts in a mixed order.
fn sample_entries(prefix: &str) -> Vec<ApiSetEntry> {
    vec![
        ApiSetEntry::new(
            format!("{prefix}ms-win-core-synch-l1-2-0"),
            "kernelbase.dll",
        ),
        ApiSetEntry::new(format!("{prefix}ms-win-core-file-l1-2-4"), "kernelbase.dll")
            .with_importer("ole32.dll", "combase.dll")
            .with_importer("Kernel32.dll", "kernelbase.dll")
            .with_importer("advapi32.dll", "sechost.dll"),
        ApiSetEntry::new(
            format!("{prefix}MS-Win-Core-Console-l1-1-0"),
            "kernel32.dll",
        ),
        ApiSetEntry::new(format!("{prefix}ms-win-shell-shellcom-l1-1-0"), ""),
    ]
}

/// Return the entries in the order that the builder writes.
fn expected_order(mut entries: Vec<ApiSetEntry>) -> Vec<ApiSetEntry> {
    entries.sort_by_key(|entry| entry.name.to_ascii_uppercase());
    for entry in &mut entries {
        entry.values[1..].sort_by_key(|value| value.importer.to_ascii_uppercase());
    }
    entries
}

#[test]
fn each_version_round_trips_the_sorted_entries() {
    for (version, prefix) in [
        (SchemaVersion::V2, ""),
        (SchemaVersion::V3, ""),
        (SchemaVersion::V4, ""),
        (SchemaVersion::V6, "api-"),
    ] {
        let mut entries = sample_entries(prefix);
        if version == SchemaVersion::V4 {
            entries[0].flags = flags::ENTRY_SEALED;
            entries[0].alias = "ms-win-core-synch-l1-2".into();
            entries[0].value_flags = 0x10;
            entries[0].values[0].flags = 0x20;
        }
        if version == SchemaVersion::V6 {
            entries[1].flags = flags::ENTRY_SEALED | flags::ENTRY_EXTENSION;
            entries[1].values[2].flags = 0x40;
        }
        let bytes = ApiSetSchemaBuilder::new(version)
            .flags(flags::SCHEMA_SEALED)
            .entries(entries.clone())
            .try_build()
            .unwrap();

        let schema = ApiSetSchema::parse(&bytes).unwrap();
        assert_eq!(schema.version(), version);
        assert_eq!(schema.entries(), expected_order(entries).as_slice());
        let expected_flags = match version {
            SchemaVersion::V4 | SchemaVersion::V6 => flags::SCHEMA_SEALED,
            SchemaVersion::V2 | SchemaVersion::V3 => 0,
        };
        assert_eq!(schema.flags(), expected_flags, "{version:?}");
    }
}

#[test]
fn rebuilding_a_parsed_schema_gives_the_same_bytes() {
    for (version, prefix) in [(SchemaVersion::V3, ""), (SchemaVersion::V6, "ext-")] {
        let bytes = ApiSetSchemaBuilder::new(version)
            .hash_factor(0x25)
            .entries(sample_entries(prefix))
            .try_build()
            .unwrap();
        let schema = ApiSetSchema::parse(&bytes).unwrap();
        let rebuilt = ApiSetSchemaBuilder::from_schema(&schema)
            .try_build()
            .unwrap();
        assert_eq!(rebuilt, bytes, "{version:?}");
    }
}

#[test]
fn version_6_keeps_the_hash_factor_and_the_hashed_names() {
    let bytes = ApiSetSchemaBuilder::new(SchemaVersion::V6)
        .hash_factor(0x25)
        .entries(sample_entries("api-"))
        .try_build()
        .unwrap();
    let schema = ApiSetSchema::parse(&bytes).unwrap();
    assert_eq!(schema.hash_factor(), Some(0x25));
    assert_eq!(schema.hash_table().len(), 4);
    let index = schema
        .entries()
        .iter()
        .position(|entry| entry.name == "api-ms-win-core-file-l1-2-4")
        .unwrap();
    assert_eq!(schema.hashed_name(index), Some("api-ms-win-core-file-l1-2"));
    assert_eq!(schema.hashed_name(4), None);
}

#[test]
fn declared_size_reads_the_size_field() {
    let v6 = ApiSetSchemaBuilder::new(SchemaVersion::V6)
        .entries(sample_entries("api-"))
        .build();
    let header = &v6[..NamespaceHeaderV6::SIZE];
    assert_eq!(
        ApiSetSchema::declared_size(header).unwrap(),
        Some(u32::try_from(v6.len()).unwrap())
    );

    let v2 = ApiSetSchemaBuilder::new(SchemaVersion::V2)
        .entries(sample_entries(""))
        .build();
    assert_eq!(ApiSetSchema::declared_size(&v2[..8]).unwrap(), None);
}

#[test]
fn an_empty_schema_round_trips() {
    for version in [
        SchemaVersion::V2,
        SchemaVersion::V3,
        SchemaVersion::V4,
        SchemaVersion::V6,
    ] {
        let bytes = ApiSetSchemaBuilder::new(version).build();
        let schema = ApiSetSchema::parse(&bytes).unwrap();
        assert_eq!(schema.entries(), []);
        assert_eq!(
            schema.resolve("api-ms-win-core-synch-l1-2-0.dll", None),
            apiset_rs::Resolution::NotFound
        );
    }
}

/// Return the message of an `InvalidInput` error.
fn invalid_input(builder: &ApiSetSchemaBuilder) -> String {
    match builder.try_build().unwrap_err().kind {
        ErrorKind::InvalidInput(message) => message,
        other => panic!("expected InvalidInput, got {other:?}"),
    }
}

#[test]
fn the_builder_rejects_names_that_the_loader_cannot_tell_apart() {
    let same_name = ApiSetSchemaBuilder::new(SchemaVersion::V2)
        .entry(ApiSetEntry::new("ms-win-core-a-l1-1-0", "a.dll"))
        .entry(ApiSetEntry::new("MS-WIN-CORE-A-L1-1-0", "b.dll"));
    assert!(invalid_input(&same_name).contains("duplicate entry names"));

    let same_hashed_name = ApiSetSchemaBuilder::new(SchemaVersion::V6)
        .entry(ApiSetEntry::new("api-ms-win-core-a-l1-1-0", "a.dll"))
        .entry(ApiSetEntry::new("api-ms-win-core-a-l1-1-1", "b.dll"));
    assert!(invalid_input(&same_hashed_name).contains("same hashed name"));

    // With factor 0, the hash is the last hashed character. Both names end in `1`.
    let same_hash = ApiSetSchemaBuilder::new(SchemaVersion::V6)
        .hash_factor(0)
        .entry(ApiSetEntry::new("api-ms-win-core-a-l1-1-0", "a.dll"))
        .entry(ApiSetEntry::new("api-ms-win-core-b-l1-1-0", "b.dll"));
    assert!(invalid_input(&same_hash).contains("different hash factor"));
}

#[test]
fn the_builder_rejects_invalid_entries_and_values() {
    let empty_name = ApiSetSchemaBuilder::new(SchemaVersion::V4).entry(ApiSetEntry::default());
    assert!(invalid_input(&empty_name).contains("empty name"));

    let no_hyphen =
        ApiSetSchemaBuilder::new(SchemaVersion::V6).entry(ApiSetEntry::new("apiset", "a.dll"));
    assert!(invalid_input(&no_hyphen).contains("needs a hyphen"));

    let mut no_importer = ApiSetEntry::new("ms-win-core-a-l1-1-0", "a.dll");
    no_importer.values.push(ApiSetValue::new("b.dll"));
    let no_importer = ApiSetSchemaBuilder::new(SchemaVersion::V2).entry(no_importer);
    assert!(invalid_input(&no_importer).contains("no importer"));

    let same_importer = ApiSetSchemaBuilder::new(SchemaVersion::V2).entry(
        ApiSetEntry::new("ms-win-core-a-l1-1-0", "a.dll")
            .with_importer("x.dll", "b.dll")
            .with_importer("X.DLL", "c.dll"),
    );
    assert!(invalid_input(&same_importer).contains("duplicate importer"));
}

#[test]
#[should_panic(expected = "use try_build()")]
fn build_panics_on_invalid_input() {
    let _ = ApiSetSchemaBuilder::new(SchemaVersion::V6)
        .entry(ApiSetEntry::new("apiset", "a.dll"))
        .build();
}

//! Resolve names with the rules of the Windows loader.

use apiset_rs::raw::{
    HashEntryV6, NamespaceEntryV2, NamespaceEntryV6, NamespaceHeaderV2, NamespaceHeaderV6,
    ValueArrayHeaderV2, ValueEntryV2, ValueEntryV4,
};
use apiset_rs::{
    ApiSetEntry, ApiSetSchema, ApiSetSchemaBuilder, ApiSetValue, Resolution, SchemaVersion,
};

/// Build and parse a schema.
fn schema(version: SchemaVersion, entries: Vec<ApiSetEntry>) -> ApiSetSchema {
    let bytes = ApiSetSchemaBuilder::new(version)
        .entries(entries)
        .try_build()
        .unwrap();
    ApiSetSchema::parse(&bytes).unwrap()
}

/// Return the host for a name and an importer.
fn host<'a>(schema: &'a ApiSetSchema, name: &str, importer: Option<&str>) -> Option<&'a str> {
    schema.resolve(name, importer).host()
}

#[test]
fn version_6_ignores_the_last_version_number_and_case() {
    let schema = schema(
        SchemaVersion::V6,
        vec![ApiSetEntry::new(
            "api-ms-win-core-synch-l1-2-1",
            "kernelbase.dll",
        )],
    );
    for name in [
        "api-ms-win-core-synch-l1-2-1.dll",
        "api-ms-win-core-synch-l1-2-0.dll",
        "API-MS-WIN-CORE-SYNCH-L1-2-99.DLL",
        "api-ms-win-core-synch-l1-2-0",
    ] {
        assert_eq!(host(&schema, name, None), Some("kernelbase.dll"), "{name}");
    }
    for name in [
        "api-ms-win-core-synch-l1-3-0.dll",
        "api-ms-win-core-synch-l1-2.dll",
        "api-ms-win-core-synch-l1-2",
    ] {
        assert_eq!(schema.resolve(name, None), Resolution::NotFound, "{name}");
    }
}

#[test]
fn version_6_uses_the_hash_of_windows() {
    // The hash table of the apisetschema.dll of Windows 11 has this hash for this entry.
    let header = NamespaceHeaderV6 {
        version: 6,
        size: 0,
        flags: 0,
        count: 1,
        entry_offset: 28,
        hash_offset: 52,
        hash_factor: 31,
    };
    let name: Vec<u8> = "api-ms-win-core-synch-l1-2-1"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let host_name: Vec<u8> = "kernelbase.dll"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let values_offset = 60;
    let name_offset = values_offset + 20;
    let host_offset = name_offset + u32::try_from(name.len()).unwrap();
    let entry = NamespaceEntryV6 {
        flags: 0,
        name_offset,
        name_length: u32::try_from(name.len()).unwrap(),
        hashed_length: 2 * 26,
        value_offset: values_offset,
        value_count: 1,
    };
    let value = ValueEntryV4 {
        flags: 0,
        name_offset: 0,
        name_length: 0,
        value_offset: host_offset,
        value_length: u32::try_from(host_name.len()).unwrap(),
    };
    let slot = HashEntryV6 {
        hash: 0xd885_3fb3,
        index: 0,
    };

    let mut bytes = Vec::new();
    bytes.extend_from_slice(&header.to_bytes());
    bytes.extend_from_slice(&entry.to_bytes());
    bytes.extend_from_slice(&slot.to_bytes());
    bytes.extend_from_slice(&value.to_bytes());
    bytes.extend_from_slice(&name);
    bytes.extend_from_slice(&host_name);
    let size = u32::try_from(bytes.len()).unwrap();
    bytes[4..8].copy_from_slice(&size.to_le_bytes());

    let schema = ApiSetSchema::parse(&bytes).unwrap();
    assert_eq!(
        host(&schema, "api-ms-win-core-synch-l1-2-0.dll", None),
        Some("kernelbase.dll")
    );
}

#[test]
fn versions_2_to_4_need_the_exact_name() {
    for version in [SchemaVersion::V2, SchemaVersion::V3, SchemaVersion::V4] {
        let schema = schema(
            version,
            vec![ApiSetEntry::new(
                "ms-win-core-console-l1-1-0",
                "kernel32.dll",
            )],
        );
        assert_eq!(
            host(&schema, "API-MS-Win-Core-Console-L1-1-0.dll", None),
            Some("kernel32.dll"),
            "{version:?}"
        );
        assert_eq!(
            host(&schema, "api-ms-win-core-console-l1-1-0", None),
            Some("kernel32.dll")
        );
        assert_eq!(
            schema.resolve("api-ms-win-core-console-l1-1-1.dll", None),
            Resolution::NotFound
        );
    }
}

#[test]
fn versions_2_to_4_remove_any_four_character_extension() {
    let schema = schema(
        SchemaVersion::V4,
        vec![ApiSetEntry::new(
            "ms-win-core-console-l1-1-0",
            "kernel32.dll",
        )],
    );
    // The loader checks only the period.
    assert_eq!(
        host(&schema, "api-ms-win-core-console-l1-1-0.xyz", None),
        Some("kernel32.dll")
    );
    assert_eq!(
        schema.resolve("api-ms-win-core-console-l1-1-0dll", None),
        Resolution::NotFound
    );
    assert_eq!(
        schema.resolve("api-ms-win-core-console-l1-1-0.dl", None),
        Resolution::NotFound
    );
}

#[test]
fn ext_names_are_api_sets_from_version_3() {
    let v2 = schema(
        SchemaVersion::V2,
        vec![ApiSetEntry::new(
            "ms-win-ntuser-window-l1-1-0",
            "user32.dll",
        )],
    );
    assert_eq!(
        v2.resolve("ext-ms-win-ntuser-window-l1-1-0.dll", None),
        Resolution::NotApiSet
    );
    assert!(v2.is_api_set_name("api-ms-win-ntuser-window-l1-1-0.dll"));

    let v3 = schema(
        SchemaVersion::V3,
        vec![ApiSetEntry::new(
            "ms-win-ntuser-window-l1-1-0",
            "user32.dll",
        )],
    );
    assert_eq!(
        host(&v3, "EXT-ms-win-ntuser-window-l1-1-0.dll", None),
        Some("user32.dll")
    );

    let v6 = schema(
        SchemaVersion::V6,
        vec![ApiSetEntry::new(
            "ext-ms-win-ntuser-window-l1-1-6",
            "user32.dll",
        )],
    );
    assert_eq!(
        host(&v6, "ext-ms-win-ntuser-window-l1-1-0.dll", None),
        Some("user32.dll")
    );
    // The prefix is part of the hashed name in version 6.
    assert_eq!(
        v6.resolve("api-ms-win-ntuser-window-l1-1-0.dll", None),
        Resolution::NotFound
    );
}

#[test]
fn names_without_the_prefix_are_not_api_sets() {
    let schema = schema(
        SchemaVersion::V6,
        vec![ApiSetEntry::new(
            "api-ms-win-core-synch-l1-2-0",
            "kernelbase.dll",
        )],
    );
    for name in [
        "kernel32.dll",
        "api",
        "apims-win-core-synch-l1-2-0.dll",
        "apx-a-b",
        "",
    ] {
        assert_eq!(schema.resolve(name, None), Resolution::NotApiSet, "{name}");
        assert_eq!(schema.find(name), None);
    }
}

#[test]
fn an_importer_selects_its_own_host() {
    for (version, name) in [
        (SchemaVersion::V2, "ms-win-core-file-l1-2-4"),
        (SchemaVersion::V4, "ms-win-core-file-l1-2-4"),
        (SchemaVersion::V6, "api-ms-win-core-file-l1-2-4"),
    ] {
        let schema = schema(
            version,
            vec![
                ApiSetEntry::new(name, "kernelbase.dll")
                    .with_importer("ole32.dll", "combase.dll")
                    .with_importer("kernel32.dll", "kernelbase-k32.dll")
                    .with_importer("advapi32.dll", "sechost.dll")
                    .with_importer("user32.dll", "win32u.dll"),
            ],
        );
        let query = "api-ms-win-core-file-l1-2-4.dll";
        for (importer, expected) in [
            ("ole32.dll", "combase.dll"),
            ("KERNEL32.DLL", "kernelbase-k32.dll"),
            ("advapi32.dll", "sechost.dll"),
            ("user32.dll", "win32u.dll"),
            ("other.dll", "kernelbase.dll"),
            ("", "kernelbase.dll"),
        ] {
            assert_eq!(
                host(&schema, query, Some(importer)),
                Some(expected),
                "{version:?} {importer}"
            );
        }
        assert_eq!(host(&schema, query, None), Some("kernelbase.dll"));
    }
}

#[test]
fn an_entry_without_a_host_has_no_host() {
    let mut no_values = ApiSetEntry::new("api-ms-win-a-l1-1-0", "x.dll");
    no_values.values.clear();
    let schema = schema(
        SchemaVersion::V6,
        vec![
            no_values,
            ApiSetEntry::new("api-ms-win-b-l1-1-0", "").with_importer("x.dll", "y.dll"),
        ],
    );

    let resolution = schema.resolve("api-ms-win-a-l1-1-0.dll", None);
    assert!(matches!(resolution, Resolution::NoHost(entry) if entry.name == "api-ms-win-a-l1-1-0"));
    assert_eq!(resolution.host(), None);

    assert!(matches!(
        schema.resolve("api-ms-win-b-l1-1-0.dll", None),
        Resolution::NoHost(_)
    ));
    let own = schema.resolve("api-ms-win-b-l1-1-0.dll", Some("x.dll"));
    assert_eq!(own.host(), Some("y.dll"));
    assert_eq!(
        own.entry().map(|entry| entry.name.as_str()),
        Some("api-ms-win-b-l1-1-0")
    );
}

/// Build a version 2 schema with explicit bytes. The entries keep the given order.
fn unsorted_v2(names: &[&str], host: &str) -> Vec<u8> {
    let utf16 = |s: &str| -> Vec<u8> { s.encode_utf16().flat_map(u16::to_le_bytes).collect() };
    let len = |n: usize| u32::try_from(n).unwrap();
    let count = names.len();
    let array_size = ValueArrayHeaderV2::SIZE + ValueEntryV2::SIZE;
    let arrays_offset = NamespaceHeaderV2::SIZE + count * NamespaceEntryV2::SIZE;
    let strings_offset = arrays_offset + count * array_size;
    let host_bytes = utf16(host);

    let mut records = NamespaceHeaderV2 {
        version: 2,
        count: len(count),
    }
    .to_bytes()
    .to_vec();
    let mut arrays = Vec::new();
    let mut strings = host_bytes.clone();
    for (i, name) in names.iter().enumerate() {
        let name = utf16(name);
        records.extend_from_slice(
            &NamespaceEntryV2 {
                name_offset: len(strings_offset + strings.len()),
                name_length: len(name.len()),
                data_offset: len(arrays_offset + i * array_size),
            }
            .to_bytes(),
        );
        strings.extend_from_slice(&name);
        arrays.extend_from_slice(&ValueArrayHeaderV2 { count: 1 }.to_bytes());
        arrays.extend_from_slice(
            &ValueEntryV2 {
                name_offset: 0,
                name_length: 0,
                value_offset: len(strings_offset),
                value_length: len(host_bytes.len()),
            }
            .to_bytes(),
        );
    }
    records.extend_from_slice(&arrays);
    records.extend_from_slice(&strings);
    records
}

#[test]
fn unsorted_entries_follow_the_binary_search_of_the_loader() {
    // The loader checks index 1 ("a") first, then index 2 ("b"), and stops. It never sees "c".
    let bytes = unsorted_v2(&["c", "a", "b"], "host.dll");
    let schema = ApiSetSchema::parse(&bytes).unwrap();
    assert_eq!(schema.entries()[0].name, "c");
    assert_eq!(
        schema.entries()[0].values,
        vec![ApiSetValue::new("host.dll")]
    );
    assert_eq!(schema.resolve("api-c.dll", None), Resolution::NotFound);
    assert_eq!(host(&schema, "api-a.dll", None), Some("host.dll"));
    assert_eq!(host(&schema, "api-b.dll", None), Some("host.dll"));

    // The builder sorts the entries, so that the loader finds each entry.
    let rebuilt = ApiSetSchemaBuilder::from_schema(&schema).build();
    let sorted = ApiSetSchema::parse(&rebuilt).unwrap();
    assert_eq!(host(&sorted, "api-c.dll", None), Some("host.dll"));
}

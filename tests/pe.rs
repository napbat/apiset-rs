//! Read the schema from the `.apiset` section of a PE image.
#![cfg(feature = "pe")]

use apiset_rs::{
    APISET_SECTION, ApiSetEntry, ApiSetSchema, ApiSetSchemaBuilder, ErrorKind, SchemaVersion,
};
use portex::section::characteristics;
use portex::{MachineType, PeBuilder};

/// Build a raw PE file with one section.
fn pe_file(section_name: &str, data: Vec<u8>) -> Vec<u8> {
    PeBuilder::new()
        .machine(MachineType::Amd64)
        .add_section(
            section_name,
            data,
            characteristics::INITIALIZED_DATA | characteristics::READ,
        )
        .try_build()
        .unwrap()
        .try_build()
        .unwrap()
}

#[test]
fn from_pe_reads_the_apiset_section() {
    let schema_bytes = ApiSetSchemaBuilder::new(SchemaVersion::V6)
        .entry(ApiSetEntry::new(
            "api-ms-win-core-synch-l1-2-0",
            "kernelbase.dll",
        ))
        .build();
    // The section data has file alignment padding after the schema.
    let file = pe_file(APISET_SECTION, schema_bytes.clone());

    let schema = ApiSetSchema::from_pe(&file).unwrap();
    assert_eq!(schema, ApiSetSchema::parse(&schema_bytes).unwrap());
    assert_eq!(
        schema
            .resolve("api-ms-win-core-synch-l1-2-1.dll", None)
            .host(),
        Some("kernelbase.dll")
    );
}

#[test]
fn from_pe_needs_the_apiset_section() {
    let file = pe_file(".data", vec![0; 16]);
    let error = ApiSetSchema::from_pe(&file).unwrap_err();
    assert!(matches!(error.kind, ErrorKind::MissingApiSetSection));
}

#[test]
fn from_pe_reports_a_pe_error() {
    let error = ApiSetSchema::from_pe(b"not a PE file").unwrap_err();
    assert!(matches!(error.kind, ErrorKind::Pe(_)));
    assert!(core::error::Error::source(&error).is_some());
}

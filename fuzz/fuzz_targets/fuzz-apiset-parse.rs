//! Fuzz target that parses arbitrary bytes as an API set schema, resolves names, and builds the
//! schema again.

#![no_main]

use apiset_rs::{ApiSetSchema, ApiSetSchemaBuilder};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = ApiSetSchema::declared_size(data);
    let Ok(schema) = ApiSetSchema::parse(data) else {
        return;
    };

    let importer = schema
        .entries()
        .iter()
        .flat_map(|entry| &entry.values)
        .map(|value| value.importer.as_str())
        .find(|importer| !importer.is_empty());
    for entry in schema.entries() {
        for prefix in ["", "api-", "ext-"] {
            let name = format!("{prefix}{}.dll", entry.name);
            let _ = schema.resolve(&name, importer);
        }
    }

    // A schema that builds must parse again with the same entries, in sorted order.
    if let Ok(bytes) = ApiSetSchemaBuilder::from_schema(&schema).try_build() {
        let rebuilt = ApiSetSchema::parse(&bytes).expect("built schema must parse");
        assert_eq!(rebuilt.entries().len(), schema.entries().len());
        let rebuilt_again = ApiSetSchemaBuilder::from_schema(&rebuilt)
            .try_build()
            .expect("rebuilt schema must build");
        assert_eq!(rebuilt_again, bytes);
    }
});

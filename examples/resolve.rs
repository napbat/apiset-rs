//! Resolve API set names with the schema of the host system.
//!
//! Run the example with one or more DLL names:
//!
//! ```text
//! cargo run --example resolve --features pe -- api-ms-win-core-synch-l1-2-0.dll
//! cargo run --example resolve --features pe -- --importer kernel32.dll api-ms-win-core-file-l1-2-4.dll
//! ```
//!
//! With no names, the example prints each entry and its hosts. Use `--schema PATH` to read a
//! different `apisetschema.dll`.

use apiset_rs::{ApiSetSchema, Resolution};
use std::path::PathBuf;
use std::process::ExitCode;

/// Return the path of `apisetschema.dll` of the host system.
fn default_schema_path() -> PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    PathBuf::from(root).join(r"System32\apisetschema.dll")
}

/// Print each entry of the schema and its hosts.
fn print_entries(schema: &ApiSetSchema) {
    println!(
        "schema version {}, flags {:#x}, {} entries",
        schema.version().as_u32(),
        schema.flags(),
        schema.entries().len()
    );
    for entry in schema.entries() {
        let hosts: Vec<String> = entry
            .values
            .iter()
            .map(|value| {
                if value.importer.is_empty() {
                    value.host.clone()
                } else {
                    format!("{} (for {})", value.host, value.importer)
                }
            })
            .collect();
        println!("{} -> {}", entry.name, hosts.join(", "));
    }
}

fn main() -> ExitCode {
    let mut schema_path = default_schema_path();
    let mut importer = None;
    let mut names = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--schema" => schema_path = args.next().map(PathBuf::from).unwrap_or_default(),
            "--importer" => importer = args.next(),
            _ => names.push(arg),
        }
    }

    let schema = match ApiSetSchema::from_pe_file(&schema_path) {
        Ok(schema) => schema,
        Err(e) => {
            eprintln!("{}: {e}", schema_path.display());
            return ExitCode::FAILURE;
        }
    };

    if names.is_empty() {
        print_entries(&schema);
        return ExitCode::SUCCESS;
    }

    for name in &names {
        match schema.resolve(name, importer.as_deref()) {
            Resolution::Host { entry, value } => {
                println!("{name} -> {} (entry {})", value.host, entry.name);
            }
            Resolution::NoHost(entry) => println!("{name} -> no host (entry {})", entry.name),
            Resolution::NotFound => println!("{name} -> not found"),
            Resolution::NotApiSet => println!("{name} -> not an API set"),
        }
    }
    ExitCode::SUCCESS
}

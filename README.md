# apiset-rs

apiset-rs is a parser, builder and resolver for the Windows API set schema. It works with
`no_std`. It resolves names with the rules of the Windows loader for each schema version.

## What it supports

- Schema versions 2 (Windows 7), 3 (Windows 8), 4 (Windows 8.1) and 6 (Windows 10 and later).
- Resolution of an API set name to its host DLL, as `ApiSetResolveToHost` in `ntdll.dll` does:
  - the `api-` and `ext-` prefix checks of each version;
  - the version 6 hash and the version-agnostic match, which ignores the last version number;
  - the importer-specific hosts, for example a different host for `kernel32.dll`;
  - the binary searches of the loader, also on data that is not sorted.
- A builder that writes a schema of each version, with sorted entries, sorted values and the
  version 6 hash table.
- Parsing and encoding of each fixed-size record, for example `API_SET_NAMESPACE_ENTRY_V6`.
- The `.apiset` section of `apisetschema.dll`, through [`portex`](https://crates.io/crates/portex)
  (the `pe` feature).
- `no_std + alloc` support. Malformed input gives an error. It does not cause a panic.

## Installation

```toml
[dependencies]
apiset-rs = { version = "0.1", features = ["pe"] }
```

For `no_std + alloc`:

```toml
[dependencies]
apiset-rs = { version = "0.1", default-features = false }
```

apiset-rs 0.1 requires Rust 1.88 or newer.

| Feature | Default | Adds |
| --- | --- | --- |
| `std` | yes | `ApiSetSchema::from_pe_file`, together with `pe` |
| `pe` | no | `ApiSetSchema::from_pe` and `from_pe_image`, through `portex` |

## Core types

| Type | Use it for |
| --- | --- |
| `ApiSetSchema` | A parsed schema. It finds and resolves names |
| `ApiSetEntry` | One API set: its name, flags and values |
| `ApiSetValue` | One host of an entry, for all importers or for one importer |
| `Resolution` | The result of `ApiSetSchema::resolve` |
| `ApiSetSchemaBuilder` | Building schema bytes |
| `raw::*` | The fixed-size records of each version |

## Quick start

Read the schema of the host system, and resolve a name:

```rust,no_run
use apiset_rs::{ApiSetSchema, Resolution};

let schema = ApiSetSchema::from_pe_file(r"C:\Windows\System32\apisetschema.dll")?;

match schema.resolve("api-ms-win-core-synch-l1-2-0.dll", None) {
    Resolution::Host { entry, value } => println!("{} -> {}", entry.name, value.host),
    Resolution::NoHost(entry) => println!("{} has no host", entry.name),
    Resolution::NotFound => println!("unknown API set"),
    Resolution::NotApiSet => println!("not an API set name"),
}

// A module can have its own host. On Windows 11, this API set resolves to kernel32.dll, but
// kernel32.dll itself gets kernelbase.dll.
let host = schema.resolve("api-ms-win-core-processthreads-l1-1-8.dll", Some("kernel32.dll"));
println!("{:?}", host.host());
# Ok::<(), apiset_rs::Error>(())
```

To read the schema of a running process, read the memory at `PEB.ApiSetMap`. First read the
header, and give it to `ApiSetSchema::declared_size`. Then read the declared number of bytes,
and give them to `ApiSetSchema::parse`.

## Name resolution

The rules change with the schema version:

| Version | Prefixes | Stored name | Match |
| --- | --- | --- | --- |
| 2 | `api-` | No prefix, no `.dll` | Exact name |
| 3, 4 | `api-`, `ext-` | No prefix, no `.dll` | Exact name |
| 6 | `api-`, `ext-` | Prefix, no `.dll` | Name up to the last hyphen |

All comparisons ignore case. In versions 2 to 4, the loader removes the last four characters
if the first of them is a period. It does not check the letters `dll`.

In version 6, `api-ms-win-core-synch-l1-2-0.dll` and `api-ms-win-core-synch-l1-2-9.dll` find
the same entry, `api-ms-win-core-synch-l1-2-1`. The loader hashes the name up to the last
hyphen, and finds the hash in the hash table of the schema.

Each entry has a list of values. The first value is the default host. Each other value names an
importing module and its host. If the importer has no value, the loader uses the default host.

`resolve` returns `NoHost` if an entry has no values or an empty host. The Windows schema has
many such entries, for API sets that the edition of Windows does not implement.

## Build a schema

```rust
use apiset_rs::{ApiSetEntry, ApiSetSchema, ApiSetSchemaBuilder, SchemaVersion};

let bytes = ApiSetSchemaBuilder::new(SchemaVersion::V6)
    .entry(ApiSetEntry::new("api-ms-win-core-synch-l1-2-0", "kernelbase.dll"))
    .entry(
        ApiSetEntry::new("api-ms-win-core-processthreads-l1-1-8", "kernelbase.dll")
            .with_importer("kernel32.dll", "kernelbase.dll"),
    )
    .try_build()?;

let schema = ApiSetSchema::parse(&bytes)?;
assert_eq!(
    schema.resolve("api-ms-win-core-synch-l1-2-0.dll", None).host(),
    Some("kernelbase.dll")
);
# Ok::<(), apiset_rs::Error>(())
```

The builder sorts the entries by name and the importer values by importer name. The first
value of each entry stays first. To change a parsed schema, use
`ApiSetSchemaBuilder::from_schema` and `entries_mut`.

The builder writes each distinct string one time, so its output is not byte-for-byte equal to
a schema from Windows. A parse of the output gives the same entries in sorted order. The
builder returns an error for input that the loader cannot resolve correctly, for example two
version 6 names with the same hashed part.

## Example program

`examples/resolve.rs` reads the schema of the host system:

```text
cargo run --example resolve --features pe -- api-ms-win-core-synch-l1-2-0.dll
cargo run --example resolve --features pe -- --importer kernel32.dll api-ms-win-core-io-l1-1-1.dll
cargo run --example resolve --features pe
```

With no names, the program prints each entry and its hosts.

## Development

[`mise`](https://mise.jdx.dev/) manages the toolchain and the pre-commit hooks:

```bash
mise install
mise run precommit-install
mise run precommit
```

`AGENTS.md` lists the rules and the checks for each change.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in
the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without
any additional terms or conditions.

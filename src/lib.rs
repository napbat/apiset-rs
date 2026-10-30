//! # apiset-rs
//!
//! A `no_std`-friendly library that parses, builds and resolves the Windows API set schema.
//!
//! An API set is a virtual DLL name, for example `api-ms-win-core-synch-l1-2-0.dll`. The
//! loader does not load a file for it. It looks up the name in the API set schema, and loads
//! the host DLL, for example `kernelbase.dll`. Windows keeps the schema in the `.apiset`
//! section of `apisetschema.dll`, and maps it at `PEB.ApiSetMap` in each process.
//!
//! ## Features
//!
//! - **All schema versions**: Parse versions 2 (Windows 7), 3 (Windows 8), 4 (Windows 8.1) and
//!   6 (Windows 10 and later) with [`ApiSetSchema::parse`].
//! - **Loader resolution**: [`ApiSetSchema::resolve`] follows `ApiSetResolveToHost` of each
//!   version. It uses the same prefix checks, hash, binary searches and importer-specific
//!   hosts.
//! - **Builder**: [`ApiSetSchemaBuilder`] writes a schema of each version.
//! - **Raw records**: The [`raw`] module parses and encodes each fixed-size schema record.
//! - **PE input**: With the `pe` feature, `ApiSetSchema::from_pe` reads the `.apiset`
//!   section of `apisetschema.dll` through `portex`.
//! - **`no_std` mode**: Disable the `std` feature for `no_std + alloc` builds. Only
//!   `ApiSetSchema::from_pe_file` needs `std`.
//!
//! ## Example
//!
//! ```
//! use apiset_rs::{ApiSetEntry, ApiSetSchema, ApiSetSchemaBuilder, Resolution, SchemaVersion};
//!
//! let bytes = ApiSetSchemaBuilder::new(SchemaVersion::V6)
//!     .entry(ApiSetEntry::new("api-ms-win-core-synch-l1-2-0", "kernelbase.dll"))
//!     .try_build()?;
//!
//! let schema = ApiSetSchema::parse(&bytes)?;
//! match schema.resolve("api-ms-win-core-synch-l1-2-0.dll", None) {
//!     Resolution::Host { value, .. } => println!("host: {}", value.host),
//!     other => println!("no host: {other:?}"),
//! }
//! # Ok::<(), apiset_rs::Error>(())
//! ```

#![cfg_attr(not(feature = "std"), no_std)]
extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

/// Internal prelude. Each source file uses `use crate::prelude::*;` to get the `alloc`
/// types and macros that the code needs.
///
/// With this prelude, the import header of each module is one line. It also keeps the
/// `no_std` imports in one location, so modules do not repeat `#[cfg]` blocks. All items
/// are `alloc` re-exports. Under the `std` feature, they shadow prelude items of the same
/// type, and this causes no problem.
pub(crate) mod prelude {
    #[cfg(feature = "pe")]
    pub use alloc::boxed::Box;
    pub use alloc::format;
    pub use alloc::string::String;
    pub use alloc::vec;
    pub use alloc::vec::Vec;
}

pub mod builder;
pub mod error;
#[cfg(feature = "pe")]
pub mod pe;
pub mod raw;
pub mod schema;
mod utf16;

pub use builder::ApiSetSchemaBuilder;
pub use error::{Error, ErrorContext, ErrorKind, Result};
#[cfg(feature = "pe")]
pub use pe::APISET_SECTION;
pub use schema::{ApiSetEntry, ApiSetSchema, ApiSetValue, Resolution, SchemaVersion};

// Compile the Rust examples in the README as doctests, so that they follow the API. The
// quick start uses `ApiSetSchema::from_pe_file`.
#[cfg(all(doctest, feature = "std", feature = "pe"))]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

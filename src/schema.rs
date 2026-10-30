//! Parse the API set schema and resolve API set names.
//!
//! The API set schema maps API set names (for example `api-ms-win-core-synch-l1-2-0.dll`) to
//! host DLLs (for example `kernelbase.dll`). Windows keeps the schema in the `.apiset` section
//! of `apisetschema.dll`. The loader maps it into each process, and `PEB.ApiSetMap` points to
//! it.
//!
//! [`ApiSetSchema::parse`] reads schema versions 2 (Windows 7), 3 (Windows 8), 4 (Windows 8.1)
//! and 6 (Windows 10 and later). [`ApiSetSchema::resolve`] finds the host of a name with the
//! rules of the loader for the schema version.
//!
//! # Example
//!
//! ```
//! use apiset_rs::{ApiSetEntry, ApiSetSchema, ApiSetSchemaBuilder, SchemaVersion};
//!
//! let bytes = ApiSetSchemaBuilder::new(SchemaVersion::V6)
//!     .entry(ApiSetEntry::new("api-ms-win-core-synch-l1-2-0", "kernelbase.dll"))
//!     .try_build()?;
//!
//! let schema = ApiSetSchema::parse(&bytes)?;
//! let resolution = schema.resolve("api-ms-win-core-synch-l1-2-0.dll", None);
//! assert_eq!(resolution.host(), Some("kernelbase.dll"));
//! # Ok::<(), apiset_rs::Error>(())
//! ```

mod parse;
mod resolve;

use crate::prelude::*;
use crate::raw::{HashEntryV6, NamespaceHeaderV4, NamespaceHeaderV6};
use crate::{Error, Result};

pub use resolve::Resolution;

/// The version of an API set schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SchemaVersion {
    /// Version 2 (Windows 7). Names have no `api-` prefix. Only `api-` names are API sets.
    V2,
    /// Version 3 (Windows 8). The layout is the same as version 2. `ext-` names are also API
    /// sets.
    V3,
    /// Version 4 (Windows 8.1). Entries have flags and an alias.
    V4,
    /// Version 6 (Windows 10 and later). Names have the prefix, and a hash table finds them.
    V6,
}

impl SchemaVersion {
    /// Return the version for the `Version` field of a schema, or `None` if the version is not
    /// supported.
    #[must_use]
    pub fn from_u32(version: u32) -> Option<Self> {
        match version {
            2 => Some(Self::V2),
            3 => Some(Self::V3),
            4 => Some(Self::V4),
            6 => Some(Self::V6),
            _ => None,
        }
    }

    /// Return the value of the `Version` field for this version.
    #[must_use]
    pub fn as_u32(self) -> u32 {
        match self {
            Self::V2 => 2,
            Self::V3 => 3,
            Self::V4 => 4,
            Self::V6 => 6,
        }
    }

    /// Read the `Version` field at the start of a schema.
    ///
    /// # Errors
    ///
    /// Returns an error if `data` is shorter than 4 bytes, or if the version is not 2, 3, 4 or 6.
    pub fn detect(data: &[u8]) -> Result<Self> {
        let Some(&bytes) = data.first_chunk::<4>() else {
            return Err(Error::buffer_too_small(4, data.len()).in_structure("API_SET_NAMESPACE"));
        };
        let version = u32::from_le_bytes(bytes);
        Self::from_u32(version).ok_or_else(|| Error::unsupported_version(version))
    }
}

/// One host of an API set entry: an `API_SET_VALUE_ENTRY`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ApiSetValue {
    /// Name of the importing module that gets this host, for example `kernel32.dll`. The value
    /// is empty for the default host.
    pub importer: String,
    /// Name of the host DLL, for example `kernelbase.dll`. An empty host means that the API set
    /// has no implementation.
    pub host: String,
    /// Value flags (V4 and V6). Zero in other versions.
    pub flags: u32,
}

impl ApiSetValue {
    /// Create a default host: a value with no importer.
    pub fn new(host: impl Into<String>) -> Self {
        Self {
            importer: String::new(),
            host: host.into(),
            flags: 0,
        }
    }

    /// Create a host for one importing module.
    pub fn for_importer(importer: impl Into<String>, host: impl Into<String>) -> Self {
        Self {
            importer: importer.into(),
            host: host.into(),
            flags: 0,
        }
    }
}

/// One API set of the schema: an `API_SET_NAMESPACE_ENTRY`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ApiSetEntry {
    /// The API set name as the schema stores it, with no `.dll` suffix.
    ///
    /// In version 6, the name has the `api-` or `ext-` prefix (for example
    /// `api-ms-win-core-synch-l1-2-0`). In versions 2, 3 and 4, the name has no prefix (for
    /// example `ms-win-core-synch-l1-2-0`).
    pub name: String,
    /// Entry flags (V4 and V6, see [`crate::raw::flags`]). Zero in other versions.
    pub flags: u32,
    /// The alias of the entry (V4 only). The loader does not use the alias. Empty in other
    /// versions.
    pub alias: String,
    /// Flags of the value array (V4 only). Zero in other versions.
    pub value_flags: u32,
    /// The hosts of the API set. The first value is the default host. The other values are
    /// hosts for one importing module each.
    pub values: Vec<ApiSetValue>,
}

impl ApiSetEntry {
    /// Create an entry with one default host.
    pub fn new(name: impl Into<String>, host: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            values: vec![ApiSetValue::new(host)],
            ..Self::default()
        }
    }

    /// Add a host for one importing module.
    #[must_use]
    pub fn with_importer(mut self, importer: impl Into<String>, host: impl Into<String>) -> Self {
        self.values.push(ApiSetValue::for_importer(importer, host));
        self
    }

    /// Return the default host, or `None` if the entry has no values.
    #[must_use]
    pub fn default_host(&self) -> Option<&str> {
        self.values.first().map(|value| value.host.as_str())
    }
}

/// A parsed API set schema.
///
/// The schema keeps the order of the entries, the values and the hash table of the source.
/// [`Self::resolve`] uses the binary searches of the loader on this order. To change a schema,
/// use [`crate::ApiSetSchemaBuilder::from_schema`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiSetSchema {
    version: SchemaVersion,
    flags: u32,
    hash_factor: u32,
    entries: Vec<ApiSetEntry>,
    /// V6 only: for each entry, the length in bytes of the UTF-8 name prefix that the
    /// `HashedLength` field covers.
    hashed_lengths: Vec<usize>,
    /// V6 only: the hash table.
    hash_table: Vec<HashEntryV6>,
}

impl ApiSetSchema {
    /// Parse an API set schema from the start of `data`.
    ///
    /// `data` is the `.apiset` section of `apisetschema.dll`, or the memory at
    /// `PEB.ApiSetMap`. For versions 4 and 6, the parser reads only the bytes that the `Size`
    /// field of the header declares.
    ///
    /// # Errors
    ///
    /// Returns an error if the version is not supported, or if a structure, an offset or a
    /// string is not valid.
    pub fn parse(data: &[u8]) -> Result<Self> {
        match SchemaVersion::detect(data)? {
            version @ (SchemaVersion::V2 | SchemaVersion::V3) => parse::parse_v2(data, version),
            SchemaVersion::V4 => parse::parse_v4(data),
            SchemaVersion::V6 => parse::parse_v6(data),
        }
    }

    /// Return the size of the schema that the header declares.
    ///
    /// Use this function to read a schema from a different process. Read the header first,
    /// then read the declared number of bytes. Versions 2 and 3 do not declare a size, and the
    /// function returns `None` for them.
    ///
    /// # Errors
    ///
    /// Returns an error if the version is not supported, or if `header` is shorter than the
    /// header of its version.
    pub fn declared_size(header: &[u8]) -> Result<Option<u32>> {
        match SchemaVersion::detect(header)? {
            SchemaVersion::V2 | SchemaVersion::V3 => Ok(None),
            SchemaVersion::V4 => Ok(Some(NamespaceHeaderV4::parse(header)?.size)),
            SchemaVersion::V6 => Ok(Some(NamespaceHeaderV6::parse(header)?.size)),
        }
    }

    /// Return the schema version.
    #[must_use]
    pub fn version(&self) -> SchemaVersion {
        self.version
    }

    /// Return the schema flags (V4 and V6, see [`crate::raw::flags`]). Zero in other versions.
    #[must_use]
    pub fn flags(&self) -> u32 {
        self.flags
    }

    /// Return the hash factor (V6 only), or `None` in other versions.
    #[must_use]
    pub fn hash_factor(&self) -> Option<u32> {
        (self.version == SchemaVersion::V6).then_some(self.hash_factor)
    }

    /// Return the entries in schema order.
    #[must_use]
    pub fn entries(&self) -> &[ApiSetEntry] {
        &self.entries
    }

    /// Return the hash table (V6 only). The slice is empty in other versions.
    #[must_use]
    pub fn hash_table(&self) -> &[HashEntryV6] {
        &self.hash_table
    }

    /// Return the hashed part of the name of the entry at `index` (V6 only).
    ///
    /// The hashed part is the name that the loader compares. It has no version suffix, for
    /// example `api-ms-win-core-synch-l1-2`. The function returns `None` in other versions, or
    /// if `index` is out of range.
    #[must_use]
    pub fn hashed_name(&self, index: usize) -> Option<&str> {
        let entry = self.entries.get(index)?;
        let len = *self.hashed_lengths.get(index)?;
        entry.name.get(..len)
    }
}

//! Parse and encode the fixed-size records of the API set schema.
//!
//! Each record is a sequence of little-endian `ULONG` fields. All offset fields are relative to
//! the start of the schema (the namespace header). All length fields count bytes, not UTF-16
//! code units.
//!
//! Each record type has the name of the first schema version that uses it. A later version can
//! use the same record. For example, schema version 6 uses [`ValueEntryV4`].
//!
//! # Example
//!
//! ```
//! use apiset_rs::raw::NamespaceHeaderV6;
//!
//! let header = NamespaceHeaderV6 {
//!     version: 6,
//!     size: 28,
//!     flags: 0,
//!     count: 0,
//!     entry_offset: 28,
//!     hash_offset: 28,
//!     hash_factor: 0x1f,
//! };
//! assert_eq!(NamespaceHeaderV6::parse(&header.to_bytes())?, header);
//! # Ok::<(), apiset_rs::Error>(())
//! ```

use crate::{Error, Result};

/// Flags of `API_SET_NAMESPACE` (V4 and V6) and of its entries.
pub mod flags {
    /// `API_SET_SCHEMA_FLAGS_SEALED`: the schema is sealed.
    pub const SCHEMA_SEALED: u32 = 0x1;
    /// `API_SET_SCHEMA_FLAGS_HOST_EXTENSION`: the schema extends a host schema.
    pub const SCHEMA_HOST_EXTENSION: u32 = 0x2;
    /// `API_SET_SCHEMA_ENTRY_FLAGS_SEALED`: the entry is sealed.
    pub const ENTRY_SEALED: u32 = 0x1;
    /// `API_SET_SCHEMA_ENTRY_FLAGS_EXTENSION`: the entry is an extension (`ext-`) API set.
    pub const ENTRY_EXTENSION: u32 = 0x2;
}

/// Define a record of `u32` fields with `SIZE`, `parse` and `to_bytes`.
macro_rules! u32_record {
    (
        $(#[$meta:meta])*
        $name:ident = $windows_name:literal, $size:literal {
            $( $(#[$field_meta:meta])* $field:ident ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
        pub struct $name {
            $( $(#[$field_meta])* pub $field: u32, )+
        }

        const _: () = assert!($size == 4 * [$(stringify!($field)),+].len());

        impl $name {
            #[doc = concat!("Size of an encoded `", $windows_name, "`, in bytes.")]
            pub const SIZE: usize = $size;

            #[doc = concat!("Parse a `", $windows_name, "` from little-endian bytes.")]
            ///
            /// # Errors
            ///
            /// Returns an error if `data` is shorter than [`Self::SIZE`].
            pub fn parse(data: &[u8]) -> Result<Self> {
                let Some(data) = data.first_chunk::<{ Self::SIZE }>() else {
                    return Err(Error::buffer_too_small(Self::SIZE, data.len())
                        .in_structure($windows_name));
                };
                let mut words = data
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|&word| u32::from_le_bytes(word));
                Ok(Self {
                    $( $field: words.next().unwrap_or_default(), )+
                })
            }

            #[doc = concat!("Encode the `", $windows_name, "` into little-endian bytes.")]
            #[must_use]
            pub fn to_bytes(self) -> [u8; $size] {
                let mut buf = [0u8; $size];
                let mut chunks = buf.as_chunks_mut::<4>().0.iter_mut();
                $(
                    if let Some(chunk) = chunks.next() {
                        *chunk = self.$field.to_le_bytes();
                    }
                )+
                buf
            }
        }
    };
}

u32_record! {
    /// `API_SET_NAMESPACE_ARRAY_V2`: the header of a version 2 or version 3 schema.
    ///
    /// An array of `count` [`NamespaceEntryV2`] records follows the header.
    NamespaceHeaderV2 = "API_SET_NAMESPACE_ARRAY_V2", 8 {
        /// Schema version (2 or 3).
        version,
        /// Number of namespace entries.
        count,
    }
}

u32_record! {
    /// `API_SET_NAMESPACE_ENTRY_V2`: one API set of a version 2 or version 3 schema.
    ///
    /// The name has no `api-` prefix and no `.dll` suffix.
    NamespaceEntryV2 = "API_SET_NAMESPACE_ENTRY_V2", 12 {
        /// Offset of the UTF-16 name.
        name_offset,
        /// Length of the name, in bytes.
        name_length,
        /// Offset of the [`ValueArrayHeaderV2`] of the entry.
        data_offset,
    }
}

u32_record! {
    /// `API_SET_VALUE_ARRAY_V2`: the header of the value array of a version 2 or version 3 entry.
    ///
    /// An array of `count` [`ValueEntryV2`] records follows the header.
    ValueArrayHeaderV2 = "API_SET_VALUE_ARRAY_V2", 4 {
        /// Number of value entries.
        count,
    }
}

u32_record! {
    /// `API_SET_VALUE_ENTRY_V2`: one host of a version 2 or version 3 entry.
    ValueEntryV2 = "API_SET_VALUE_ENTRY_V2", 16 {
        /// Offset of the UTF-16 name of the importing module.
        name_offset,
        /// Length of the importing module name, in bytes. Zero for the default host.
        name_length,
        /// Offset of the UTF-16 host name.
        value_offset,
        /// Length of the host name, in bytes.
        value_length,
    }
}

u32_record! {
    /// `API_SET_NAMESPACE_ARRAY_V4`: the header of a version 4 schema.
    ///
    /// An array of `count` [`NamespaceEntryV4`] records follows the header.
    NamespaceHeaderV4 = "API_SET_NAMESPACE_ARRAY_V4", 16 {
        /// Schema version (4).
        version,
        /// Size of the complete schema, in bytes.
        size,
        /// Schema flags (see [`flags`]).
        flags,
        /// Number of namespace entries.
        count,
    }
}

u32_record! {
    /// `API_SET_NAMESPACE_ENTRY_V4`: one API set of a version 4 schema.
    ///
    /// The name has no `api-` or `ext-` prefix and no `.dll` suffix.
    NamespaceEntryV4 = "API_SET_NAMESPACE_ENTRY_V4", 24 {
        /// Entry flags (see [`flags`]).
        flags,
        /// Offset of the UTF-16 name.
        name_offset,
        /// Length of the name, in bytes.
        name_length,
        /// Offset of the UTF-16 alias. The loader does not use the alias.
        alias_offset,
        /// Length of the alias, in bytes.
        alias_length,
        /// Offset of the [`ValueArrayHeaderV4`] of the entry.
        data_offset,
    }
}

u32_record! {
    /// `API_SET_VALUE_ARRAY_V4`: the header of the value array of a version 4 entry.
    ///
    /// An array of `count` [`ValueEntryV4`] records follows the header.
    ValueArrayHeaderV4 = "API_SET_VALUE_ARRAY_V4", 8 {
        /// Value array flags.
        flags,
        /// Number of value entries.
        count,
    }
}

u32_record! {
    /// `API_SET_VALUE_ENTRY_V4` (also `API_SET_VALUE_ENTRY_V6`): one host of a version 4 or
    /// version 6 entry.
    ValueEntryV4 = "API_SET_VALUE_ENTRY", 20 {
        /// Value flags.
        flags,
        /// Offset of the UTF-16 name of the importing module.
        name_offset,
        /// Length of the importing module name, in bytes. Zero for the default host.
        name_length,
        /// Offset of the UTF-16 host name.
        value_offset,
        /// Length of the host name, in bytes.
        value_length,
    }
}

u32_record! {
    /// `API_SET_NAMESPACE_V6`: the header of a version 6 schema.
    NamespaceHeaderV6 = "API_SET_NAMESPACE_V6", 28 {
        /// Schema version (6).
        version,
        /// Size of the complete schema, in bytes.
        size,
        /// Schema flags (see [`flags`]).
        flags,
        /// Number of namespace entries and of hash entries.
        count,
        /// Offset of the array of [`NamespaceEntryV6`] records.
        entry_offset,
        /// Offset of the array of [`HashEntryV6`] records.
        hash_offset,
        /// Multiplier of the name hash.
        hash_factor,
    }
}

u32_record! {
    /// `API_SET_NAMESPACE_ENTRY_V6`: one API set of a version 6 schema.
    ///
    /// The name has the `api-` or `ext-` prefix, but no `.dll` suffix.
    NamespaceEntryV6 = "API_SET_NAMESPACE_ENTRY_V6", 24 {
        /// Entry flags (see [`flags`]).
        flags,
        /// Offset of the UTF-16 name.
        name_offset,
        /// Length of the name, in bytes.
        name_length,
        /// Length of the hashed part of the name, in bytes. The hashed part ends before the
        /// last hyphen.
        hashed_length,
        /// Offset of the array of [`ValueEntryV4`] records.
        value_offset,
        /// Number of value entries.
        value_count,
    }
}

u32_record! {
    /// `API_SET_HASH_ENTRY_V6`: one entry of the hash table of a version 6 schema.
    ///
    /// The hash table is sorted by `hash`.
    HashEntryV6 = "API_SET_HASH_ENTRY_V6", 8 {
        /// Hash of the hashed part of the entry name.
        hash,
        /// Index of the [`NamespaceEntryV6`] record.
        index,
    }
}

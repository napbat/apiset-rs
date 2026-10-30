//! UTF-16 string helpers: bounded reads, case folding and the loader comparisons.

use crate::prelude::*;
use crate::{Error, Result};
use core::cmp::Ordering;

/// Return the bytes of the range `offset..offset + len` in `data`.
pub(crate) fn range<'a>(
    data: &'a [u8],
    offset: u64,
    len: u64,
    structure: &str,
) -> Result<&'a [u8]> {
    let end = offset
        .checked_add(len)
        .ok_or_else(|| Error::offset_out_of_bounds(u64::MAX, data.len()).in_structure(structure))?;
    let bounds = usize::try_from(offset)
        .ok()
        .zip(usize::try_from(end).ok())
        .and_then(|(start, end)| data.get(start..end));
    bounds.ok_or_else(|| {
        Error::offset_out_of_bounds(end, data.len())
            .at_offset(offset)
            .in_structure(structure)
    })
}

/// Read a UTF-16LE string of `length` bytes at `offset` in `data`.
///
/// A zero `length` gives an empty string. The loader does not read the offset in that case, so
/// the offset is not checked.
pub(crate) fn read_string(
    data: &[u8],
    offset: u32,
    length: u32,
    structure: &str,
) -> Result<String> {
    if length == 0 {
        return Ok(String::new());
    }
    if !length.is_multiple_of(2) {
        return Err(Error::invalid_utf16()
            .at_offset(u64::from(offset))
            .in_structure(structure));
    }
    let bytes = range(data, u64::from(offset), u64::from(length), structure)?;
    let units = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&pair| u16::from_le_bytes(pair));
    char::decode_utf16(units)
        .collect::<core::result::Result<String, _>>()
        .map_err(|_| {
            Error::invalid_utf16()
                .at_offset(u64::from(offset))
                .in_structure(structure)
        })
}

/// Convert one UTF-16 code unit to uppercase, as `RtlUpcaseUnicodeChar` does.
///
/// The function uses the Unicode uppercase mapping of the code unit if the mapping is one
/// character in the Basic Multilingual Plane. Otherwise it keeps the code unit. API set names
/// and host names use ASCII, and for ASCII the result is the same as the Windows table.
pub(crate) fn upcase(unit: u16) -> u16 {
    if (u16::from(b'a')..=u16::from(b'z')).contains(&unit) {
        return unit - 0x20;
    }
    if unit < 0x80 {
        return unit;
    }
    let Some(c) = char::from_u32(u32::from(unit)) else {
        return unit;
    };
    let mut upper = c.to_uppercase();
    match (upper.next(), upper.next()) {
        (Some(u), None) => u16::try_from(u32::from(u)).unwrap_or(unit),
        _ => unit,
    }
}

/// Compare two strings as UTF-16 and ignore case, as `RtlCompareUnicodeStrings` does.
///
/// The function compares the uppercase code units of the common prefix. If the prefixes are
/// equal, the shorter string is less.
pub(crate) fn compare_ignore_case(a: &str, b: &str) -> Ordering {
    let mut a = a.encode_utf16();
    let mut b = b.encode_utf16();
    loop {
        match (a.next(), b.next()) {
            (Some(x), Some(y)) => match upcase(x).cmp(&upcase(y)) {
                Ordering::Equal => {}
                other => return other,
            },
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
        }
    }
}

/// Calculate the V6 hash of a name, as `ApiSetpSearchForApiSet` does.
///
/// The function hashes the UTF-16 code units. It converts only the ASCII letters `A` to `Z` to
/// lowercase.
pub(crate) fn hash(name: &str, factor: u32) -> u32 {
    name.encode_utf16().fold(0u32, |hash, unit| {
        let lower = if unit.wrapping_sub(u16::from(b'A')) <= 25 {
            unit + 0x20
        } else {
            unit
        };
        hash.wrapping_mul(factor).wrapping_add(u32::from(lower))
    })
}

//! Find the `apisetschema.dll` payloads in an update package.
//!
//! The tool supports these package forms:
//!
//! - An MSU that is a WIM file (Windows 11 24H2 and later). It holds an LZMS-compressed WIM
//!   with `express.psf.cix.xml`, and a `.psf` file with the payloads. The tool reads only the
//!   ranges that it needs.
//! - An MSU or a CAB that is a cabinet file (earlier releases). It holds an inner cabinet with
//!   the package. The inner cabinet holds `express.psf.cix.xml` and a `.psf` file is next to
//!   it, or the inner cabinet holds the payloads itself. A cabinet folder is one compressed
//!   stream, so the tool reads the folder up to the last payload that it needs.
//!
//! Each payload is below a component directory, for example
//! `amd64_microsoft-windows-apisetschema-wnc_31bf3856ad364e35_10.0.26100.9549_none_6d84...`.
//! A forward delta is in the `f\` subdirectory. A reverse delta (`r\`) is not used.

use crate::Result;
use crate::http::HttpFile;
use std::collections::BTreeMap;
use std::io::{self, Read};
use winupdate::cix::{PsfFile, PsfIndex, SourceKind};
use winupdate::{Cab, ReadAt, Wim};

/// The name of the file in the payloads.
const FILE_NAME: &str = "apisetschema.dll";
/// The name of the PSF index.
const INDEX_NAME: &str = "express.psf.cix.xml";

/// The content of one payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Payload {
    /// The complete file.
    Full(Vec<u8>),
    /// A forward delta (PA30 or PA31), without a CRC-32 prefix.
    Delta(Vec<u8>),
}

/// A temporary file that is removed on drop.
struct Spool {
    path: std::path::PathBuf,
}

impl Spool {
    /// Create an empty temporary file.
    fn create() -> io::Result<Self> {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let number = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("apiset-fetch-{}-{number}.cab", std::process::id()));
        std::fs::File::create(&path)?;
        Ok(Self { path })
    }

    /// Open the file for reading and writing.
    fn file(&self) -> io::Result<std::fs::File> {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.path)
    }
}

impl Drop for Spool {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

impl ReadAt for HttpFile {
    fn size(&self) -> io::Result<u64> {
        Ok(self.len())
    }

    fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        HttpFile::read_exact_at(self, offset, buf)
    }
}

/// Return the payload path of a component that `name` gives, with its kind: `Some(true)` for
/// a forward delta, `Some(false)` for a full file, and `None` for another file.
fn classify(name: &str, component: &str) -> Option<bool> {
    let lower = name.to_ascii_lowercase();
    let rest = lower.strip_prefix(&component.to_ascii_lowercase())?;
    let rest = rest.strip_prefix('\\')?;
    if rest == FILE_NAME {
        Some(false)
    } else if rest.strip_prefix("f\\") == Some(FILE_NAME) {
        Some(true)
    } else {
        None
    }
}

/// Turn the bytes of a payload into a [`Payload`].
fn payload(bytes: Vec<u8>, delta: bool) -> Result<Payload> {
    if !delta {
        return Ok(Payload::Full(bytes));
    }
    // A forward delta has a CRC-32 prefix in PSF files and in recent cabinets.
    if bytes.starts_with(b"PA30") || bytes.starts_with(b"PA31") {
        return Ok(Payload::Delta(bytes));
    }
    Ok(Payload::Delta(winupdate::psf::strip_crc(&bytes)?.to_vec()))
}

/// Read the payloads of `components` from the package at `url`.
///
/// The result maps each component that the package has to its payload. A component that
/// the package does not have is not in the result.
///
/// # Errors
///
/// Returns an error if the package cannot be read or has an unknown form.
pub fn read_payloads(
    agent: &ureq::Agent,
    url: &str,
    components: &[String],
) -> Result<BTreeMap<String, Payload>> {
    let file = HttpFile::open(agent, url)?;
    let mut magic = [0u8; 4];
    file.read_exact_at(0, &mut magic)?;
    match &magic {
        b"MSWI" => from_wim(&file, components),
        b"MSCF" => from_cabinet(file, components),
        _ => Err(format!("{url}: unknown package form {magic:02x?}").into()),
    }
}

/// Return the PSF entries of the components.
fn index_entries<'a>(
    index: &'a PsfIndex,
    components: &[String],
) -> Vec<(String, &'a PsfFile, bool)> {
    let mut found = Vec::new();
    for component in components {
        // Prefer a full file to a delta.
        let mut best: Option<(&PsfFile, bool)> = None;
        for file in &index.files {
            if let Some(delta) = classify(&file.name, component)
                && best.is_none_or(|(_, best_delta)| best_delta && !delta)
            {
                best = Some((file, delta));
            }
        }
        if let Some((file, delta)) = best {
            found.push((component.clone(), file, delta));
        }
    }
    found
}

/// Read the bytes of a PSF entry with a random-access PSF reader.
fn read_entry<R: ReadAt + ?Sized>(psf: &R, file: &PsfFile) -> Result<Vec<u8>> {
    let source = file
        .sources
        .first()
        .ok_or_else(|| format!("{}: no source in the PSF index", file.name))?;
    let bytes = winupdate::psf::read_source(psf, source)?;
    match &source.kind {
        SourceKind::Raw => Ok(bytes),
        // An intra-package PA30 delta has no source.
        SourceKind::Pa30 => Ok(msdelta::apply(&[], &bytes)?),
        SourceKind::Other(kind) => Err(format!("{}: PSF source type {kind}", file.name).into()),
    }
}

/// Read payloads from an MSU that is a WIM file.
fn from_wim(file: &HttpFile, components: &[String]) -> Result<BTreeMap<String, Payload>> {
    let msu = Wim::open(file)?;
    let image = msu.image(1)?;
    let find = |extension: &str| {
        image
            .files()
            .iter()
            .find(|f| {
                let name = f.path.to_ascii_lowercase();
                name.contains("-kb") && name.ends_with(extension) && !name.contains('\\')
            })
            .ok_or_else(|| format!("the MSU has no package {extension} file"))
    };
    let package = Wim::open(msu.open_file(find(".wim")?)?)?;
    let package_image = package.image(1)?;
    let index_file = package_image
        .find(INDEX_NAME)
        .ok_or("the package has no PSF index")?;
    let index = PsfIndex::parse(&String::from_utf8(package.read(index_file)?)?)?;
    let psf = msu.open_file(find(".psf")?)?;

    let mut payloads = BTreeMap::new();
    for (component, entry, delta) in index_entries(&index, components) {
        payloads.insert(component, payload(read_entry(&psf, entry)?, delta)?);
    }
    Ok(payloads)
}

/// Return `true` if a cabinet entry is the package cabinet of an update.
fn is_package_cabinet(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    std::path::Path::new(&name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("cab"))
        && name.contains("-kb")
        && !name.starts_with("ssu-")
        && !name.contains("wsusscan")
        && !name.contains("desktopdeployment")
        && !name.contains("aggregatedmetadata")
}

/// Read payloads from an MSU or a CAB that is a cabinet file.
fn from_cabinet(file: HttpFile, components: &[String]) -> Result<BTreeMap<String, Payload>> {
    let mut outer = Cab::open(file)?;
    let inner_name = outer
        .entries()
        .iter()
        .find(|entry| is_package_cabinet(&entry.name))
        .map(|entry| entry.name.clone());
    let psf_name = outer
        .entries()
        .iter()
        .find(|entry| entry.name.to_ascii_lowercase().ends_with(".psf"))
        .map(|entry| entry.name.clone());

    // A package cabinet without an inner cabinet holds the payloads itself.
    let Some(inner_name) = inner_name else {
        return payloads_in_cabinet(&mut outer, components);
    };
    // An inner cabinet can be larger than 700 MB: keep it in a temporary file, not in memory.
    let spool = Spool::create()?;
    io::copy(&mut outer.open_file(&inner_name)?, &mut spool.file()?)?;
    let mut inner = Cab::open(spool.file()?)?;
    let Some(psf_name) = psf_name.filter(|_| inner.find(INDEX_NAME).is_some()) else {
        return payloads_in_cabinet(&mut inner, components);
    };

    let index = PsfIndex::parse(&String::from_utf8(inner.read(INDEX_NAME)?)?)?;
    let mut wanted = index_entries(&index, components);
    // The PSF file is in a compressed cabinet folder: read it once, in offset order.
    wanted.sort_by_key(|(_, entry, _)| entry.sources.first().map_or(0, |s| s.offset));
    let mut stream = outer.open_file(&psf_name)?;
    let mut position = 0u64;
    let mut payloads = BTreeMap::new();
    for (component, entry, delta) in wanted {
        let source = entry
            .sources
            .first()
            .ok_or_else(|| format!("{}: no source in the PSF index", entry.name))?;
        let skip = source
            .offset
            .checked_sub(position)
            .ok_or_else(|| format!("{}: overlapping PSF sources", entry.name))?;
        io::copy(&mut (&mut stream).take(skip), &mut io::sink())?;
        let mut bytes = vec![0; usize::try_from(source.length)?];
        stream.read_exact(&mut bytes)?;
        position = source.offset + source.length;
        // `read_source` checks the hash; use it on the bytes that the stream gave.
        let window = winupdate::Slice::new(bytes.as_slice(), 0, source.length);
        let shifted = winupdate::cix::Source {
            offset: 0,
            ..source.clone()
        };
        let mut checked = winupdate::psf::read_source(&window, &shifted)?;
        if source.kind == SourceKind::Pa30 {
            checked = msdelta::apply(&[], &checked)?;
        }
        payloads.insert(component, payload(checked, delta)?);
    }
    Ok(payloads)
}

/// Read payloads that a cabinet holds as files.
fn payloads_in_cabinet<R: ReadAt>(
    cabinet: &mut Cab<R>,
    components: &[String],
) -> Result<BTreeMap<String, Payload>> {
    let mut payloads = BTreeMap::new();
    for component in components {
        let found = cabinet
            .entries()
            .iter()
            .filter_map(|entry| classify(&entry.name, component).map(|d| (entry.name.clone(), d)))
            .min_by_key(|(_, delta)| *delta);
        if let Some((name, delta)) = found {
            let bytes = cabinet.read(&name)?;
            payloads.insert(component.clone(), payload(bytes, delta)?);
        }
    }
    Ok(payloads)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_paths_match_the_component_directory() {
        let component = "amd64_microsoft-windows-apisetschema-wnc_31bf3856ad364e35_10.0.26100.9549_none_6d8461b9bafd9b1b";
        let delta = format!("{component}\\f\\apisetschema.dll");
        assert_eq!(classify(&delta, component), Some(true));
        assert_eq!(
            classify(&format!("{component}\\APISETSCHEMA.DLL"), component),
            Some(false)
        );
        assert_eq!(
            classify(&format!("{component}\\r\\apisetschema.dll"), component),
            None
        );
        assert_eq!(
            classify(&format!("{component}x\\apisetschema.dll"), component),
            None
        );
        assert_eq!(
            classify(&format!("{component}\\f\\other.dll"), component),
            None
        );
    }

    #[test]
    fn forward_deltas_lose_their_crc_prefix() {
        let delta = b"PA31rest".to_vec();
        let mut wrapped = crc_prefix(&delta);
        wrapped.extend_from_slice(&delta);
        assert_eq!(
            payload(wrapped, true).unwrap(),
            Payload::Delta(delta.clone())
        );
        assert_eq!(
            payload(delta.clone(), true).unwrap(),
            Payload::Delta(delta.clone())
        );
        assert_eq!(payload(delta.clone(), false).unwrap(), Payload::Full(delta));
    }

    fn crc_prefix(data: &[u8]) -> Vec<u8> {
        // CRC-32 (IEEE), bitwise.
        let mut crc = !0u32;
        for &byte in data {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        (!crc).to_le_bytes().to_vec()
    }
}

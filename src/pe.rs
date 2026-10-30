//! Read the API set schema from `apisetschema.dll` with `portex`.
//!
//! Windows keeps the schema in the `.apiset` section of
//! `C:\Windows\System32\apisetschema.dll`. The functions in this module find the section and
//! parse its data.
//!
//! # Example
//!
//! ```no_run
//! use apiset_rs::ApiSetSchema;
//!
//! # #[cfg(feature = "std")]
//! # {
//! let schema = ApiSetSchema::from_pe_file(r"C:\Windows\System32\apisetschema.dll")?;
//! println!("{} API sets", schema.entries().len());
//! # }
//! # Ok::<(), apiset_rs::Error>(())
//! ```

use crate::{ApiSetSchema, Error, ErrorKind, Result};
use portex::PeImage;

/// The name of the section that holds the schema.
pub const APISET_SECTION: &str = ".apiset";

impl ApiSetSchema {
    /// Parse the schema in the `.apiset` section of a parsed PE image.
    ///
    /// # Errors
    ///
    /// Returns a `MissingApiSetSection` error if the image has no `.apiset` section. Returns
    /// an error if the schema in the section is not valid.
    pub fn from_pe_image(image: &PeImage) -> Result<Self> {
        let section = image
            .section_by_name(APISET_SECTION)
            .ok_or_else(|| Error::new(ErrorKind::MissingApiSetSection))?;
        Self::parse(&section.data)
    }

    /// Parse the schema in the `.apiset` section of a raw PE file, for example
    /// `apisetschema.dll`.
    ///
    /// # Errors
    ///
    /// Returns a `Pe` error if the file does not parse. Returns the errors of
    /// [`Self::from_pe_image`].
    pub fn from_pe(file: &[u8]) -> Result<Self> {
        Self::from_pe_image(&PeImage::parse(file)?)
    }

    /// Read a raw PE file from the file system, and parse the schema in its `.apiset` section.
    ///
    /// # Errors
    ///
    /// Returns a `Pe` error if the file cannot be read or does not parse. Returns the errors
    /// of [`Self::from_pe_image`].
    #[cfg(feature = "std")]
    pub fn from_pe_file(path: impl AsRef<std::path::Path>) -> Result<Self> {
        Self::from_pe_image(&PeImage::from_file(path)?)
    }
}

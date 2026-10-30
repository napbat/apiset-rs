//! Error types for API set schema parsing and building.
//!
//! An error can have a context: an offset in the schema, a structure name and details. Use the
//! context to find the cause of a problem when you parse or build a schema.
//!
//! # Example
//!
//! ```
//! use apiset_rs::ApiSetSchema;
//!
//! match ApiSetSchema::parse(&[6, 0, 0, 0]) {
//!     Ok(_schema) => println!("Loaded schema"),
//!     Err(e) => {
//!         eprintln!("Error: {e}");
//!         if let Some(ctx) = e.context() {
//!             eprintln!("  Context: {ctx}");
//!         }
//!     }
//! }
//! ```

use crate::prelude::*;
use core::fmt;

/// Result type alias for apiset operations.
pub type Result<T, E = Error> = core::result::Result<T, E>;

/// Information about the location of an error.
#[derive(Debug, Clone, Default)]
pub struct ErrorContext {
    /// Offset from the start of the schema where the error occurred (if known).
    pub offset: Option<u64>,
    /// Name of the structure that the parser was reading (if known).
    pub structure: Option<String>,
    /// Additional details about the error.
    pub details: Option<String>,
}

impl ErrorContext {
    /// Create an empty context.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the offset.
    #[must_use]
    pub fn at_offset(mut self, offset: u64) -> Self {
        self.offset = Some(offset);
        self
    }

    /// Set the structure name.
    #[must_use]
    pub fn in_structure(mut self, name: impl Into<String>) -> Self {
        self.structure = Some(name.into());
        self
    }

    /// Set additional details.
    #[must_use]
    pub fn with_details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
    }

    /// Return `true` if the context has no information.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.offset.is_none() && self.structure.is_none() && self.details.is_none()
    }
}

impl fmt::Display for ErrorContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = Vec::new();
        if let Some(offset) = self.offset {
            parts.push(format!("offset {offset:#x}"));
        }
        if let Some(structure) = &self.structure {
            parts.push(format!("in {structure}"));
        }
        if let Some(details) = &self.details {
            parts.push(details.clone());
        }
        write!(f, "{}", parts.join(", "))
    }
}

/// An error from parsing or building an API set schema.
#[derive(Debug)]
pub struct Error {
    /// The error kind.
    pub kind: ErrorKind,
    /// Optional context about where the error occurred.
    pub context: Option<ErrorContext>,
}

/// The kind of error that occurred.
#[derive(Debug)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The buffer is too small for the expected data.
    BufferTooSmall {
        /// Number of bytes required.
        expected: usize,
        /// Number of bytes available.
        actual: usize,
    },
    /// A range is not inside the schema.
    OffsetOutOfBounds {
        /// The end of the range that is out of bounds.
        offset: u64,
        /// Size of the region that the check used.
        size: usize,
    },
    /// The schema version is not 2, 3, 4 or 6.
    UnsupportedVersion(u32),
    /// A string is not valid UTF-16, or a UTF-16 string has an odd byte length.
    InvalidUtf16,
    /// The schema structure is not valid.
    InvalidSchema(String),
    /// The input of the builder is not valid.
    InvalidInput(String),
    /// The PE image has no `.apiset` section.
    #[cfg(feature = "pe")]
    MissingApiSetSection,
    /// The PE image does not parse. The error is boxed, because `portex::Error` is large.
    #[cfg(feature = "pe")]
    Pe(Box<portex::Error>),
}

impl Error {
    /// Create a new error with the given kind.
    #[must_use]
    pub fn new(kind: ErrorKind) -> Self {
        Self {
            kind,
            context: None,
        }
    }

    /// Add context to this error.
    #[must_use]
    pub fn with_context(mut self, context: ErrorContext) -> Self {
        self.context = Some(context);
        self
    }

    /// Add offset context.
    #[must_use]
    pub fn at_offset(mut self, offset: u64) -> Self {
        let ctx = self.context.take().unwrap_or_default();
        self.context = Some(ctx.at_offset(offset));
        self
    }

    /// Add structure name context.
    #[must_use]
    pub fn in_structure(mut self, name: impl Into<String>) -> Self {
        let ctx = self.context.take().unwrap_or_default();
        self.context = Some(ctx.in_structure(name));
        self
    }

    /// Get the context if present.
    #[must_use]
    pub fn context(&self) -> Option<&ErrorContext> {
        self.context.as_ref()
    }

    /// Get the error kind.
    #[must_use]
    pub fn kind(&self) -> &ErrorKind {
        &self.kind
    }

    /// Create a `BufferTooSmall` error.
    #[must_use]
    pub fn buffer_too_small(expected: usize, actual: usize) -> Self {
        Self::new(ErrorKind::BufferTooSmall { expected, actual })
    }

    /// Create an `OffsetOutOfBounds` error.
    #[must_use]
    pub fn offset_out_of_bounds(offset: u64, size: usize) -> Self {
        Self::new(ErrorKind::OffsetOutOfBounds { offset, size })
    }

    /// Create an `UnsupportedVersion` error.
    #[must_use]
    pub fn unsupported_version(version: u32) -> Self {
        Self::new(ErrorKind::UnsupportedVersion(version))
    }

    /// Create an `InvalidUtf16` error.
    #[must_use]
    pub fn invalid_utf16() -> Self {
        Self::new(ErrorKind::InvalidUtf16)
    }

    /// Create an `InvalidSchema` error.
    pub fn invalid_schema(msg: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidSchema(msg.into()))
    }

    /// Create an `InvalidInput` error.
    pub fn invalid_input(msg: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidInput(msg.into()))
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErrorKind::BufferTooSmall { expected, actual } => {
                write!(
                    f,
                    "Buffer too small: expected {expected} bytes, got {actual}"
                )
            }
            ErrorKind::OffsetOutOfBounds { offset, size } => {
                write!(f, "Offset {offset:#x} out of bounds (size: {size})")
            }
            ErrorKind::UnsupportedVersion(v) => write!(f, "Unsupported schema version: {v}"),
            ErrorKind::InvalidUtf16 => write!(f, "Invalid UTF-16 string"),
            ErrorKind::InvalidSchema(msg) => write!(f, "Invalid schema: {msg}"),
            ErrorKind::InvalidInput(msg) => write!(f, "Invalid builder input: {msg}"),
            #[cfg(feature = "pe")]
            ErrorKind::MissingApiSetSection => write!(f, "The PE image has no .apiset section"),
            #[cfg(feature = "pe")]
            ErrorKind::Pe(e) => write!(f, "PE error: {e}"),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.kind)?;
        if let Some(ctx) = &self.context
            && !ctx.is_empty()
        {
            write!(f, " ({ctx})")?;
        }
        Ok(())
    }
}

impl core::error::Error for Error {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match &self.kind {
            #[cfg(feature = "pe")]
            ErrorKind::Pe(e) => Some(&**e),
            _ => None,
        }
    }
}

#[cfg(feature = "pe")]
impl From<portex::Error> for Error {
    fn from(e: portex::Error) -> Self {
        Self::new(ErrorKind::Pe(Box::new(e)))
    }
}

impl From<ErrorKind> for Error {
    fn from(kind: ErrorKind) -> Self {
        Self::new(kind)
    }
}

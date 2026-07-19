//! Error and diagnostic types.
//!
//! Two families exist, reflecting the two phases:
//!
//! * [`SchemaError`] — fatal problems with the *schemas themselves* (bad YAML, a
//!   schema that will not compile, an unresolvable `$ref`/`x-ref`). These are
//!   surfaced when building a [`crate::DataStore`] and, in richer form, by the
//!   build-time [`crate::validate_schemas`] check.
//! * [`DataError`] — non-fatal problems found while parsing *data files* against
//!   a schema. Parsing never aborts on the first problem: a complete list is
//!   collected so a caller (or the UI) can report everything at once.

use std::fmt;
use std::path::PathBuf;

use uuid::Uuid;

/// A fatal problem with a schema.
#[derive(Debug, Clone, thiserror::Error)]
pub enum SchemaError {
    /// The schema text was not valid YAML/JSON.
    #[error("schema '{id}' is not valid YAML/JSON: {message}")]
    Parse { id: String, message: String },
    /// The schema did not compile as a JSON Schema (structural error / bad `$ref`).
    #[error("schema '{id}' failed to compile: {message}")]
    Compile { id: String, message: String },
    /// An `x-ref` names a schema id that was not registered.
    #[error("schema '{id}' has an x-ref to unknown schema '{target}' (at {pointer})")]
    UnknownRefTarget {
        id: String,
        target: String,
        pointer: String,
    },
    /// A data file referenced a schema id that was not registered.
    #[error("unknown schema id '{0}'")]
    UnknownSchema(String),
}

/// A failure while creating, cloning, or placing a new document via a factory.
#[derive(Debug, Clone, thiserror::Error)]
pub enum FactoryError {
    /// The schema id is not registered.
    #[error("unknown schema id '{0}'")]
    UnknownSchema(String),
    /// A per-file document was requested but no base data directory is set.
    #[error("no data directory configured for new files (call set_data_dir)")]
    NoDataDir,
    /// The schema's root object has no identity, so it cannot be stored one-per-file.
    #[error("schema '{0}' has no root id; it is not stored as one file per instance")]
    NotPerFile(String),
    /// The requested source document is not loaded in this store.
    #[error("no loaded document has source '{0}'")]
    UnknownSource(String),
}

/// The category of a non-fatal [`DataError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataErrorKind {
    /// The file could not be parsed as YAML.
    Yaml,
    /// The document failed JSON Schema validation.
    Validation,
    /// A unit-bearing value could not be decoded (e.g. `"10 furlongs"`).
    Unit,
    /// An identifier value was not a well-formed UUID.
    Id,
    /// A reference value was not a well-formed UUID.
    Reference,
    /// A required property was absent (and had no default).
    MissingRequired,
    /// A reference did not resolve to any known object (set during `resolve`).
    UnresolvedRef,
    /// The file's `schema_version` is missing or incompatible with the schema.
    SchemaVersion,
}

/// A single, non-fatal problem found while parsing or resolving a data file.
#[derive(Debug, Clone, PartialEq)]
pub struct DataError {
    /// Root schema the file was parsed against.
    pub schema_id: String,
    /// Originating file, when the caller supplied one.
    pub source: Option<PathBuf>,
    /// JSON Pointer to the offending location within the document.
    pub pointer: String,
    /// What kind of problem this is.
    pub kind: DataErrorKind,
    /// Human-readable detail.
    pub message: String,
}

impl DataError {
    pub(crate) fn new(
        schema_id: &str,
        source: &Option<PathBuf>,
        pointer: impl Into<String>,
        kind: DataErrorKind,
        message: impl Into<String>,
    ) -> Self {
        Self {
            schema_id: schema_id.to_string(),
            source: source.clone(),
            pointer: pointer.into(),
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for DataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let at = if self.pointer.is_empty() { "/" } else { &self.pointer };
        write!(f, "[{}] {at}: {}", self.schema_id, self.message)
    }
}

/// A reason a node (or document) is flagged incomplete after resolution.
#[derive(Debug, Clone, PartialEq)]
pub enum Reason {
    /// A required property was absent.
    MissingRequired(String),
    /// A reference could not be resolved to a known object.
    UnresolvedRef(Uuid),
}

/// One place that references an object slated for removal — enough for a UI to
/// point the user at the item that blocks the delete.
#[derive(Debug, Clone, PartialEq)]
pub struct Referrer {
    /// Root identity of the referring document, if it has one.
    pub document_id: Option<Uuid>,
    /// Source file of the referring document.
    pub source: Option<PathBuf>,
    /// JSON Pointer to the reference field within that document.
    pub pointer: String,
    /// The specific id (inside the document being removed) that is referenced.
    pub target_id: Uuid,
}

/// A failure to remove a document.
#[derive(Debug, Clone, thiserror::Error)]
pub enum RemoveError {
    /// No loaded document has that root identity.
    #[error("no document with id '{0}'")]
    NotFound(Uuid),
    /// The document (or an object within it) is referenced elsewhere. The
    /// `referrers` name every dependant so the UI can explain the blockage.
    #[error("document '{id}' is referenced by {} other item(s); cannot be removed", referrers.len())]
    InUse { id: Uuid, referrers: Vec<Referrer> },
}

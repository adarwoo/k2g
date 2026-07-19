//! A generic, schema-driven data store.
//!
//! `datastore` loads JSON Schema documents (authored in YAML), validates data
//! files against them, applies schema defaults, decodes unit- and id-bearing
//! fields into typed values, and resolves cross-object references — producing a
//! **metadata-annotated document tree** that a UI or an application context can
//! drive directly. It embeds nothing application-specific: the caller supplies
//! the schemas.
//!
//! # Pipeline
//!
//! 1. **Initialize** — [`DataStore::builder`] takes `(id, yaml_text)` schema
//!    pairs (typically `include_str!`'d) and compiles a validator per schema,
//!    wiring cross-file `$ref`s. It is *runtime-trusting*: it does not re-verify
//!    that each schema is itself valid JSON Schema — that is a build-time check
//!    (see [`validate_schemas`]).
//! 2. **Parse** — [`DataStore::parse`] validates each file (collecting *all*
//!    problems, non-fatally), applies `default`/`const`, decodes `units.yaml`
//!    fields to [`units`] types and `id.yaml` fields to UUIDs, and records
//!    `x-ref` fields as still-unresolved [`Reference`]s. Every node carries
//!    schema [`Meta`] (title, description, required, kind, constraints).
//! 3. **Resolve** — [`DataStore::resolve`] links every reference to the object
//!    that owns the matching UUID, returning a [`ResolvedStore`] of
//!    [`Handle`]s. Unresolved references and missing required fields mark their
//!    node (and document) [`Status::Incomplete`].
//!
//! # Schema conventions
//!
//! | Convention | Meaning |
//! |------------|---------|
//! | `$ref: "units.yaml#/$defs/<name>"` | a unit-bearing value (see [`UnitKind`]) |
//! | `$ref: "id.yaml#/$defs/uuid_v7"`   | this object's own identity |
//! | `x-ref: "<schema-id>"` (sibling)   | a reference to another object by UUID |
//!
//! `x-` keywords are ignored by JSON Schema validation, so they annotate without
//! affecting validity.
//!
//! # Persistence (write-back)
//!
//! Editing a document through [`ResolvedStore::edit`] or
//! [`ResolvedStore::set_value`] transparently schedules a write of that document
//! back to its own source file — the change *is* the save; no separate call is
//! needed. Writes run on a lazily-started background thread through a queue that
//! **coalesces per file** (a burst of edits to one file collapses to a single
//! write of the latest data). The document is snapshotted to bytes on the
//! calling thread, so the writer never shares the live tree. Each write is
//! atomic (temp file + `rename`) and portable. [`ResolvedStore::flush`] waits for
//! the queue to drain; dropping the store also drains it.
//!
//! Only **per-file** atomicity is guaranteed. A *cross-file* transaction — commit
//! several files all-or-nothing — is intentionally not provided: no OS offers an
//! atomic multi-file rename, so it would require a write-ahead journal (record
//! intent, fsync, apply, then clear, with idempotent crash-recovery replay on
//! startup). That is a substantial subsystem; for independent files such as
//! `settings.yaml` or `stock.yaml` the per-file guarantee is sufficient, and
//! [`ResolvedStore::flush`] narrows the window where related files disagree.
//!
//! # Creating, cloning, exporting
//!
//! New instances come from the schema, not hand-built maps:
//!
//! * [`DataStore::instantiate`] builds a fresh, unattached node — defaults
//!   applied, identity fields assigned new UUIDv7s, required-without-default
//!   fields left absent (so it is [`Status::Incomplete`] until filled).
//! * [`ResolvedStore::create_document`] does that *and* stores it: a schema
//!   whose root has an identity is written one file per instance, at
//!   `<data_dir>/<schema-stem>/<uuid>.yaml` (set the base with
//!   [`ResolvedStore::set_data_dir`]).
//! * [`ResolvedStore::add_item`] appends a fresh item to an in-document array
//!   (e.g. adding a tool to `stock.yaml`), persisting that file.
//! * [`ResolvedStore::clone_item`] / [`ResolvedStore::clone_document`] deep-copy
//!   a structure, assigning new ids and remapping any references that pointed
//!   inside the copy.
//! * [`ResolvedStore::export`] writes a loaded document to an arbitrary path,
//!   reusing the same background writer.
//! * [`ResolvedStore::remove_document`] drops a document and deletes its file —
//!   but refuses (with a [`RemoveError::InUse`] listing every [`Referrer`]) when
//!   something still references it, so the UI can explain the blockage.
//!
//! # Schema reference & versioning
//!
//! Persisted files carry a reserved top-level `$schema` meta-key (the schema id)
//! plus `schema_version`. The crate strips `$schema` before JSON-Schema
//! validation and re-stamps it on write, so schemas need not declare it. On
//! parse, if a schema declares `x-schema-version`, the file's `schema_version`
//! is checked against it: a newer file is rejected, and (for now) an older file
//! is rejected too — an in-place upgrade path is planned. Both are reported as
//! [`DataErrorKind::SchemaVersion`].
//!
//! # Example
//!
//! ```
//! use datastore::{DataStore, NodeValue, ParseInput, UnitValue};
//!
//! let thing = r#"
//! $schema: "https://json-schema.org/draft/2020-12/schema"
//! $id: "thing.yaml"
//! type: object
//! required: [id]
//! properties:
//!   id: { $ref: "id.yaml#/$defs/uuid_v7" }
//!   width: { $ref: "units.yaml#/$defs/size" }
//!   note: { type: string, default: "n/a" }
//! "#;
//! // Minimal stand-ins for the shared schemas.
//! let ids = r#"{ "$id": "id.yaml", "$defs": { "uuid_v7": { "type": "string" } } }"#;
//! let units = r#"{ "$id": "units.yaml", "$defs": { "size": { "type": "string" } } }"#;
//!
//! let store = DataStore::builder()
//!     .schema("id.yaml", ids)
//!     .schema("units.yaml", units)
//!     .schema("thing.yaml", thing)
//!     .build()
//!     .unwrap();
//!
//! let file = r#"
//! id: "0194fd2c-5f2e-7a9d-9a76-3b2b9fbc3f11"
//! width: "10mm"
//! "#;
//! let out = store.parse(&[ParseInput { schema_id: "thing.yaml", source: None, text: file }]);
//! assert!(out.errors.is_empty());
//!
//! let doc = &out.documents[0];
//! // The unit field decoded to a typed Length.
//! let width = doc.root.get_pointer("/width").unwrap();
//! assert!(matches!(width.value, NodeValue::Unit(UnitValue::Length(_))));
//! // The absent `note` picked up its schema default, flagged as such.
//! let note = doc.root.get_pointer("/note").unwrap();
//! assert!(note.meta.default_applied);
//! assert!(matches!(&note.value, NodeValue::Str(s) if s == "n/a"));
//! ```

mod error;
mod model;
mod parse;
mod persist;
mod resolve;
mod schema;
mod units_bridge;

pub use error::{
    DataError, DataErrorKind, FactoryError, Reason, Referrer, RemoveError, SchemaError,
};
pub use model::{
    Constraints, Document, FieldKind, Handle, Meta, Node, NodeValue, Reference, RefState, SchemaId,
    Status, UnitValue,
};
pub use persist::WriteError;
pub use resolve::ResolvedStore;
pub use units_bridge::UnitKind;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use schema::SchemaSet;

/// A compiled set of schemas, ready to parse and resolve data files.
pub struct DataStore {
    schemas: Arc<SchemaSet>,
}

/// Incremental builder for a [`DataStore`].
pub struct DataStoreBuilder {
    sources: Vec<(String, String)>,
}

/// One data file to parse, tagged with the schema id it should validate against.
pub struct ParseInput<'a> {
    /// The root schema id (e.g. `"cnc.yaml"`).
    pub schema_id: &'a str,
    /// Originating file, echoed into any [`DataError`]s.
    pub source: Option<PathBuf>,
    /// The file's raw YAML/JSON text.
    pub text: &'a str,
}

/// The result of parsing a batch of files: the annotated documents plus every
/// non-fatal problem found across all of them.
pub struct ParseOutcome {
    pub documents: Vec<Document>,
    pub errors: Vec<DataError>,
}

impl DataStore {
    /// Starts building a store.
    pub fn builder() -> DataStoreBuilder {
        DataStoreBuilder {
            sources: Vec::new(),
        }
    }

    /// Parses a batch of data files. Never aborts on the first problem: it
    /// returns every document it could build and a complete error list.
    pub fn parse(&self, inputs: &[ParseInput<'_>]) -> ParseOutcome {
        let mut documents = Vec::new();
        let mut errors = Vec::new();
        for input in inputs {
            if let Some(document) = parse::parse_document(
                &self.schemas,
                input.schema_id,
                input.source.clone(),
                input.text,
                &mut errors,
            ) {
                documents.push(document);
            }
        }
        ParseOutcome { documents, errors }
    }

    /// Resolves references across the given documents into a [`ResolvedStore`].
    pub fn resolve(&self, documents: Vec<Document>) -> ResolvedStore {
        ResolvedStore::build(documents, Arc::clone(&self.schemas))
    }

    /// Opens an empty live store — the usual entry point when documents will be
    /// loaded from directories via [`ResolvedStore::parse_directory`] rather than
    /// parsed up front. Equivalent to `resolve(vec![])`.
    pub fn open(&self) -> ResolvedStore {
        ResolvedStore::build(Vec::new(), Arc::clone(&self.schemas))
    }

    /// Builds a fresh, unattached instance of `schema_id`: schema defaults are
    /// applied and identity fields get new UUIDv7s. Required fields without a
    /// default are left absent, so the node reports [`Status::Incomplete`] until
    /// filled. Returns `None` for an unknown schema.
    ///
    /// To create *and* store a new per-file document, use
    /// [`ResolvedStore::create_document`] instead.
    pub fn instantiate(&self, schema_id: &str) -> Option<Node> {
        parse::instantiate(&self.schemas, schema_id)
    }

    /// Writes every embedded schema to `dir` under its `$id` filename, creating
    /// `dir` if needed (e.g. to seed a user's schema directory).
    pub fn export_schemas(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        for (id, text) in self.schemas.iter_texts() {
            std::fs::write(dir.join(id), text)?;
        }
        Ok(())
    }
}

impl DataStoreBuilder {
    /// Registers one schema by id (its `$id` is used if present, else this id).
    pub fn schema(mut self, id: &str, text: &str) -> Self {
        self.sources.push((id.to_string(), text.to_string()));
        self
    }

    /// Compiles the registered schemas into a [`DataStore`].
    pub fn build(self) -> Result<DataStore, SchemaError> {
        Ok(DataStore {
            schemas: Arc::new(SchemaSet::from_sources(&self.sources)?),
        })
    }
}

/// Build-time schema validation: parses and compiles every schema and checks
/// that each `x-ref` targets a registered schema, returning *all* problems.
///
/// Intended to run from a build script, an `xtask`, or a test so that
/// [`DataStore::builder`] can trust the schemas at runtime.
pub fn validate_schemas(sources: &[(&str, &str)]) -> Result<(), Vec<SchemaError>> {
    let owned: Vec<(String, String)> = sources
        .iter()
        .map(|(id, text)| (id.to_string(), text.to_string()))
        .collect();
    SchemaSet::validate(&owned)
}

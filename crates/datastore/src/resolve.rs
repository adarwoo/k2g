//! Reference resolution.
//!
//! Once a batch of documents is parsed, [`ResolvedStore::build`] does two passes:
//!
//! 1. **Index** — every object that carries an identity ([`NodeValue::Id`]) is
//!    registered in a [`Handle`]-addressed table keyed by its UUID. UUIDv7 is
//!    globally unique, so one flat registry spans all documents.
//! 2. **Link** — every [`NodeValue::Ref`] is looked up in that table; a hit
//!    becomes [`RefState::Resolved`], a miss stays [`RefState::Unresolved`] and
//!    flags its node (and the enclosing document) [`Status::Incomplete`].
//!
//! Resolved references are handles, not Rust references, so there is no lifetime
//! or ownership entanglement: follow one with [`ResolvedStore::get`].

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;
use uuid::Uuid;

use crate::error::{DataError, DataErrorKind, FactoryError, Reason, Referrer, RemoveError};
use crate::model::{Document, Handle, Node, NodeValue, RefState, Status};
use crate::parse::aggregate_status;
use crate::persist::{WriteError, Writer};
use crate::schema::SchemaSet;

/// Where a referenced object lives within the resolved documents.
struct Location {
    doc: usize,
    pointer: String,
}

/// A batch of parsed documents with references resolved to [`Handle`]s.
///
/// Beyond read access, the store supports **in-place editing that auto-persists**:
/// mutating a document through [`ResolvedStore::edit`] (or
/// [`ResolvedStore::set_value`]) snapshots it and schedules a background write
/// back to its source file. No separate save call is needed. A background writer
/// thread is started lazily on the first edit; read-only use never spawns one.
pub struct ResolvedStore {
    documents: Vec<Document>,
    locations: Vec<Location>,
    by_uuid: HashMap<Uuid, Handle>,
    writer: Option<Writer>,
    schemas: Arc<SchemaSet>,
    data_dir: Option<PathBuf>,
    collections: HashMap<String, PathBuf>,
}

impl ResolvedStore {
    /// Builds a live store from parsed documents, indexing identities and
    /// resolving references.
    pub(crate) fn build(documents: Vec<Document>, schemas: Arc<SchemaSet>) -> Self {
        let mut store = Self {
            documents,
            locations: Vec::new(),
            by_uuid: HashMap::new(),
            writer: None,
            schemas,
            data_dir: None,
            collections: HashMap::new(),
        };
        store.resolve_all();
        store
    }

    /// The resolved documents.
    pub fn documents(&self) -> &[Document] {
        &self.documents
    }

    /// The object a [`Handle`] addresses.
    pub fn get(&self, handle: Handle) -> Option<&Node> {
        let location = self.locations.get(handle.0)?;
        self.documents
            .get(location.doc)?
            .root
            .get_pointer(&location.pointer)
    }

    /// The handle for an object with a given identity, if one was indexed.
    pub fn handle_for(&self, id: Uuid) -> Option<Handle> {
        self.by_uuid.get(&id).copied()
    }

    /// Edits the document loaded from `source` in place, then schedules a
    /// background write of the result back to that same file.
    ///
    /// This is the whole persistence contract: change the data, and the write
    /// is arranged for you — no separate save call. `f` receives the document by
    /// mutable reference; whatever it returns is passed back. Returns `None` if
    /// no loaded document has that source path.
    ///
    /// Note: structural changes that add or remove identified objects leave the
    /// reference registry stale; call [`Self::resolve_references`] afterwards if
    /// handles must reflect the new shape. Editing field *values* is safe.
    pub fn edit<R>(&mut self, source: &Path, f: impl FnOnce(&mut Document) -> R) -> Option<R> {
        let index = self
            .documents
            .iter()
            .position(|doc| doc.source.as_deref() == Some(source))?;
        let result = f(&mut self.documents[index]);
        self.schedule_write(index);
        Some(result)
    }

    /// Convenience edit: replaces the value at `pointer` (e.g. `/machine/max_feed_rate`)
    /// and schedules the write. Returns `Some(true)` if the field existed,
    /// `Some(false)` if the pointer did not resolve, `None` if `source` is unknown.
    pub fn set_value(
        &mut self,
        source: &Path,
        pointer: &str,
        value: crate::model::NodeValue,
    ) -> Option<bool> {
        self.edit(source, |doc| match doc.root.get_pointer_mut(pointer) {
            Some(node) => {
                node.value = value;
                true
            }
            None => false,
        })
    }

    /// Blocks until all scheduled writes have completed (e.g. before shutdown or
    /// in tests). A no-op if nothing has been edited.
    pub fn flush(&self) {
        if let Some(writer) = &self.writer {
            writer.flush();
        }
    }

    /// Drains and returns any errors from background writes so far.
    pub fn write_errors(&self) -> Vec<WriteError> {
        self.writer
            .as_ref()
            .map(Writer::take_errors)
            .unwrap_or_default()
    }

    /// Sets the base data directory. A schema's collection then lives at
    /// `<base>/<schema-stem>/`, used for new files and [`Self::parse_collection`].
    pub fn set_data_dir(&mut self, dir: impl Into<PathBuf>) {
        self.data_dir = Some(dir.into());
    }

    /// Associates a schema with an explicit directory, overriding the
    /// `<data_dir>/<stem>` convention for that schema's files.
    pub fn set_collection_dir(&mut self, schema_id: &str, dir: impl Into<PathBuf>) {
        self.collections.insert(schema_id.to_string(), dir.into());
    }

    /// Loads every `*.yaml` file in `dir` as an instance of `schema_id`, tagging
    /// each with its source path, registering `dir` as that schema's collection,
    /// and re-resolving references. Returns all non-fatal problems found.
    ///
    /// Afterwards the schema's identities are enumerable via
    /// [`Self::document_ids`], and new/cloned files of that schema land in `dir` —
    /// so the caller never has to name a path again.
    pub fn parse_directory(&mut self, schema_id: &str, dir: &Path) -> Vec<DataError> {
        self.collections.insert(schema_id.to_string(), dir.to_path_buf());
        let mut errors = Vec::new();

        match std::fs::read_dir(dir) {
            Ok(entries) => {
                let mut paths: Vec<PathBuf> = entries
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| is_yaml_file(path))
                    .collect();
                paths.sort();
                for path in paths {
                    match std::fs::read_to_string(&path) {
                        Ok(text) => {
                            if let Some(doc) = crate::parse::parse_document(
                                &self.schemas,
                                schema_id,
                                Some(path),
                                &text,
                                &mut errors,
                            ) {
                                self.documents.push(doc);
                            }
                        }
                        Err(error) => errors.push(DataError::new(
                            schema_id,
                            &Some(path),
                            "",
                            DataErrorKind::Yaml,
                            error.to_string(),
                        )),
                    }
                }
            }
            Err(error) => errors.push(DataError::new(
                schema_id,
                &None,
                "",
                DataErrorKind::Yaml,
                format!("cannot read directory '{}': {error}", dir.display()),
            )),
        }

        self.resolve_all();
        errors
    }

    /// Parses pre-read `(source, text)` items for `schema_id`, registering `dir`
    /// as that schema's collection (so new/cloned files of that schema land in
    /// `dir`) and re-resolving references. Like [`Self::parse_directory`], but for
    /// content the caller has already read from disk and possibly transformed —
    /// e.g. normalizing a legacy on-disk shape into schema form before load.
    /// Loaded documents are not marked dirty; they are written only on later edit.
    pub fn parse_texts(
        &mut self,
        schema_id: &str,
        dir: &Path,
        items: &[(PathBuf, String)],
    ) -> Vec<DataError> {
        self.collections.insert(schema_id.to_string(), dir.to_path_buf());
        let mut errors = Vec::new();
        for (path, text) in items {
            if let Some(doc) = crate::parse::parse_document(
                &self.schemas,
                schema_id,
                Some(path.clone()),
                text,
                &mut errors,
            ) {
                self.documents.push(doc);
            }
        }
        self.resolve_all();
        errors
    }

    /// Loads the collection for `schema_id` from its associated directory
    /// (explicitly registered, or `<data_dir>/<stem>`). Errors only if no
    /// directory is known.
    pub fn parse_collection(&mut self, schema_id: &str) -> Result<Vec<DataError>, FactoryError> {
        let dir = self.collection_dir(schema_id).ok_or(FactoryError::NoDataDir)?;
        Ok(self.parse_directory(schema_id, &dir))
    }

    /// The identities of all loaded documents of a given schema ("type").
    pub fn document_ids(&self, schema_id: &str) -> Vec<Uuid> {
        self.documents
            .iter()
            .filter(|doc| doc.schema_id == schema_id)
            .filter_map(|doc| doc.root.identity())
            .collect()
    }

    /// The loaded document whose root identity is `id`.
    pub fn document_by_id(&self, id: Uuid) -> Option<&Document> {
        self.documents.iter().find(|doc| doc.root.identity() == Some(id))
    }

    /// Re-indexes identities and re-resolves references across all documents.
    /// Call after structural edits made through [`Self::edit`]/[`Self::edit_by_id`].
    pub fn resolve_references(&mut self) {
        self.resolve_all();
    }

    /// Edits the document with root identity `id` in place, scheduling its write.
    /// The path-free counterpart to [`Self::edit`]. Returns `None` if unknown.
    pub fn edit_by_id<R>(&mut self, id: Uuid, f: impl FnOnce(&mut Document) -> R) -> Option<R> {
        let index = self.documents.iter().position(|doc| doc.root.identity() == Some(id))?;
        let result = f(&mut self.documents[index]);
        self.schedule_write(index);
        Some(result)
    }

    /// Replaces the value at `pointer` in the document loaded from `source` by
    /// decoding `raw` against the field's schema (units, integer/number/boolean,
    /// enums), then schedules the write. Returns `Some(true)` if set,
    /// `Some(false)` if `raw` could not be decoded for that field, `None` if the
    /// source or pointer is unknown. The UI's string-input write path.
    pub fn set_value_str(&mut self, source: &Path, pointer: &str, raw: &str) -> Option<bool> {
        let schema_id = self
            .documents
            .iter()
            .find(|doc| doc.source.as_deref() == Some(source))?
            .schema_id
            .clone();
        match crate::parse::decode_str(&self.schemas, &schema_id, pointer, raw) {
            Some(value) => self.set_value(source, pointer, value),
            None => Some(false),
        }
    }

    /// Path-free counterpart to [`Self::set_value_str`], keyed by the document's
    /// root identity `id`.
    pub fn set_value_str_by_id(&mut self, id: Uuid, pointer: &str, raw: &str) -> Option<bool> {
        let schema_id = self
            .documents
            .iter()
            .find(|doc| doc.root.identity() == Some(id))?
            .schema_id
            .clone();
        match crate::parse::decode_str(&self.schemas, &schema_id, pointer, raw) {
            Some(value) => self.set_value_by_id(id, pointer, value),
            None => Some(false),
        }
    }

    /// Path-free convenience: replaces the value at `pointer` in the document
    /// identified by `id`, scheduling its write.
    pub fn set_value_by_id(&mut self, id: Uuid, pointer: &str, value: NodeValue) -> Option<bool> {
        self.edit_by_id(id, |doc| match doc.root.get_pointer_mut(pointer) {
            Some(node) => {
                node.value = value;
                true
            }
            None => false,
        })
    }

    /// Creates a fresh instance of `schema_id` (schema defaults + generated ids),
    /// stores it one file per instance at `<collection-dir>/<uuid>.yaml`, and
    /// schedules the write. Returns the new instance's id — no path required.
    ///
    /// The schema's root must have an identity, and a collection directory must
    /// be known (via [`Self::set_data_dir`], [`Self::set_collection_dir`], or a
    /// prior [`Self::parse_directory`]).
    pub fn create_document(&mut self, schema_id: &str) -> Result<Uuid, FactoryError> {
        let root = crate::parse::instantiate(&self.schemas, schema_id)
            .ok_or_else(|| FactoryError::UnknownSchema(schema_id.to_string()))?;
        let (id, path) = self.placement(schema_id, &root)?;
        self.insert_document(Document {
            schema_id: schema_id.to_string(),
            source: Some(path),
            root,
            status: Status::Complete,
        });
        Ok(id)
    }

    /// Creates a fresh instance of `schema_id` seeded from a *source document*
    /// `source` (schema defaults/consts, then `source` deep-overlaid, then fresh
    /// identities), stores it one file per instance at
    /// `<collection-dir>/<uuid>.yaml`, and schedules the write. Returns the new
    /// instance's id.
    ///
    /// The *from a source document* counterpart to [`Self::create_document`] —
    /// e.g. materialising a new CNC profile from a bundled template. Any `id` in
    /// `source` is ignored; a fresh one is generated. As with
    /// [`Self::create_document`], the schema's root must have an identity and a
    /// collection directory must be known.
    pub fn create_document_from(
        &mut self,
        schema_id: &str,
        source: &Value,
    ) -> Result<Uuid, FactoryError> {
        let root = crate::parse::instantiate_from(&self.schemas, schema_id, source)
            .ok_or_else(|| FactoryError::UnknownSchema(schema_id.to_string()))?;
        let (id, path) = self.placement(schema_id, &root)?;
        self.insert_document(Document {
            schema_id: schema_id.to_string(),
            source: Some(path),
            root,
            status: Status::Complete,
        });
        Ok(id)
    }

    /// Replaces the entire content of the document identified by `id` by
    /// re-parsing `value` against the document's own schema — applying defaults,
    /// decoding units/ids, and re-validating — while preserving its source file,
    /// then schedules the write and re-resolves references. `value` must carry the
    /// same id. Returns the non-fatal parse problems, or `None` if `id` is unknown
    /// or the re-parse produced no document.
    ///
    /// This is the structural-edit path for complex documents: the caller mutates
    /// the plain [`Document::to_value`] (adding/removing object properties or array
    /// items) and hands it back, rather than composing node trees by hand. Re-parse
    /// re-applies schema defaults and re-marks completeness, so the tree stays
    /// consistent after structural change.
    pub fn replace_document_from_value(
        &mut self,
        id: Uuid,
        value: &Value,
    ) -> Option<Vec<DataError>> {
        let index = self
            .documents
            .iter()
            .position(|doc| doc.root.identity() == Some(id))?;
        self.replace_document_at(index, value)
    }

    /// Path-addressed twin of [`Self::replace_document_from_value`], for
    /// singleton documents (e.g. stock, settings) that have no root identity and
    /// are located by their source file. Re-parses `value` against the document's
    /// schema, swaps it in, schedules the write, and re-resolves references.
    /// Returns `None` if no loaded document has that source path.
    pub fn replace_document_from_value_at(
        &mut self,
        source: &Path,
        value: &Value,
    ) -> Option<Vec<DataError>> {
        let index = self.index_of(source)?;
        self.replace_document_at(index, value)
    }

    /// Shared core of the value-level structural-edit path: re-parse `value`
    /// against document `index`'s schema, swap the tree in, schedule its write,
    /// and re-resolve references. Re-parse re-applies schema defaults and
    /// re-marks completeness so the tree stays consistent after structural change.
    fn replace_document_at(&mut self, index: usize, value: &Value) -> Option<Vec<DataError>> {
        let schema_id = self.documents[index].schema_id.clone();
        let source = self.documents[index].source.clone();
        let text = serde_json::to_string(value).ok()?;
        let mut errors = Vec::new();
        let doc = crate::parse::parse_document(&self.schemas, &schema_id, source, &text, &mut errors)?;
        self.documents[index] = doc;
        self.schedule_write(index);
        self.resolve_all();
        Some(errors)
    }

    /// Appends a fresh item to the array at `array_pointer` in the document
    /// loaded from `source`, then schedules the write. Returns the new index.
    pub fn add_item(&mut self, source: &Path, array_pointer: &str) -> Option<usize> {
        let index = self.index_of(source)?;
        let schema_id = self.documents[index].schema_id.clone();
        let item = crate::parse::instantiate_item(&self.schemas, &schema_id, array_pointer)?;
        let position = self.push_into_array(index, array_pointer, item)?;
        self.resolve_all();
        self.schedule_write(index);
        Some(position)
    }

    /// Clones the array item at `item_pointer` (regenerating ids), appends the
    /// copy to the same array, schedules the write, and returns its index.
    pub fn clone_item(&mut self, source: &Path, item_pointer: &str) -> Option<usize> {
        let index = self.index_of(source)?;
        let clone = self.documents[index]
            .root
            .get_pointer(item_pointer)?
            .clone_with_new_ids();
        let array_pointer = parent_pointer(item_pointer)?;
        let position = self.push_into_array(index, array_pointer, clone)?;
        self.resolve_all();
        self.schedule_write(index);
        Some(position)
    }

    /// Clones the document loaded from `source` (regenerating ids) into a new
    /// per-file document, schedules its write, and returns the new id.
    pub fn clone_document(&mut self, source: &Path) -> Result<Uuid, FactoryError> {
        let index = self
            .index_of(source)
            .ok_or_else(|| FactoryError::UnknownSource(source.display().to_string()))?;
        self.clone_document_at(index)
    }

    /// Path-free clone: clones the document with root identity `id`. Returns the
    /// new id.
    pub fn clone_document_by_id(&mut self, id: Uuid) -> Result<Uuid, FactoryError> {
        let index = self
            .documents
            .iter()
            .position(|doc| doc.root.identity() == Some(id))
            .ok_or_else(|| FactoryError::UnknownSource(id.to_string()))?;
        self.clone_document_at(index)
    }

    /// Exports the document loaded from `source` to `target` (a copy write — the
    /// document keeps its own source). Reuses the background writer.
    pub fn export(&mut self, source: &Path, target: &Path) -> Option<()> {
        let index = self.index_of(source)?;
        let bytes = serialize_document(&self.documents[index]);
        self.writer
            .get_or_insert_with(Writer::start)
            .write(target.to_path_buf(), bytes);
        Some(())
    }

    /// Removes the document with root identity `id` — dropping it from the store
    /// and deleting its file — but only if nothing else references it (or any
    /// object inside it). On refusal, [`RemoveError::InUse`] lists every
    /// [`Referrer`] so the UI can point the user at what blocks the delete.
    ///
    /// The file delete is queued behind any pending writes for it, so a prior
    /// edit cannot re-create the file after removal.
    pub fn remove_document(&mut self, id: Uuid) -> Result<(), RemoveError> {
        let index = self
            .documents
            .iter()
            .position(|doc| doc.root.identity() == Some(id))
            .ok_or(RemoveError::NotFound(id))?;

        // Every id defined inside this document — a reference to any of them
        // would dangle if we removed it.
        let owned_ids = collect_identities_set(&self.documents[index].root);
        let referrers = self.referrers_to(index, &owned_ids);
        if !referrers.is_empty() {
            return Err(RemoveError::InUse { id, referrers });
        }

        let removed = self.documents.remove(index);
        if let Some(path) = removed.source {
            self.writer.get_or_insert_with(Writer::start).delete(path);
        }
        self.resolve_all();
        Ok(())
    }

    /// Every referrer, in any document other than `exclude`, whose target is one
    /// of `owned_ids`.
    fn referrers_to(&self, exclude: usize, owned_ids: &HashSet<Uuid>) -> Vec<Referrer> {
        let mut out = Vec::new();
        for (idx, doc) in self.documents.iter().enumerate() {
            if idx == exclude {
                continue;
            }
            collect_referrers(&doc.root, "", owned_ids, doc, &mut out);
        }
        out
    }

    // ---- internals ----

    fn index_of(&self, source: &Path) -> Option<usize> {
        self.documents
            .iter()
            .position(|doc| doc.source.as_deref() == Some(source))
    }

    /// The directory a schema's files live in: an explicit association, else
    /// `<data_dir>/<schema-stem>`.
    fn collection_dir(&self, schema_id: &str) -> Option<PathBuf> {
        if let Some(dir) = self.collections.get(schema_id) {
            return Some(dir.clone());
        }
        self.data_dir
            .as_ref()
            .map(|base| base.join(collection_name(schema_id)))
    }

    /// The `(id, path)` placement for a new per-file document: its root identity
    /// and `<collection-dir>/<uuid>.yaml`.
    fn placement(&self, schema_id: &str, root: &Node) -> Result<(Uuid, PathBuf), FactoryError> {
        let id = root
            .identity()
            .ok_or_else(|| FactoryError::NotPerFile(schema_id.to_string()))?;
        let dir = self.collection_dir(schema_id).ok_or(FactoryError::NoDataDir)?;
        Ok((id, dir.join(format!("{id}.yaml"))))
    }

    /// Clones the document at `index` into a new per-file document; returns its id.
    fn clone_document_at(&mut self, index: usize) -> Result<Uuid, FactoryError> {
        let schema_id = self.documents[index].schema_id.clone();
        let root = self.documents[index].root.clone_with_new_ids();
        let (id, path) = self.placement(&schema_id, &root)?;
        self.insert_document(Document {
            schema_id,
            source: Some(path),
            root,
            status: Status::Complete,
        });
        Ok(id)
    }

    fn push_into_array(&mut self, index: usize, array_pointer: &str, item: Node) -> Option<usize> {
        let array = self.documents[index].root.get_pointer_mut(array_pointer)?;
        if let NodeValue::Array(items) = &mut array.value {
            items.push(item);
            Some(items.len() - 1)
        } else {
            None
        }
    }

    /// Pushes a new document, re-resolves the store, and schedules the new
    /// document's write.
    fn insert_document(&mut self, document: Document) {
        let index = self.documents.len();
        self.documents.push(document);
        self.resolve_all();
        self.schedule_write(index);
    }

    /// Rebuilds the identity registry and re-resolves every reference across all
    /// documents, refreshing each document's status. Idempotent.
    fn resolve_all(&mut self) {
        self.reindex();
        for doc in self.documents.iter_mut() {
            resolve_node(&mut doc.root, &self.by_uuid);
            doc.status = aggregate_status(&doc.root);
        }
    }

    /// Rebuilds the identity registry from scratch after a structural change.
    fn reindex(&mut self) {
        let mut locations = Vec::new();
        let mut by_uuid = HashMap::new();
        for (doc_idx, doc) in self.documents.iter().enumerate() {
            index_node(doc_idx, &doc.root, "", &mut locations, &mut by_uuid);
        }
        self.locations = locations;
        self.by_uuid = by_uuid;
    }

    /// Serializes the document at `index` to bytes (a copy, on this thread) and
    /// hands it to the background writer, starting the writer if needed.
    fn schedule_write(&mut self, index: usize) {
        let Some(path) = self.documents[index].source.clone() else {
            return; // no source: nothing to persist to
        };
        let bytes = serialize_document(&self.documents[index]);
        self.writer.get_or_insert_with(Writer::start).write(path, bytes);
    }
}

/// Collects every identity in a subtree into a set.
fn collect_identities_set(root: &Node) -> HashSet<Uuid> {
    fn walk(node: &Node, set: &mut HashSet<Uuid>) {
        match &node.value {
            NodeValue::Id(id) => {
                set.insert(*id);
            }
            NodeValue::Object(map) => map.values().for_each(|c| walk(c, set)),
            NodeValue::Array(items) => items.iter().for_each(|c| walk(c, set)),
            _ => {}
        }
    }
    let mut set = HashSet::new();
    walk(root, &mut set);
    set
}

/// Records every reference within `doc` whose target is in `owned_ids`.
fn collect_referrers(
    node: &Node,
    pointer: &str,
    owned_ids: &HashSet<Uuid>,
    doc: &Document,
    out: &mut Vec<Referrer>,
) {
    match &node.value {
        NodeValue::Ref(reference) if owned_ids.contains(&reference.raw) => out.push(Referrer {
            document_id: doc.root.identity(),
            source: doc.source.clone(),
            pointer: pointer.to_string(),
            target_id: reference.raw,
        }),
        NodeValue::Object(map) => {
            for (key, child) in map {
                collect_referrers(child, &join_pointer(pointer, key), owned_ids, doc, out);
            }
        }
        NodeValue::Array(items) => {
            for (idx, child) in items.iter().enumerate() {
                collect_referrers(child, &format!("{pointer}/{idx}"), owned_ids, doc, out);
            }
        }
        _ => {}
    }
}

/// The collection directory name for a schema id: its stem (without `.yaml`).
fn collection_name(schema_id: &str) -> String {
    schema_id
        .strip_suffix(".yaml")
        .or_else(|| schema_id.strip_suffix(".yml"))
        .unwrap_or(schema_id)
        .to_string()
}

/// The parent of a JSON Pointer (`/items/0` → `/items`).
fn parent_pointer(pointer: &str) -> Option<&str> {
    pointer.rfind('/').map(|idx| &pointer[..idx])
}

/// Whether `path` is an existing `.yaml`/`.yml` file.
fn is_yaml_file(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("yaml") || ext.eq_ignore_ascii_case("yml"))
            .unwrap_or(false)
}

/// Serializes a document to YAML bytes for write-back, stamping the reserved
/// `$schema` meta-key at the top. Values round-trip losslessly; YAML comments
/// are not preserved.
fn serialize_document(document: &Document) -> Vec<u8> {
    let mut value = document.to_value();
    if let Value::Object(map) = &value {
        let mut stamped = serde_json::Map::with_capacity(map.len() + 1);
        stamped.insert("$schema".to_string(), Value::String(document.schema_id.clone()));
        for (key, child) in map {
            stamped.insert(key.clone(), child.clone());
        }
        value = Value::Object(stamped);
    }
    serde_yaml::to_string(&value).unwrap_or_default().into_bytes()
}

/// Registers every object that has an `Id` child, keyed by that UUID.
fn index_node(
    doc: usize,
    node: &Node,
    pointer: &str,
    locations: &mut Vec<Location>,
    by_uuid: &mut HashMap<Uuid, Handle>,
) {
    match &node.value {
        NodeValue::Object(map) => {
            if let Some(id) = map.values().find_map(|child| match child.value {
                NodeValue::Id(id) => Some(id),
                _ => None,
            }) {
                if !by_uuid.contains_key(&id) {
                    let handle = Handle(locations.len());
                    locations.push(Location {
                        doc,
                        pointer: pointer.to_string(),
                    });
                    by_uuid.insert(id, handle);
                }
            }
            for (key, child) in map {
                index_node(doc, child, &join_pointer(pointer, key), locations, by_uuid);
            }
        }
        NodeValue::Array(items) => {
            for (idx, child) in items.iter().enumerate() {
                index_node(doc, child, &format!("{pointer}/{idx}"), locations, by_uuid);
            }
        }
        _ => {}
    }
}

/// Resolves each reference node against the identity registry. Idempotent: it
/// re-resolves already-linked references too, so handles stay valid after the
/// registry is rebuilt (e.g. when documents are added).
fn resolve_node(node: &mut Node, by_uuid: &HashMap<Uuid, Handle>) {
    // Look up first (immutable), then apply (mutable) to avoid overlapping borrows.
    let outcome = match &node.value {
        NodeValue::Ref(reference) => Some(by_uuid.get(&reference.raw).copied().ok_or(reference.raw)),
        _ => None,
    };

    match outcome {
        Some(Ok(handle)) => {
            if let NodeValue::Ref(reference) = &mut node.value {
                reference.state = RefState::Resolved(handle);
            }
            node.status = Status::Complete;
        }
        Some(Err(raw)) => {
            if let NodeValue::Ref(reference) = &mut node.value {
                reference.state = RefState::Unresolved;
            }
            node.status = Status::Incomplete(vec![Reason::UnresolvedRef(raw)]);
        }
        None => {}
    }

    match &mut node.value {
        NodeValue::Object(map) => {
            for child in map.values_mut() {
                resolve_node(child, by_uuid);
            }
        }
        NodeValue::Array(items) => {
            for child in items.iter_mut() {
                resolve_node(child, by_uuid);
            }
        }
        _ => {}
    }
}

/// Appends a (pointer-escaped) key to a JSON Pointer.
fn join_pointer(pointer: &str, key: &str) -> String {
    let escaped = key.replace('~', "~0").replace('/', "~1");
    format!("{pointer}/{escaped}")
}

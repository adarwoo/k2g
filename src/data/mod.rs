//! Central data API — the single facade that owns all persisted application
//! data through the [`datastore`] crate.
//!
//! See [docs/data-api.md](../../docs/data-api.md) for the full design. `AppData` is
//! now the single reader and writer of every mutable persisted realm (settings,
//! stock, and the profile collections): the runtime hydrates its in-memory state
//! from here at launch and mirrors edits straight back down. The legacy `config`
//! persistence layer has been retired; only read-only catalog loading remains
//! outside AppData (see [`crate::catalog_io`]).
//!
//! `AppData` manages:
//! - **Settings** and **Stock** — singletons at fixed paths under the data dir.
//! - **CNC / Fixture / Toolset / Machining** — per-file profile collections.
//! - **Catalog** — a read-only collection whose tools are reference targets for
//!   stock items.
//! - **CNC templates** — bundled seeds used to create new CNC profiles.

#![allow(dead_code)]

/// The typed application model (profiles, stock, catalog, job, unit-bearing
/// tool core). Formerly the top-level `domain` module; it lives under `data`
/// because these are the shapes `AppData` reads from and writes to the store.
pub mod model;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use datastore::{
    DataError, DataStore, Document, FactoryError, Handle, Node, NodeValue, ParseInput,
    RemoveError, ResolvedStore, WriteError,
};
use log::warn;
use serde_json::Value;
use uuid::Uuid;

use crate::paths::AppDirs;

/// Every schema the application persists, embedded at build time. The order is
/// irrelevant except that referenced schemas (`id.yaml`, `units.yaml`) must be
/// present — cross-file `$ref`s are wired by the builder.
const SCHEMAS: &[(&str, &str)] = &[
    ("id.yaml", include_str!("../../schemas/id.yaml")),
    ("units.yaml", include_str!("../../schemas/units.yaml")),
    ("settings.yaml", include_str!("../../schemas/settings.yaml")),
    ("stock.yaml", include_str!("../../schemas/stock.yaml")),
    ("cnc.yaml", include_str!("../../schemas/cnc.yaml")),
    ("fixture.yaml", include_str!("../../schemas/fixture.yaml")),
    ("toolset.yaml", include_str!("../../schemas/toolset.yaml")),
    ("machining.yaml", include_str!("../../schemas/machining.yaml")),
    ("catalog.yaml", include_str!("../../schemas/catalog.yaml")),
];

/// Bundled CNC templates: `(key, embedded YAML)`. Each is a `cnc.yaml`-shaped
/// seed with no `id`; see [`AppData::create_cnc_from_template`].
const CNC_TEMPLATES: &[(&str, &str)] = &[
    ("genmitsu_3018", include_str!("../../assets/cnc_templates/genmitsu_3018.yaml")),
    ("masso_g3_with_atc", include_str!("../../assets/cnc_templates/masso_g3_with_atc.yaml")),
    ("masso_g3_no_atc", include_str!("../../assets/cnc_templates/masso_g3_no_atc.yaml")),
    ("batam", include_str!("../../assets/cnc_templates/batam.yaml")),
];

/// Reserved meta-key stamped at the top of every persisted file (mirrors the
/// datastore writer) so seeded singleton files match the on-disk format.
const SCHEMA_META_KEY: &str = "$schema";

const SETTINGS_FILE: &str = "global.setting.yaml";
const STOCK_FILE: &str = "stock.yaml";

/// The four id'd, per-file profile collections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    Cnc,
    Fixture,
    Toolset,
    Machining,
}

impl Profile {
    pub const ALL: [Profile; 4] = [Profile::Cnc, Profile::Fixture, Profile::Toolset, Profile::Machining];

    /// The schema id backing this collection.
    fn schema_id(self) -> &'static str {
        match self {
            Profile::Cnc => "cnc.yaml",
            Profile::Fixture => "fixture.yaml",
            Profile::Toolset => "toolset.yaml",
            Profile::Machining => "machining.yaml",
        }
    }

    /// The subdirectory (under the data dir) holding this collection's files.
    ///
    /// Machining lives in `processing_profiles` — the legacy on-disk location —
    /// so AppData operates in place on the user's real data (its files are
    /// normalized into `machining.yaml` form on load; see
    /// [`normalize_machining_value`]).
    fn dir_name(self) -> &'static str {
        match self {
            Profile::Cnc => "cnc_profiles",
            Profile::Fixture => "fixture_profiles",
            Profile::Toolset => "toolset_profiles",
            Profile::Machining => "processing_profiles",
        }
    }
}

/// A bundled CNC template parsed into a reusable seed.
struct CncTemplate {
    key: &'static str,
    name: String,
    seed: Value,
}

/// Lightweight descriptor of a CNC template for the UI picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateInfo {
    pub key: String,
    pub name: String,
}

/// The central data facade. Holds one live, auto-persisting [`ResolvedStore`]
/// plus the parsed CNC templates.
pub struct AppData {
    store: ResolvedStore,
    cnc_templates: Vec<CncTemplate>,
    settings_path: PathBuf,
    stock_path: PathBuf,
}

impl AppData {
    /// Loads all data rooted at the standard application directories.
    /// Convenience wrapper over [`Self::load_from`].
    ///
    /// Operates in place on the existing `configs/` tree (where the legacy layer
    /// also keeps its files) so migrated screens edit the user's real profiles
    /// without a data migration. Single-writer discipline per realm keeps this
    /// safe while both layers coexist: a realm is written by exactly one of them.
    pub fn load(dirs: &AppDirs) -> (Self, Vec<DataError>) {
        Self::load_from(&dirs.configs, &dirs.catalogs)
    }

    /// Loads all data from an explicit data directory and catalog directory.
    ///
    /// Missing singleton files are seeded with schema defaults; missing
    /// collection directories are created empty. Returns every non-fatal problem
    /// found (parse/validation errors) without aborting the load.
    pub fn load_from(data_dir: &Path, catalogs_dir: &Path) -> (Self, Vec<DataError>) {
        let schemas = build_datastore();
        let mut errors = Vec::new();

        // Ensure the directory tree exists so directory reads never fail on a
        // fresh install.
        ensure_dir(data_dir);
        ensure_dir(catalogs_dir);
        for profile in Profile::ALL {
            ensure_dir(&data_dir.join(profile.dir_name()));
        }

        let settings_path = data_dir.join(SETTINGS_FILE);
        let stock_path = data_dir.join(STOCK_FILE);

        // Seed the singletons if absent, then parse them as the initial docs.
        seed_singleton_if_missing(&schemas, "settings.yaml", &settings_path);
        seed_singleton_if_missing(&schemas, "stock.yaml", &stock_path);

        // A legacy `global.setting.yaml`/`stock.yaml` (written before the
        // datastore's `x-schema-version` gating, by the retired
        // `save_global_settings`/`save_stock`) lacks the `schema_version` field the
        // gate now requires; inject it on load so such files still parse (and are
        // then rewritten in modern form by the AppData writer).
        let settings_text = fs::read_to_string(&settings_path).ok().map(|text| inject_schema_version(&text));
        let stock_text = fs::read_to_string(&stock_path).ok().map(|text| inject_schema_version(&text));
        let mut inputs = Vec::new();
        if let Some(text) = &settings_text {
            inputs.push(ParseInput { schema_id: "settings.yaml", source: Some(settings_path.clone()), text });
        }
        if let Some(text) = &stock_text {
            inputs.push(ParseInput { schema_id: "stock.yaml", source: Some(stock_path.clone()), text });
        }
        let outcome = schemas.parse(&inputs);
        errors.extend(outcome.errors);
        let mut store = schemas.resolve(outcome.documents);

        // Load each profile collection (registers its directory for new files).
        // Machining files on disk carry a legacy shape (per-op `enabled` flags,
        // empty-string refs); they are normalized before parsing.
        for profile in Profile::ALL {
            let dir = data_dir.join(profile.dir_name());
            if profile == Profile::Machining {
                errors.extend(load_machining_normalized(&mut store, &dir));
            } else {
                errors.extend(store.parse_directory(profile.schema_id(), &dir));
            }
        }

        // Load the read-only catalog collection last, so stock references
        // resolve against catalog tools on the final pass.
        errors.extend(store.parse_directory("catalog.yaml", catalogs_dir));

        let cnc_templates = load_cnc_templates();

        (
            Self { store, cnc_templates, settings_path, stock_path },
            errors,
        )
    }

    // ---- singletons -------------------------------------------------------

    /// The settings document, if loaded.
    pub fn settings(&self) -> Option<&Document> {
        self.singleton("settings.yaml")
    }

    /// Replaces a settings field by JSON Pointer and schedules the write.
    pub fn set_setting(&mut self, pointer: &str, value: NodeValue) -> Option<bool> {
        self.store.set_value(&self.settings_path, pointer, value)
    }

    /// Replaces the entire settings document from a plain value, re-parsing it
    /// against the schema and scheduling the write. The single-writer bridge for
    /// `global.setting.yaml`: the runtime's in-memory settings (units, theme,
    /// selected-profile ids) are mirrored here so AppData is the sole writer of the
    /// settings singleton. Returns any parse problems, or `None` if the settings
    /// document is not loaded.
    pub fn replace_settings_from_value(&mut self, value: &Value) -> Option<Vec<DataError>> {
        let path = self.settings_path.clone();
        self.store.replace_document_from_value_at(&path, value)
    }

    /// The stock document, if loaded.
    pub fn stock(&self) -> Option<&Document> {
        self.singleton("stock.yaml")
    }

    /// Appends a fresh, defaulted stock tool item; returns its index.
    pub fn add_stock_item(&mut self) -> Option<usize> {
        self.store.add_item(&self.stock_path, "/tools")
    }

    /// Clones the stock item at `index`; returns the new item's index.
    pub fn clone_stock_item(&mut self, index: usize) -> Option<usize> {
        self.store.clone_item(&self.stock_path, &format!("/tools/{index}"))
    }

    /// Removes the stock item at `index`, scheduling the write. Returns whether an
    /// item was removed. (Stock is a singleton addressed by its file, not by id.)
    pub fn remove_stock_item(&mut self, index: usize) -> bool {
        let path = self.stock_path.clone();
        let removed = self.store.edit(&path, |doc| {
            match doc.root.get_pointer_mut("/tools").map(|node| &mut node.value) {
                Some(NodeValue::Array(items)) if index < items.len() => {
                    items.remove(index);
                    true
                }
                _ => false,
            }
        });
        if removed == Some(true) {
            self.store.resolve_references();
        }
        removed.unwrap_or(false)
    }

    /// Sets a stock field from a raw input string, schema-decoded (the UI write
    /// path for stock tool fields). `Some(true)` if set, `Some(false)` if `raw`
    /// could not be decoded, `None` if the pointer is unknown.
    pub fn set_stock_str(&mut self, pointer: &str, raw: &str) -> Option<bool> {
        let path = self.stock_path.clone();
        self.store.set_value_str(&path, pointer, raw)
    }

    /// Sets a stock field to a typed value directly (e.g. an enum/bool from a
    /// select or checkbox), scheduling the write.
    pub fn set_stock_value(&mut self, pointer: &str, value: NodeValue) -> Option<bool> {
        let path = self.stock_path.clone();
        self.store.set_value(&path, pointer, value)
    }

    /// Replaces the entire stock document from a plain value (the legacy
    /// `stock_value_from_tools` projection), re-parsing it against the schema and
    /// scheduling the write. This is the single-writer bridge: the Stock screen's
    /// in-memory tool list is the edit buffer, and every change is mirrored here
    /// so AppData is the sole writer of `stock.yaml`. Returns any parse problems,
    /// or `None` if the stock document is not loaded.
    pub fn replace_stock_from_value(&mut self, value: &Value) -> Option<Vec<DataError>> {
        let path = self.stock_path.clone();
        self.store.replace_document_from_value_at(&path, value)
    }

    /// Appends pre-built stock tool-item values (the nested `ref`/`base`/… shape,
    /// e.g. from the catalog picker's projection) to `/tools`, renumbering `order`
    /// to stay monotonic, then re-parsing via the sole writer. Returns the count
    /// appended. Lets a caller add tools without round-tripping the whole legacy
    /// projection.
    pub fn append_stock_tool_values(&mut self, items: &[Value]) -> usize {
        if items.is_empty() {
            return 0;
        }
        let Some(mut value) = self.stock().map(|doc| doc.to_value()) else {
            return 0;
        };
        let Some(tools) = value.get_mut("tools").and_then(Value::as_array_mut) else {
            return 0;
        };
        let base = tools.len();
        for (offset, item) in items.iter().enumerate() {
            let mut item = item.clone();
            if let Some(obj) = item.as_object_mut() {
                obj.insert("order".to_string(), Value::from((base + offset) as i64));
            }
            tools.push(item);
        }
        self.replace_stock_from_value(&value);
        items.len()
    }

    /// Removes every stock tool whose `id` is in `ids`, re-parsing via the sole
    /// writer. Returns the number removed. Removal is by id (the stock singleton
    /// is path-addressed and its tools carry app-managed ids), so a filtered/sorted
    /// UI selection maps cleanly without tracking array indices.
    pub fn remove_stock_tools_by_ids(&mut self, ids: &[String]) -> usize {
        if ids.is_empty() {
            return 0;
        }
        let Some(mut value) = self.stock().map(|doc| doc.to_value()) else {
            return 0;
        };
        let Some(tools) = value.get_mut("tools").and_then(Value::as_array_mut) else {
            return 0;
        };
        let id_set: std::collections::HashSet<&str> = ids.iter().map(String::as_str).collect();
        let before = tools.len();
        tools.retain(|tool| {
            tool.get("id")
                .and_then(Value::as_str)
                .map(|id| !id_set.contains(id))
                .unwrap_or(true)
        });
        let removed = before - tools.len();
        if removed > 0 {
            self.replace_stock_from_value(&value);
        }
        removed
    }

    // ---- collections ------------------------------------------------------

    /// Every loaded document of a profile kind, paired with its id.
    pub fn list(&self, profile: Profile) -> Vec<(Uuid, &Document)> {
        let schema_id = profile.schema_id();
        self.store
            .documents()
            .iter()
            .filter(|doc| doc.schema_id == schema_id)
            .filter_map(|doc| doc.root.identity().map(|id| (id, doc)))
            .collect()
    }

    /// The document with root identity `id`, of any kind.
    pub fn get(&self, id: Uuid) -> Option<&Document> {
        self.store.document_by_id(id)
    }

    /// Creates a new profile from schema defaults; returns its id.
    pub fn create(&mut self, profile: Profile) -> Result<Uuid, FactoryError> {
        self.store.create_document(profile.schema_id())
    }

    /// Duplicates an existing profile (fresh ids); returns the new id.
    pub fn clone(&mut self, id: Uuid) -> Result<Uuid, FactoryError> {
        self.store.clone_document_by_id(id)
    }

    /// Creates a new profile of `kind` seeded from an arbitrary value (e.g. an
    /// imported YAML document), assigning fresh ids. Returns the new id.
    pub fn create_from_value(&mut self, kind: Profile, seed: &Value) -> Result<Uuid, FactoryError> {
        self.store.create_document_from(kind.schema_id(), seed)
    }

    /// Serializes the document `id` to YAML (for export/download).
    pub fn document_yaml(&self, id: Uuid) -> Option<String> {
        let value = self.get(id)?.to_value();
        serde_yaml::to_string(&value).ok()
    }

    /// Edits a document in place and schedules its write. Re-resolves references
    /// afterwards in case the closure changed structure.
    pub fn edit(&mut self, id: Uuid, f: impl FnOnce(&mut Document)) {
        if self.store.edit_by_id(id, f).is_some() {
            self.store.resolve_references();
        }
    }

    /// Replaces a single field of a profile by JSON Pointer and schedules the
    /// write.
    pub fn set_field(&mut self, id: Uuid, pointer: &str, value: NodeValue) -> Option<bool> {
        self.store.set_value_by_id(id, pointer, value)
    }

    /// Sets a profile field from a raw input string, decoding it against the
    /// field's schema (units, integer/number/boolean, enums). The UI's
    /// string-input write path. `Some(true)` if set, `Some(false)` if `raw`
    /// could not be decoded, `None` if the id/pointer is unknown.
    pub fn set_str(&mut self, id: Uuid, pointer: &str, raw: &str) -> Option<bool> {
        self.store.set_value_str_by_id(id, pointer, raw)
    }

    /// Sets a settings-singleton field from a raw input string (schema-decoded).
    pub fn set_setting_str(&mut self, pointer: &str, raw: &str) -> Option<bool> {
        let path = self.settings_path.clone();
        self.store.set_value_str(&path, pointer, raw)
    }

    /// Removes a profile and deletes its file, unless something still references
    /// it (then [`RemoveError::InUse`] names the referrers).
    pub fn remove(&mut self, id: Uuid) -> Result<(), RemoveError> {
        self.store.remove_document(id)
    }

    // ---- catalog (read-only) ---------------------------------------------

    /// Every loaded tool catalog.
    pub fn catalogs(&self) -> impl Iterator<Item = &Document> {
        self.store
            .documents()
            .iter()
            .filter(|doc| doc.schema_id == "catalog.yaml")
    }

    /// Follows a resolved reference handle (e.g. a stock item's catalog tool).
    pub fn tool(&self, handle: Handle) -> Option<&Node> {
        self.store.get(handle)
    }

    // ---- CNC templates ----------------------------------------------------

    /// The available CNC templates, as `(key, name)` descriptors.
    pub fn cnc_templates(&self) -> Vec<TemplateInfo> {
        self.cnc_templates
            .iter()
            .map(|t| TemplateInfo { key: t.key.to_string(), name: t.name.clone() })
            .collect()
    }

    /// Creates a new CNC profile seeded from the template `key`; returns its id.
    pub fn create_cnc_from_template(&mut self, key: &str) -> Result<Uuid, FactoryError> {
        let seed = self
            .cnc_templates
            .iter()
            .find(|t| t.key == key)
            .map(|t| t.seed.clone())
            .ok_or_else(|| FactoryError::UnknownSource(format!("cnc template '{key}'")))?;
        self.store.create_document_from("cnc.yaml", &seed)
    }

    // ---- machining structural edits --------------------------------------
    //
    // Machining documents have structural fields the fine-grained setters can't
    // express: the cnc/fixture/toolset bindings (a `default` reference plus a
    // `choices` array) and the `operations` array. These edit the plain document
    // value and re-parse it (see [`ResolvedStore::replace_document_from_value`]).

    /// Edits a document at the plain-value level and re-parses it (structural
    /// edits that the fine-grained setters can't express). Returns `false` if
    /// `id` is unknown or the re-parse produced no document.
    fn edit_document_value(&mut self, id: Uuid, f: impl FnOnce(&mut Value)) -> bool {
        let Some(mut value) = self.get(id).map(|doc| doc.to_value()) else {
            return false;
        };
        f(&mut value);
        self.store.replace_document_from_value(id, &value).is_some()
    }

    /// Sets a machining profile's binding for `field` (`"cnc"`, `"fixture"`, or
    /// `"toolset"`): the active `default` reference (absent when `None`) and the
    /// allowed `choices`.
    pub fn set_machining_binding(
        &mut self,
        id: Uuid,
        field: &str,
        default: Option<Uuid>,
        choices: &[Uuid],
    ) -> bool {
        let field = field.to_string();
        self.edit_document_value(id, |value| {
            let Some(binding) = value.get_mut(&field).and_then(Value::as_object_mut) else {
                return;
            };
            match default {
                Some(uuid) => {
                    binding.insert("default".into(), Value::String(uuid.to_string()));
                }
                None => {
                    binding.remove("default");
                }
            }
            binding.insert(
                "choices".into(),
                Value::Array(choices.iter().map(|u| Value::String(u.to_string())).collect()),
            );
        })
    }

    /// Sets the machining profile's enabled `operations`, in order. Each entry is
    /// an operation key (e.g. `"drill_pth"`). The per-operation config objects are
    /// always present (schema defaults); this only changes what is *enabled*.
    pub fn set_machining_operations(&mut self, id: Uuid, operations: &[String]) -> bool {
        self.edit_document_value(id, |value| {
            let Some(obj) = value.as_object_mut() else {
                return;
            };
            obj.insert(
                "operations".into(),
                Value::Array(operations.iter().map(|s| Value::String(s.clone())).collect()),
            );
        })
    }

    // ---- toolset rack edits ----------------------------------------------
    //
    // A toolset's `slots` are a `T1..Tn` rack: each slot has a `mode`
    // (`fixed`/`spare`/`do_not_use`) and, when fixed, a `tool_id`. The schema
    // forbids `tool_id` unless the slot is fixed, so switching away from fixed
    // must also drop it — a structural change made at the value level.

    /// Sets the slot at array position `slot_pos`: its `mode`, and (only for a
    /// `fixed` slot) its `tool_id`, which is removed for `spare`/`do_not_use`.
    pub fn set_toolset_slot_mode(
        &mut self,
        id: Uuid,
        slot_pos: usize,
        mode: &str,
        tool_id: Option<Uuid>,
    ) -> bool {
        let mode = mode.to_string();
        self.edit_document_value(id, |value| {
            let Some(slot) = value
                .pointer_mut(&format!("/slots/{slot_pos}"))
                .and_then(Value::as_object_mut)
            else {
                return;
            };
            slot.insert("mode".into(), Value::String(mode.clone()));
            match tool_id {
                Some(uuid) if mode == "fixed" => {
                    slot.insert("tool_id".into(), Value::String(uuid.to_string()));
                }
                _ => {
                    slot.remove("tool_id");
                }
            }
        })
    }

    /// Resizes the rack to `count` slots (clamped 1..=64). New slots are `spare`;
    /// removed slots are dropped from the end. Slot `index` values stay `1..=n`.
    pub fn set_toolset_slot_count(&mut self, id: Uuid, count: usize) -> bool {
        let count = count.clamp(1, 64);
        self.edit_document_value(id, |value| {
            let Some(slots) = value.get_mut("slots").and_then(Value::as_array_mut) else {
                return;
            };
            let current = slots.len();
            if count > current {
                for i in current..count {
                    slots.push(serde_json::json!({ "index": i + 1, "mode": "spare" }));
                }
            } else {
                slots.truncate(count);
            }
        })
    }

    // ---- lifecycle --------------------------------------------------------

    /// Blocks until all scheduled writes have completed (e.g. at shutdown).
    pub fn flush(&self) {
        self.store.flush();
    }

    /// Drains and returns any background write errors so far.
    pub fn write_errors(&self) -> Vec<WriteError> {
        self.store.write_errors()
    }

    // ---- internals --------------------------------------------------------

    fn singleton(&self, schema_id: &str) -> Option<&Document> {
        self.store
            .documents()
            .iter()
            .find(|doc| doc.schema_id == schema_id)
    }
}

// ---------------------------------------------------------------------------
// Global singleton — the process-wide live store the UI binds to.
//
// AppData owns a background writer thread and is therefore not `Clone`, so
// (unlike the legacy `AppCtx`) it cannot live inside a cloned Dioxus signal.
// It lives here behind an `RwLock`, mirroring `GLOBAL_CTX`; the UI subscribes to
// changes via a separate render-counter signal (see `ui::bindings`).
// ---------------------------------------------------------------------------

static APP_DATA: OnceLock<RwLock<AppData>> = OnceLock::new();

/// Initializes the global [`AppData`] store from the standard application
/// directories. Idempotent (a second call is ignored). Returns any non-fatal
/// load problems. Safe to call at startup alongside the legacy context.
pub fn init_appdata() -> Vec<DataError> {
    match crate::paths::ensure_app_dirs() {
        Ok(dirs) => {
            let (data, errors) = AppData::load(&dirs);
            let _ = APP_DATA.set(RwLock::new(data));
            errors
        }
        Err(error) => {
            warn!("AppData init skipped: {error}");
            Vec::new()
        }
    }
}

/// Whether [`init_appdata`] has run.
pub fn appdata_ready() -> bool {
    APP_DATA.get().is_some()
}

/// Runs `f` with a shared read lock on the global store. Panics if the store has
/// not been initialized by [`init_appdata`].
pub fn with_appdata<R>(f: impl FnOnce(&AppData) -> R) -> R {
    let lock = APP_DATA.get().expect("AppData must be initialized before use");
    let guard = lock.read().expect("AppData read lock poisoned");
    f(&guard)
}

/// Runs `f` with an exclusive write lock on the global store.
pub fn with_appdata_mut<R>(f: impl FnOnce(&mut AppData) -> R) -> R {
    let lock = APP_DATA.get().expect("AppData must be initialized before use");
    let mut guard = lock.write().expect("AppData write lock poisoned");
    f(&mut guard)
}

/// Compiles the embedded schemas into a [`DataStore`]. The schemas are validated
/// by a test (`all_embedded_schemas_are_valid`), so a compile failure here is a
/// build-time bug, not a runtime condition.
fn build_datastore() -> DataStore {
    let mut builder = DataStore::builder();
    for (id, text) in SCHEMAS {
        builder = builder.schema(id, text);
    }
    builder.build().expect("embedded schemas must compile")
}

/// Parses the bundled CNC templates into reusable seeds, taking each display
/// name from the template's `name` field (falling back to the key).
fn load_cnc_templates() -> Vec<CncTemplate> {
    let mut out = Vec::new();
    for (key, text) in CNC_TEMPLATES {
        match parse_yaml_value(text) {
            Some(value) => {
                let name = value
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or(key)
                    .to_string();
                out.push(CncTemplate { key, name, seed: value });
            }
            None => warn!("bundled CNC template '{key}' failed to parse; skipping"),
        }
    }
    out
}

/// Writes schema-default content for a singleton if its file does not yet exist,
/// stamping the reserved `$schema` key so it matches the datastore write format.
fn seed_singleton_if_missing(schemas: &DataStore, schema_id: &str, path: &Path) {
    if path.exists() {
        return;
    }
    let Some(node) = schemas.instantiate(schema_id) else {
        warn!("cannot seed singleton '{schema_id}': unknown schema");
        return;
    };

    let stamped = match node.to_value() {
        Value::Object(map) => {
            let mut out = serde_json::Map::with_capacity(map.len() + 1);
            out.insert(SCHEMA_META_KEY.to_string(), Value::String(schema_id.to_string()));
            out.extend(map);
            Value::Object(out)
        }
        other => other,
    };

    match serde_yaml::to_string(&stamped) {
        Ok(text) => {
            if let Some(parent) = path.parent() {
                ensure_dir(parent);
            }
            if let Err(error) = fs::write(path, text) {
                warn!("failed to seed singleton '{}': {error}", path.display());
            }
        }
        Err(error) => warn!("failed to serialize singleton '{schema_id}': {error}"),
    }
}

/// Parses YAML text into a JSON [`Value`], returning `None` on any parse error.
fn parse_yaml_value(text: &str) -> Option<Value> {
    let yaml: serde_yaml::Value = serde_yaml::from_str(text).ok()?;
    serde_json::to_value(yaml).ok()
}

/// Whether `path` names a YAML file.
fn is_yaml(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("yaml") | Some("yml")
    )
}

/// Loads machining profiles from `dir`, normalizing each on-disk file into
/// `machining.yaml` form (see [`normalize_machining_value`]) before parsing, and
/// registers `dir` as the machining collection so new/edited files land there.
fn load_machining_normalized(store: &mut ResolvedStore, dir: &Path) -> Vec<DataError> {
    let mut items: Vec<(PathBuf, String)> = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !is_yaml(&path) {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            let Some(mut value) = parse_yaml_value(&text) else {
                continue;
            };
            normalize_machining_value(&mut value);
            if let Ok(normalized) = serde_json::to_string(&value) {
                items.push((path, normalized));
            }
        }
    }
    store.parse_texts("machining.yaml", dir, &items)
}

/// Converts a legacy `processing_profiles` value into `machining.yaml` form:
///
/// - removes each operation object's `enabled` flag — the schema drives
///   enablement from the `operations` array, and `additionalProperties: false`
///   would otherwise reject the field;
/// - drops empty-string references so an unset cnc/fixture/toolset reads as
///   *absent* (hence incomplete, prompting the user) rather than an invalid UUID.
///
/// Operation config objects are left in place (always materialized by the
/// loader); only their `enabled` flag is stripped.
fn normalize_machining_value(value: &mut Value) {
    let Some(obj) = value.as_object_mut() else {
        return;
    };

    for key in ["drill_locating_pins", "drill_pth", "drill_npth", "route_board", "mill_board"] {
        if let Some(op) = obj.get_mut(key).and_then(Value::as_object_mut) {
            op.remove("enabled");
        }
    }

    for key in ["cnc", "fixture", "toolset"] {
        let Some(binding) = obj.get_mut(key).and_then(Value::as_object_mut) else {
            continue;
        };
        if binding.get("default").and_then(Value::as_str) == Some("") {
            binding.remove("default");
        }
        if let Some(choices) = binding.get_mut("choices").and_then(Value::as_array_mut) {
            choices.retain(|choice| choice.as_str().map(|s| !s.is_empty()).unwrap_or(true));
        }
    }
}

/// Injects `schema_version: 1` into a singleton's on-disk text when absent, so a
/// legacy file (written before the datastore's `x-schema-version` gating — e.g. by
/// the retired `save_stock`/`save_global_settings`) still parses. Returns a JSON
/// string (a superset-compatible input for the YAML-or-JSON parser); on any parse
/// failure it passes the original text through unchanged so the normal error path
/// reports it.
fn inject_schema_version(text: &str) -> String {
    let Some(mut value) = parse_yaml_value(text) else {
        return text.to_string();
    };
    if let Some(obj) = value.as_object_mut() {
        obj.entry("schema_version").or_insert(Value::from(1));
    }
    serde_json::to_string(&value).unwrap_or_else(|_| text.to_string())
}

/// Creates `dir` (and parents) if absent, logging a warning on failure.
fn ensure_dir(dir: &Path) {
    if let Err(error) = fs::create_dir_all(dir) {
        warn!("failed to create directory '{}': {error}", dir.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Loads an `AppData` rooted at two fresh temp subdirectories.
    fn load_temp(root: &Path) -> (AppData, Vec<DataError>) {
        AppData::load_from(&root.join("data"), &root.join("catalogs"))
    }

    #[test]
    fn all_embedded_schemas_are_valid() {
        // Guards the `expect` in `build_datastore`.
        datastore::validate_schemas(SCHEMAS).expect("all embedded schemas valid");
    }

    #[test]
    fn load_seeds_singletons_on_a_fresh_dir() {
        let dir = tempdir().unwrap();
        let (data, errors) = load_temp(dir.path());

        assert!(errors.is_empty(), "unexpected load errors: {errors:#?}");
        assert!(dir.path().join("data").join(SETTINGS_FILE).exists());
        assert!(dir.path().join("data").join(STOCK_FILE).exists());

        let settings = data.settings().expect("settings loaded");
        assert!(settings.status.is_complete(), "{:?}", settings.status);
        let stock = data.stock().expect("stock loaded");
        assert!(stock.status.is_complete(), "{:?}", stock.status);
    }

    #[test]
    fn create_and_list_a_profile_writes_a_file() {
        let dir = tempdir().unwrap();
        let (mut data, _) = load_temp(dir.path());

        let id = data.create(Profile::Cnc).expect("create cnc");
        data.flush();

        let path = dir.path().join("data").join("cnc_profiles").join(format!("{id}.yaml"));
        assert!(path.exists(), "expected profile file at {}", path.display());

        let listed = data.list(Profile::Cnc);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].0, id);
    }

    #[test]
    fn machining_profile_is_creatable_and_loadable() {
        // Guards the machining.yaml fix (previously unsatisfiable: required
        // id/cnc/fixture/toolset were not defined as properties).
        let dir = tempdir().unwrap();
        let (mut data, _) = load_temp(dir.path());
        let id = data.create(Profile::Machining).expect("create machining");
        data.flush();
        let path = dir.path().join("data").join("processing_profiles").join(format!("{id}.yaml"));
        assert!(path.exists(), "expected machining file at {}", path.display());
        assert!(data.get(id).is_some(), "machining doc should be loaded");
    }

    #[test]
    fn machining_normalizes_legacy_files_and_edits_bindings() {
        // A legacy `processing_profiles` file: per-op `enabled` flags and an
        // empty-string fixture ref, neither of which is valid machining.yaml.
        let dir = tempdir().unwrap();
        let data_dir = dir.path().join("data");
        let proc_dir = data_dir.join("processing_profiles");
        fs::create_dir_all(&proc_dir).unwrap();
        let id = uuid::Uuid::now_v7();
        let cnc = uuid::Uuid::now_v7();
        let legacy = format!(
            "schema_version: 2\n\
             id: \"{id}\"\n\
             name: Legacy\n\
             side_to_machine: top\n\
             cnc: {{ default: \"{cnc}\", choices: [\"{cnc}\"] }}\n\
             fixture: {{ default: '', choices: [''] }}\n\
             toolset: {{ default: '', choices: [''] }}\n\
             operations: [drill_pth]\n\
             routing: {{ cut_depth_strategy: automatic, multi_pass_max_depth: 1mm }}\n\
             drill_locating_pins: {{ enabled: false }}\n\
             drill_pth: {{ enabled: true, holes: {{ route_fallback: false, drill_first: true, pilot: false, oblong: drill_ends_then_route, oversize: {{ relative: 8%, max: 0.20mm }}, undersize: {{ relative: 8%, max: 0.20mm }} }} }}\n\
             drill_npth: {{ enabled: false }}\n\
             route_board: {{ enabled: false }}\n\
             mill_board: {{ enabled: false }}\n"
        );
        fs::write(proc_dir.join(format!("{id}.yaml")), legacy).unwrap();

        let (mut data, _errors) = AppData::load_from(&data_dir, &dir.path().join("catalogs"));

        // The legacy file loaded as a machining doc: `enabled` gone, empty fixture
        // ref dropped (so it is absent, not an invalid UUID).
        let doc = data.get(id).expect("legacy machining loaded");
        assert!(doc.root.get_pointer("/drill_pth/enabled").is_none(), "enabled should be stripped");
        assert!(doc.root.get_pointer("/fixture/default").is_none(), "empty ref should be dropped");
        assert!(doc.root.get_pointer("/cnc/default").is_some(), "real ref preserved");

        // Structural edits round-trip: set a fixture binding and change operations.
        let fixture = uuid::Uuid::now_v7();
        assert!(data.set_machining_binding(id, "fixture", Some(fixture), &[fixture]));
        assert!(data.set_machining_operations(id, &["drill_pth".to_string(), "route_board".to_string()]));

        let doc = data.get(id).unwrap();
        let fixture_default = doc.root.get_pointer("/fixture/default").expect("fixture set");
        assert!(matches!(&fixture_default.value, NodeValue::Ref(r) if r.raw == fixture));
        let ops = doc.root.get_pointer("/operations").unwrap();
        assert!(matches!(&ops.value, NodeValue::Array(a) if a.len() == 2));
    }

    #[test]
    fn stock_items_add_edit_and_remove() {
        let dir = tempdir().unwrap();
        let (mut data, _) = load_temp(dir.path());

        let count = |data: &AppData| {
            data.stock()
                .and_then(|doc| doc.root.get_pointer("/tools"))
                .map(|node| match &node.value {
                    NodeValue::Array(items) => items.len(),
                    _ => 0,
                })
                .unwrap_or(0)
        };

        let first = data.add_stock_item().expect("add first");
        data.add_stock_item().expect("add second");
        assert_eq!(count(&data), 2);

        // Edit an enum field on the first item.
        assert_eq!(
            data.set_stock_str(&format!("/tools/{first}/availability"), "out_of_stock"),
            Some(true)
        );
        let availability = data
            .stock()
            .unwrap()
            .root
            .get_pointer(&format!("/tools/{first}/availability"))
            .unwrap();
        assert!(matches!(&availability.value, NodeValue::Str(s) if s == "out_of_stock"));

        // Remove it.
        assert!(data.remove_stock_item(first));
        assert_eq!(count(&data), 1);
        assert!(!data.remove_stock_item(99), "out-of-range remove is a no-op");
    }

    #[test]
    fn stock_replace_from_value_persists_and_reloads() {
        // Mirrors AppState::persist_stock, the sole writer of stock.yaml: the real
        // `stock_value_from_tools` projection (unit fields as the canonical strings
        // `Length`/`FeedRate`/etc. serialize to, plus the mandatory schema_version)
        // is pushed through `replace_stock_from_value`. Proves the projection
        // re-parses (units + enums decode) and the written file reloads cleanly.
        use crate::data::model::stock::{stock_value_from_tools, Tool, ToolPreference, ToolStatus};
        use units::{Angle, FeedRate, Length, RotationalSpeed};

        let dir = tempdir().unwrap();
        let (mut data, _) = load_temp(dir.path());

        let tools = vec![
            Tool {
                id: uuid::Uuid::now_v7().to_string(),
                composite_name: "Router 1.5mm".into(),
                name: String::new(),
                kind: "Router".into(),
                diameter: Length::from_mm(1.5),
                catalog_diameter: Some(Length::from_mm(1.5)),
                point_angle: Angle::from_degrees(118.0),
                catalog_point_angle: Some(Angle::from_degrees(118.0)),
                feed_rate: Some(FeedRate::from_mm_per_min(1200.0)),
                catalog_feed_rate: Some(FeedRate::from_mm_per_min(1200.0)),
                spindle_speed: Some(RotationalSpeed::from_rpm(12000.0)),
                catalog_spindle_speed: Some(RotationalSpeed::from_rpm(12000.0)),
                status: ToolStatus::OutOfStock,
                preference: ToolPreference::Preferred,
                source_catalog: "Manual".into(),
                manufacturer: None,
                sku: None,
            },
            Tool {
                id: uuid::Uuid::now_v7().to_string(),
                composite_name: "Drill 0.8mm".into(),
                name: String::new(),
                kind: "Drill".into(),
                diameter: Length::from_mm(0.8),
                catalog_diameter: Some(Length::from_mm(0.8)),
                point_angle: Angle::from_degrees(118.0),
                catalog_point_angle: Some(Angle::from_degrees(118.0)),
                feed_rate: None,
                catalog_feed_rate: None,
                spindle_speed: None,
                catalog_spindle_speed: None,
                status: ToolStatus::InStock,
                preference: ToolPreference::Neutral,
                source_catalog: "Manual".into(),
                manufacturer: None,
                sku: None,
            },
        ];

        let value = stock_value_from_tools(&tools);
        assert_eq!(value.get("schema_version"), Some(&Value::from(1)), "projection must carry schema_version");

        let problems = data.replace_stock_from_value(&value).expect("stock singleton loaded");
        assert!(problems.is_empty(), "unexpected parse problems: {problems:#?}");

        // Two tools; the unit and enum fields decoded from their string forms.
        let stock = data.stock().unwrap();
        assert!(matches!(&stock.root.get_pointer("/tools").unwrap().value, NodeValue::Array(a) if a.len() == 2));
        let diameter = stock.root.get_pointer("/tools/0/base/diameter").unwrap();
        assert!(matches!(&diameter.value, NodeValue::Unit(_)), "diameter should decode to a unit: {:?}", diameter.value);
        let availability = stock.root.get_pointer("/tools/0/availability").unwrap();
        assert!(matches!(&availability.value, NodeValue::Str(s) if s == "out_of_stock"));

        // The sole-writer output is a valid, reloadable file.
        data.flush();
        let (reloaded, errors) = load_temp(dir.path());
        assert!(errors.is_empty(), "reload errors: {errors:#?}");
        let reloaded_tools = reloaded.stock().unwrap().root.get_pointer("/tools").unwrap();
        assert!(
            matches!(&reloaded_tools.value, NodeValue::Array(a) if a.len() == 2),
            "two tools should survive the persist + reload round trip"
        );
    }

    #[test]
    fn stock_append_and_remove_by_id_edit_the_tool_list() {
        // The catalog picker's append path and the bulk-delete path, both
        // value-level over the sole writer.
        let dir = tempdir().unwrap();
        let (mut data, _) = load_temp(dir.path());

        let id_a = uuid::Uuid::now_v7().to_string();
        let id_b = uuid::Uuid::now_v7().to_string();
        let tool = |id: &str| {
            serde_json::json!({
                "id": id,
                "availability": "in_stock",
                "preference": "neutral",
                "ref": { "catalog": "Manual", "tool_id": id },
                "base": { "name": "Router 2mm", "kind": "routerbit", "diameter": "2mm" }
            })
        };

        let count = |data: &AppData| {
            data.stock()
                .and_then(|doc| doc.root.get_pointer("/tools"))
                .map(|node| match &node.value {
                    NodeValue::Array(items) => items.len(),
                    _ => 0,
                })
                .unwrap_or(0)
        };

        assert_eq!(data.append_stock_tool_values(&[tool(&id_a), tool(&id_b)]), 2);
        assert_eq!(count(&data), 2);
        // `order` is renumbered monotonically from the existing length.
        let order = data.stock().unwrap().root.get_pointer("/tools/1/order").unwrap();
        assert!(matches!(&order.value, NodeValue::Int(1)));

        // Remove by id (unknown ids are ignored).
        assert_eq!(data.remove_stock_tools_by_ids(&[id_a.clone(), "nope".to_string()]), 1);
        assert_eq!(count(&data), 1);
        let remaining = data.stock().unwrap().root.get_pointer("/tools/0/id").unwrap();
        assert!(matches!(&remaining.value, NodeValue::Id(id) if id.to_string() == id_b));

        assert_eq!(data.remove_stock_tools_by_ids(&[]), 0, "empty removal is a no-op");
    }

    #[test]
    fn stock_load_injects_schema_version_into_a_legacy_file() {
        // A stock.yaml written by the retired legacy `save_stock` has no
        // `schema_version`; load must still parse it (else there is no stock doc
        // for the sole writer to edit, and stock could never persist again).
        let dir = tempdir().unwrap();
        let data_dir = dir.path().join("data");
        fs::create_dir_all(&data_dir).unwrap();
        let id = uuid::Uuid::now_v7();
        let legacy = format!(
            "tools:\n\
             - id: \"{id}\"\n\
             \x20 availability: in_stock\n\
             \x20 preference: neutral\n\
             \x20 ref: {{ catalog: Manual, tool_id: \"{id}\" }}\n\
             \x20 base: {{ name: \"Router 2mm\", kind: routerbit, diameter: 2mm }}\n"
        );
        fs::write(data_dir.join(STOCK_FILE), legacy).unwrap();

        let (data, errors) = AppData::load_from(&data_dir, &dir.path().join("catalogs"));
        assert!(errors.is_empty(), "legacy stock should load without errors: {errors:#?}");
        let stock = data.stock().expect("legacy stock loaded");
        assert!(matches!(&stock.root.get_pointer("/tools").unwrap().value, NodeValue::Array(a) if a.len() == 1));
        assert!(
            matches!(stock.root.get_pointer("/schema_version").map(|n| &n.value), Some(NodeValue::Int(1))),
            "schema_version should have been injected"
        );
    }

    #[test]
    fn toolset_create_names_and_edits_the_rack() {
        // `name`/`slots` defaults let a fresh toolset be named and its rack grown.
        let dir = tempdir().unwrap();
        let (mut data, _) = load_temp(dir.path());
        let id = data.create(Profile::Toolset).expect("create toolset");

        // The name node exists (default), so it can be set.
        assert_eq!(data.set_field(id, "/name", NodeValue::Str("Rack A".into())), Some(true));

        // The rack seeds one slot and can be grown, and a slot can go fixed.
        assert!(data.set_toolset_slot_count(id, 3));
        let tool = Uuid::now_v7();
        assert!(data.set_toolset_slot_mode(id, 0, "fixed", Some(tool)));

        let doc = data.get(id).unwrap();
        assert!(matches!(&doc.root.get_pointer("/name").unwrap().value, NodeValue::Str(s) if s == "Rack A"));
        let slots = doc.root.get_pointer("/slots").unwrap();
        assert!(matches!(&slots.value, NodeValue::Array(a) if a.len() == 3));
        assert!(matches!(&doc.root.get_pointer("/slots/0/mode").unwrap().value, NodeValue::Str(s) if s == "fixed"));
        assert!(doc.root.get_pointer("/slots/0/tool_id").is_some());

        // Switching a fixed slot back to spare drops its tool_id (schema rule).
        assert!(data.set_toolset_slot_mode(id, 0, "spare", None));
        assert!(data.get(id).unwrap().root.get_pointer("/slots/0/tool_id").is_none());
    }

    #[test]
    fn create_cnc_from_template_preserves_the_template_name() {
        let dir = tempdir().unwrap();
        let (mut data, _) = load_temp(dir.path());

        let id = data.create_cnc_from_template("genmitsu_3018").expect("create from template");
        data.flush();

        let doc = data.get(id).expect("profile present");
        assert!(doc.status.is_complete(), "{:?}", doc.status);
        let name = doc.root.get_pointer("/name").unwrap();
        assert!(matches!(&name.value, NodeValue::Str(s) if s == "Genmitsu 3018-Pro"));
    }

    #[test]
    fn unknown_template_key_is_an_error() {
        let dir = tempdir().unwrap();
        let (mut data, _) = load_temp(dir.path());
        assert!(data.create_cnc_from_template("does_not_exist").is_err());
    }

    #[test]
    fn cnc_templates_lists_all_bundled_seeds() {
        let dir = tempdir().unwrap();
        let (data, _) = load_temp(dir.path());
        let templates = data.cnc_templates();
        assert_eq!(templates.len(), CNC_TEMPLATES.len());
        assert!(templates.iter().any(|t| t.name == "Masso G3 - With ATC"));
    }

    #[test]
    fn set_setting_persists_across_reload() {
        let dir = tempdir().unwrap();
        {
            let (mut data, _) = load_temp(dir.path());
            let existed = data
                .set_setting("/theme", NodeValue::Str("Dark".to_string()))
                .expect("settings loaded");
            assert!(existed, "theme field should exist");
            data.flush();
        }

        let (data, _) = load_temp(dir.path());
        let theme = data.settings().unwrap().root.get_pointer("/theme").unwrap();
        assert!(matches!(&theme.value, NodeValue::Str(s) if s == "Dark"));
    }

    #[test]
    fn settings_replace_from_value_persists_and_reloads() {
        // The runtime mirrors its whole settings snapshot down through
        // replace_settings_from_value — the sole-writer bridge for global settings.
        // Exercises a real UUID stored in a `[string, null]` selection field.
        let dir = tempdir().unwrap();
        let pid = uuid::Uuid::now_v7().to_string();
        {
            let (mut data, _) = load_temp(dir.path());
            let payload = serde_json::json!({
                "schema_version": 1,
                "units": "in",
                "theme": "Dark",
                "selected_process_profile_id": pid,
                "selected_cnc_profile_id": Value::Null,
                "selected_fixture_profile_id": Value::Null,
                "selected_toolset_profile_id": Value::Null,
            });
            let problems = data
                .replace_settings_from_value(&payload)
                .expect("settings loaded");
            assert!(problems.is_empty(), "settings replace should not error: {problems:#?}");
            data.flush();
        }

        let (data, errors) = load_temp(dir.path());
        assert!(errors.is_empty(), "reload should be clean: {errors:#?}");
        let settings = data.settings().expect("settings reloaded");
        assert!(matches!(&settings.root.get_pointer("/units").unwrap().value, NodeValue::Str(s) if s == "in"));
        assert!(matches!(&settings.root.get_pointer("/theme").unwrap().value, NodeValue::Str(s) if s == "Dark"));
        assert!(
            matches!(&settings.root.get_pointer("/selected_process_profile_id").unwrap().value, NodeValue::Str(s) if *s == pid),
            "a selected-profile UUID must round-trip through the [string, null] field"
        );
    }

    #[test]
    fn settings_load_injects_schema_version_into_a_legacy_file() {
        // A global.setting.yaml written by the retired legacy save_global_settings
        // has no schema_version; load must still parse it so AppData can adopt and
        // rewrite the settings singleton (else settings could never persist again).
        let dir = tempdir().unwrap();
        let data_dir = dir.path().join("data");
        fs::create_dir_all(&data_dir).unwrap();
        let legacy = "units: in\n\
                      theme: Dark\n\
                      selected_process_profile_id: null\n\
                      selected_cnc_profile_id: null\n\
                      selected_fixture_profile_id: null\n\
                      selected_toolset_profile_id: null\n";
        fs::write(data_dir.join(SETTINGS_FILE), legacy).unwrap();

        let (data, errors) = AppData::load_from(&data_dir, &dir.path().join("catalogs"));
        assert!(errors.is_empty(), "legacy settings should load without errors: {errors:#?}");
        let settings = data.settings().expect("legacy settings loaded");
        assert!(matches!(&settings.root.get_pointer("/theme").unwrap().value, NodeValue::Str(s) if s == "Dark"));
        assert!(
            matches!(settings.root.get_pointer("/schema_version").map(|n| &n.value), Some(NodeValue::Int(1))),
            "schema_version should have been injected"
        );
    }

    #[test]
    fn remove_unreferenced_profile_succeeds() {
        let dir = tempdir().unwrap();
        let (mut data, _) = load_temp(dir.path());
        let id = data.create(Profile::Fixture).expect("create fixture");
        assert!(data.remove(id).is_ok());
        assert!(data.list(Profile::Fixture).is_empty());
    }
}

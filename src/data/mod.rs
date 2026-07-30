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
    DataError, DataStore, Document, FactoryError, NodeValue, ParseInput,
    RemoveError, ResolvedStore,
};
use log::warn;
use serde_json::Value;
use uuid::Uuid;

use crate::data::model::{EdgeTab, MACHINING_OPERATIONS};
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
    ("job.yaml", include_str!("../../schemas/job.yaml")),
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
const JOB_FILE: &str = "job.yaml";

/// The four id'd, per-file profile collections. The single live **Job** is not
/// here — it is a singleton (`job.yaml`, one per install, no id/name), handled
/// alongside settings/stock.
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
    job_path: PathBuf,
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
        let job_path = data_dir.join(JOB_FILE);

        // Seed the singletons if absent, then parse them as the initial docs.
        seed_singleton_if_missing(&schemas, "settings.yaml", &settings_path);
        seed_singleton_if_missing(&schemas, "stock.yaml", &stock_path);
        seed_singleton_if_missing(&schemas, "job.yaml", &job_path);

        // A legacy `global.setting.yaml`/`stock.yaml` (written before the
        // datastore's `x-schema-version` gating, by the retired
        // `save_global_settings`/`save_stock`) lacks the `schema_version` field the
        // gate now requires; inject it on load so such files still parse (and are
        // then rewritten in modern form by the AppData writer).
        let settings_text = fs::read_to_string(&settings_path).ok().map(|text| inject_schema_version(&text));
        let stock_text = fs::read_to_string(&stock_path).ok().map(|text| {
            materialize_stock_overrides(&migrate_stock_ref(&inject_schema_version(&text)))
        });
        let job_text = fs::read_to_string(&job_path).ok().map(|text| inject_schema_version(&text));
        let mut inputs = Vec::new();
        if let Some(text) = &settings_text {
            inputs.push(ParseInput { schema_id: "settings.yaml", source: Some(settings_path.clone()), text });
        }
        if let Some(text) = &stock_text {
            inputs.push(ParseInput { schema_id: "stock.yaml", source: Some(stock_path.clone()), text });
        }
        if let Some(text) = &job_text {
            inputs.push(ParseInput { schema_id: "job.yaml", source: Some(job_path.clone()), text });
        }
        let outcome = schemas.parse(&inputs);
        errors.extend(outcome.errors);
        let mut store = schemas.resolve(outcome.documents);

        // Load each profile collection (registers its directory for new files).
        // Machining files on disk carry a legacy shape (per-op `enabled` flags,
        // empty-string refs); they are normalized before parsing.
        for profile in Profile::ALL {
            let dir = data_dir.join(profile.dir_name());
            let normalize = match profile {
                Profile::Machining => Some(normalize_machining_file as fn(&mut Value, &Path)),
                Profile::Cnc => Some(normalize_cnc_value as fn(&mut Value, &Path)),
                Profile::Fixture => Some(normalize_fixture_value as fn(&mut Value, &Path)),
                Profile::Toolset => None,
            };
            match normalize {
                Some(normalize) => {
                    errors.extend(load_normalized(&mut store, profile.schema_id(), &dir, normalize))
                }
                None => errors.extend(store.parse_directory(profile.schema_id(), &dir)),
            }
        }

        // Load the read-only catalog collection last, so stock references
        // resolve against catalog tools on the final pass.
        errors.extend(store.parse_directory("catalog.yaml", catalogs_dir));

        let cnc_templates = load_cnc_templates();

        (
            Self { store, cnc_templates, settings_path, stock_path, job_path },
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

    /// Removes a profile and deletes its file, unless something still references
    /// it (then [`RemoveError::InUse`] names the referrers).
    pub fn remove(&mut self, id: Uuid) -> Result<(), RemoveError> {
        self.store.remove_document(id)
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

    // ---- machining step structural edits ---------------------------------
    //
    // A machining profile is an ordered `steps` array; each step has structural
    // fields the fine-grained setters can't express: the cnc/fixture/toolset
    // references and the `operations` array, plus add/remove/reorder of steps
    // themselves. These edit the plain document value and re-parse it (see
    // [`ResolvedStore::replace_document_from_value`]).

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

    /// Sets a **step's** profile reference for `field` (`"cnc"`, `"fixture"`, or
    /// `"toolset"`).
    ///
    /// `None` **removes** the key rather than writing an empty string: absent is how the
    /// schema spells "no profile chosen", and an empty string would fail the `uuid_v7`
    /// pattern on the next load.
    pub fn set_step_reference(
        &mut self,
        id: Uuid,
        step: usize,
        field: &str,
        target: Option<Uuid>,
    ) -> bool {
        let field = field.to_string();
        self.edit_document_value(id, |value| {
            let Some(step_obj) = value
                .pointer_mut(&format!("/steps/{step}"))
                .and_then(Value::as_object_mut)
            else {
                return;
            };
            match target {
                Some(uuid) => {
                    step_obj.insert(field, Value::String(uuid.to_string()));
                }
                None => {
                    step_obj.remove(&field);
                }
            }
        })
    }

    /// Sets a **step's** enabled `operations`, in order. Each entry is an
    /// operation key (e.g. `"drill_pth"`). The per-operation config objects are
    /// always present (schema defaults); this only changes what is *enabled*.
    pub fn set_step_operations(&mut self, id: Uuid, step: usize, operations: &[String]) -> bool {
        self.edit_document_value(id, |value| {
            let Some(step_obj) = value
                .pointer_mut(&format!("/steps/{step}"))
                .and_then(Value::as_object_mut)
            else {
                return;
            };
            step_obj.insert(
                "operations".into(),
                Value::Array(operations.iter().map(|s| Value::String(s.clone())).collect()),
            );
        })
    }

    /// Appends a fresh default step with one operation (op-config objects materialize
    /// on re-parse). Bindings are left absent until the user picks them.
    ///
    /// The operation is the first the new step is actually allowed to run: most of them
    /// may only be claimed by one step per board side, so defaulting to `drill_pth`
    /// unconditionally would make "+ Add step" produce a profile that cannot generate.
    /// The new step machines the top side (the schema default), so only top-side steps
    /// are counted against it.
    pub fn add_step(&mut self, id: Uuid) -> bool {
        self.edit_document_value(id, |value| {
            let Some(steps) = value.get_mut("steps").and_then(Value::as_array_mut) else {
                return;
            };

            let claimed: Vec<&str> = steps
                .iter()
                .filter(|step| {
                    // Absent means top, which is the schema default for a step that has
                    // never had the field written.
                    step.get("side_to_machine").and_then(Value::as_str).unwrap_or("top") != "bottom"
                })
                .filter_map(|step| step.get("operations").and_then(Value::as_array))
                .flatten()
                .filter_map(Value::as_str)
                .collect();

            let operation = MACHINING_OPERATIONS
                .iter()
                .find(|op| !op.once_per_side || !claimed.contains(&op.key))
                // Unreachable while any operation is repeatable, but a step must carry
                // at least one (`minItems: 1`), and a step the gate then complains about
                // is better than one the schema rejects.
                .map_or(MACHINING_OPERATIONS[0].key, |op| op.key);

            steps.push(serde_json::json!({ "name": "Machining step", "operations": [operation] }));
        })
    }

    /// Removes the step at `step`. A machining profile keeps at least one step, so
    /// removing the last one is a no-op.
    pub fn remove_step(&mut self, id: Uuid, step: usize) -> bool {
        self.edit_document_value(id, |value| {
            let Some(steps) = value.get_mut("steps").and_then(Value::as_array_mut) else {
                return;
            };
            if steps.len() > 1 && step < steps.len() {
                steps.remove(step);
            }
        })
    }

    /// Reorders a step from index `from` to index `to`. Out-of-range or `from ==
    /// to` is a no-op.
    pub fn move_step(&mut self, id: Uuid, from: usize, to: usize) -> bool {
        self.edit_document_value(id, |value| {
            let Some(steps) = value.get_mut("steps").and_then(Value::as_array_mut) else {
                return;
            };
            if from < steps.len() && to < steps.len() && from != to {
                let item = steps.remove(from);
                steps.insert(to, item);
            }
        })
    }

    /// Back-compat shim for the still-single-step machining UI: edits the first step's
    /// operations. Removed once the UI drives steps by index (Stage 3).
    pub fn set_machining_operations(&mut self, id: Uuid, operations: &[String]) -> bool {
        self.set_step_operations(id, 0, operations)
    }

    // ---- job singleton ----------------------------------------------------
    //
    // The Job is the single live thing being processed: one `job.yaml` per
    // install (no id/name), referencing the machining profile it runs.

    /// The live job document (singleton), if loaded.
    pub fn job(&self) -> Option<&Document> {
        self.singleton("job.yaml")
    }

    /// The machining profile the live job references, if set.
    pub fn job_machining_profile(&self) -> Option<Uuid> {
        self.job()
            .and_then(|doc| doc.root.get_pointer("/machining_profile"))
            .and_then(|node| match &node.value {
                NodeValue::Ref(reference) => Some(reference.raw),
                NodeValue::Id(id) => Some(*id),
                NodeValue::Str(s) => Uuid::parse_str(s).ok(),
                _ => None,
            })
    }

    /// Sets (or clears with `None`) the live job's `machining_profile`, re-parsing
    /// so the UUID string decodes to a resolved reference (the datastore rejects
    /// setting a ref from a raw string via `set_str`, so this goes value-level).
    pub fn set_job_machining_profile(&mut self, target: Option<Uuid>) -> bool {
        let path = self.job_path.clone();
        let Some(mut value) = self.job().map(|doc| doc.to_value()) else {
            return false;
        };
        if let Some(obj) = value.as_object_mut() {
            match target {
                Some(uuid) => {
                    obj.insert("machining_profile".into(), Value::String(uuid.to_string()));
                }
                None => {
                    obj.remove("machining_profile");
                }
            }
        }
        self.store.replace_document_from_value_at(&path, &value).is_some()
    }

    /// The board orientation angle (degrees) the live job stores. Absent/legacy
    /// job files default to 0 (the schema default), so this never fails.
    pub fn job_board_orientation(&self) -> i32 {
        self.job()
            .map(|doc| doc.to_value())
            .and_then(|value| value.get("board_orientation").and_then(Value::as_i64))
            .map(|angle| angle as i32)
            .unwrap_or(0)
    }

    /// Sets the live job's `board_orientation` (degrees). Goes value-level and
    /// re-parses so the datastore re-validates the angle against the schema.
    pub fn set_job_board_orientation(&mut self, angle: i32) -> bool {
        let path = self.job_path.clone();
        let Some(mut value) = self.job().map(|doc| doc.to_value()) else {
            return false;
        };
        if let Some(obj) = value.as_object_mut() {
            obj.insert("board_orientation".into(), Value::from(angle));
        }
        self.store.replace_document_from_value_at(&path, &value).is_some()
    }

    /// The live job's retaining-tab placements, in file order. Empty — the schema
    /// default, and what a legacy job file yields — means "place them automatically
    /// from the profile's tab count" (see `job.yaml`).
    pub fn job_edge_tabs(&self) -> Vec<EdgeTab> {
        self.job()
            .map(|doc| doc.to_value())
            .and_then(|value| value.get("edge_tabs").and_then(Value::as_array).cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(EdgeTab::from_value)
            .collect()
    }

    /// Replaces the live job's tab placements wholesale.
    ///
    /// Wholesale rather than add/remove because the board view edits the set as a
    /// whole and the list is short; it also keeps the persisted order equal to the
    /// order shown. Re-parses, so the datastore re-validates each entry.
    pub fn set_job_edge_tabs(&mut self, tabs: &[EdgeTab]) -> bool {
        let path = self.job_path.clone();
        let Some(mut value) = self.job().map(|doc| doc.to_value()) else {
            return false;
        };
        if let Some(obj) = value.as_object_mut() {
            obj.insert(
                "edge_tabs".into(),
                Value::Array(tabs.iter().copied().map(EdgeTab::to_value).collect()),
            );
        }
        self.store.replace_document_from_value_at(&path, &value).is_some()
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

/// Blocks until every scheduled write of the global store has landed on disk.
///
/// Needed at shutdown: the store lives in a `static` that is never dropped, so the
/// writer thread's own drain-on-drop never runs and a write queued moments before exit
/// would die with the process. A no-op when the store was never initialized.
pub fn flush_appdata() {
    if appdata_ready() {
        with_appdata(|data| data.flush());
    }
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

/// Loads a profile collection from `dir`, running each on-disk file through
/// `normalize` before parsing, and registers `dir` as that collection so new/edited
/// files land there.
///
/// The normalisers exist because on-disk profiles outlive their schema: a file written
/// by an older build carries a shape the current schema would reject (or, worse, quietly
/// accept as wrong). Fixing it here — once, on the way in — is what keeps the rest of the
/// app from having to know about historical shapes.
fn load_normalized(
    store: &mut ResolvedStore,
    schema_id: &str,
    dir: &Path,
    normalize: fn(&mut Value, &Path),
) -> Vec<DataError> {
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
            normalize(&mut value, &path);
            if let Ok(normalized) = serde_json::to_string(&value) {
                items.push((path, normalized));
            }
        }
    }
    store.parse_texts(schema_id, dir, &items)
}

/// The axis feed limit shipped as the schema default, for backfilling profiles written
/// before `machine.max_feed_xy`/`max_feed_z` existed. Must match `schemas/cnc.yaml`.
const MAX_FEED_DEFAULT: &str = "5000mm/min";

/// The program extension shipped as the schema default, for the same reason.
const OUTPUT_EXTENSION_DEFAULT: &str = "nc";

/// Whether a motion template emits a feed word at all: either through the `{feedrate}`
/// variable or as a hardcoded `F<number>`.
///
/// The hardcoded case matters — a profile that pins its own feed is unusual but valid,
/// and rewriting it would silently change how that machine cuts.
fn emits_a_feed(template: &str) -> bool {
    template.contains("{feedrate}")
        || template
            .as_bytes()
            .windows(2)
            .any(|w| w[0] == b'F' && (w[1].is_ascii_digit() || w[1] == b'.'))
}

/// Whether `text` contains `word` as a whole word.
///
/// Whole-word so `G54` inside `G540`, `XG54` or `G54.1` does not count. Machine words
/// are delimited by whitespace or line ends in every dialect this touches, which is what
/// makes the rule safe to apply to a template we did not write.
fn contains_word(text: &str, word: &str) -> bool {
    let is_boundary = |c: Option<char>| c.is_none_or(|c| !c.is_ascii_alphanumeric() && c != '.');
    let mut rest = text;

    while let Some(hit) = rest.find(word) {
        let before = rest[..hit].chars().next_back();
        let after = rest[hit + word.len()..].chars().next();
        if is_boundary(before) && is_boundary(after) {
            return true;
        }
        rest = &rest[hit + word.len()..];
    }
    false
}

/// Whether a template emits a spindle-speed word — an `S` followed by the `{rpm}`
/// variable or by a literal number.
fn sets_spindle_speed(template: &str) -> bool {
    template.contains("S{rpm}")
        || template
            .as_bytes()
            .windows(2)
            .any(|w| w[0] == b'S' && w[1].is_ascii_digit())
}

/// Removes a trailing emit line that does nothing but set the spindle speed.
///
/// Only the *last* emit line, and only when it is nothing else: a line like
/// `` `T{slot} M06 S{rpm} `` still does the tool change, so it is left alone and the
/// duplicate accepted rather than risk mangling a hand-written template.
fn drop_trailing_speed_line(template: &str) -> String {
    let mut lines: Vec<&str> = template.lines().collect();
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    let is_speed_only = lines.last().is_some_and(|line| {
        let body = line.trim().trim_start_matches('`').trim();
        body.starts_with('S') && !body[1..].contains(char::is_whitespace) && sets_spindle_speed(body)
    });
    if is_speed_only {
        lines.pop();
    }
    let mut out = lines.join("\n");
    // Preserve the trailing newline of a YAML block scalar, so the round trip is clean.
    if template.ends_with('\n') && !out.is_empty() {
        out.push('\n');
    }
    out
}

/// Repairs a CNC profile written before `linear_cut` carried a feed.
///
/// The old shipped default was `` `G1 X{x} Y{y} Z{z} S{s} `` — a G1 with **no F**, which
/// runs at whatever feed happens to be modal. After a drill block that is the drill's
/// plunge feed, and a router driven at a drill's plunge feed breaks. A saved profile that
/// cannot emit a feed is therefore repaired rather than merely flagged; one that already
/// emits a feed (variable or hardcoded) is left exactly as the operator wrote it.
fn normalize_cnc_value(value: &mut Value, path: &Path) {
    let file = path.file_name().and_then(|n| n.to_str()).unwrap_or("cnc.yaml").to_string();

    // The axis feed limits became required after profiles were already in the field.
    // Backfilled with the schema's own conservative default rather than merely reported:
    // the parser would fill the same value in memory anyway, so leaving the document
    // without them buys nothing and warns on every launch until the profile is next
    // edited. 5000 mm/min is deliberately slow — a machine that is faster says so in its
    // specification, and the operator raises it.
    if let Some(machine) = value.pointer_mut("/machine").and_then(Value::as_object_mut) {
        for key in ["max_feed_xy", "max_feed_z"] {
            if !machine.contains_key(key) {
                warn!("[{file}] machine.{key} was missing; filled in at {MAX_FEED_DEFAULT}");
                machine.insert(key.into(), Value::from(MAX_FEED_DEFAULT));
            }
        }
        // Likewise required-with-a-default. Silent rather than warned: every profile
        // that predates the key was a G-code machine, so the default is not an
        // assumption about the hardware the way a feed limit is.
        if !machine.contains_key("output_file_extension") {
            machine.insert("output_file_extension".into(), Value::from(OUTPUT_EXTENSION_DEFAULT));
        }
    }

    // The three checks below **report** and do not rewrite. Each was once a repair that
    // wrote a replacement template, and every one of those replacements was machine code
    // this application had chosen — a G1 line, a G2/G3 pair, a G-word expression. That is
    // exactly what the primitives exist to keep out of here: the profile owns the machine
    // language, so a profile that needs changing is told, in terms of the variable or
    // behaviour at fault, and the operator makes the edit in the primitive editor.
    //
    // Reporting is also the only correct behaviour for a profile that is not G-code at
    // all: an Excellon `linear_cut` legitimately carries no `F` word, and "repairing" it
    // into a G1 line would have destroyed it.

    // `linear_cut` with no feed at all. A G1 with no F runs at whatever feed is modal —
    // after a drill block that is the drill's plunge feed, and a router driven at a
    // drill's plunge feed breaks. Worth saying loudly.
    if let Some(template) = value.pointer("/primitives/linear_cut").and_then(Value::as_str) {
        if !emits_a_feed(template) {
            warn!(
                "[{file}] primitives.linear_cut emits no feed rate ('{template}'), so a \
                 routing move will run at whatever feed is left over from the previous \
                 block — a drill's plunge feed, which breaks routers. Add the feed \
                 variable to the template."
            );
        }
    }

    // The stored zero-point count is gone: a fixture no longer holds an ordinal to index
    // into it, and `set_origin` decides what this controller accepts. Dropped quietly —
    // a count carried no operator decision that could be lost with it.
    if let Some(machine) = value.get_mut("machine").and_then(Value::as_object_mut) {
        machine.remove("work_coordinate_systems");
    }

    // `initialise` naming a work coordinate system outright overrides whatever the step's
    // fixture says it is set up in — the one thing a fixture most needs to be able to
    // say. `set_origin()` emits the fixture's own reference, validated.
    //
    // Only a whole-word `G54` is looked for, because that was this application's own
    // shipped default. A profile naming a different system (a Bantam's `G55`, say) is
    // doing so because its machine reserves the lower ones.
    if let Some(template) = value.pointer("/primitives/initialise").and_then(Value::as_str) {
        if contains_word(template, "G54") {
            warn!(
                "[{file}] primitives.initialise selects G54 outright, ignoring the \
                 fixture's Machine Origin Reference. Call `set_origin();` there instead — \
                 it emits the fixture's own offset and refuses one this machine does not \
                 have."
            );
        }
    }

    // `change_tool` used to end with `S{rpm}`, and `start_spindle` opens with the same
    // word — and the Coder always emits the two adjacent, so such a program carries two
    // identical S words in a row. Harmless, and the operator's line to delete: editing
    // someone's tool-change template on their behalf means deciding what a spindle-speed
    // line looks like on their machine.
    let spindle_sets_speed = value
        .pointer("/primitives/start_spindle")
        .and_then(Value::as_str)
        .is_some_and(sets_spindle_speed);
    if spindle_sets_speed {
        if let Some(template) = value.pointer("/primitives/change_tool").and_then(Value::as_str) {
            if drop_trailing_speed_line(template) != template {
                warn!(
                    "[{file}] primitives.change_tool ends by setting the spindle speed \
                     that start_spindle sets immediately afterwards, so every tool change \
                     emits it twice. Drop the trailing line if your controller does not \
                     want it."
                );
            }
        }
    }

    // `machine.line_numbering_increment` was a bare integer the application turned into
    // an "N<n> " prefix itself. Numbering is now a template, so the field is retired —
    // and it must be *removed* here, or `additionalProperties: false` rejects the whole
    // profile. Dropping a dead key is not the same as authoring machine code, so that
    // part stays; what does **not** happen any more is seeding a `line_number` template
    // from it, because "N" is a G-code word and this profile may not be G-code.
    //
    // The cost is honest and stated: a profile upgraded from the retired field stops
    // numbering until its operator writes the template the warning describes.
    let increment = value
        .pointer_mut("/machine")
        .and_then(Value::as_object_mut)
        .and_then(|machine| machine.remove("line_numbering_increment"))
        .and_then(|v| v.as_u64());
    if let Some(increment) = increment {
        let already_set = value
            .pointer("/primitives/line_number")
            .and_then(Value::as_str)
            .is_some_and(|t| !t.trim().is_empty());
        if !already_set && increment != 0 {
            warn!(
                "[{file}] machine.line_numbering_increment ({increment}) is retired and \
                 this profile has no primitives.line_number, so its programs are no \
                 longer numbered. Put the numbering word your controller expects in that \
                 field, stepping by {increment} — `line` counts the program's lines."
            );
        }
    }
}

/// Fixture blocks that were declared in full but never shown, never read and never
/// implemented. Removed rather than left as furniture nobody could tell was inert; the
/// shape can come back when there is a decision about what it should be.
const RETIRED_FIXTURE_KEYS: &[&str] =
    &["locating_pins", "keep_out_zones", "occupancy", "probing_alignment"];

/// Brings a fixture file onto the current schema.
///
/// Three repairs, all of which `additionalProperties: false` and the tightened `origin`
/// enums would otherwise turn into a rejected profile on load:
///
/// - The [retired blocks](RETIRED_FIXTURE_KEYS) are dropped.
/// - `work_coordinate_system` (an integer ordinal) is dropped in favour of
///   `origin_reference` — see [`retire_work_coordinate_system`].
/// - `origin.x0`/`origin.y0` both used to accept all four of `left|right|front|back`,
///   which allowed `x0: front` and even `x0: left` with `y0: left` — combinations with
///   no meaning. X can only be zeroed on a left or right edge and Y on a front or back
///   one. Anything outside the new enums is corrected to the default rather than
///   rejected, because a nonsense value was the schema's fault, not the operator's.
fn normalize_fixture_value(value: &mut Value, path: &Path) {
    let file = path.file_name().and_then(|n| n.to_str()).unwrap_or("fixture.yaml").to_string();
    let Some(obj) = value.as_object_mut() else {
        return;
    };

    for key in RETIRED_FIXTURE_KEYS {
        if obj.remove(*key).is_some() {
            warn!("[{file}] dropped the retired fixture block '{key}'");
        }
    }

    retire_work_coordinate_system(obj, &file);

    let Some(origin) = obj.get_mut("origin").and_then(Value::as_object_mut) else {
        return;
    };
    for (axis, allowed, fallback) in
        [("x0", ["left", "right"], "left"), ("y0", ["front", "back"], "front")]
    {
        let current = origin.get(axis).and_then(Value::as_str).unwrap_or(fallback);
        if !allowed.contains(&current) {
            warn!("[{file}] origin.{axis} was '{current}', which is not an {axis} edge; using '{fallback}'");
            origin.insert(axis.to_string(), Value::from(fallback));
        }
    }
}

/// Drops the retired `work_coordinate_system` ordinal, leaving `origin_reference` unset.
///
/// The ordinal is **not** converted. It looks convertible — most profiles mapped it as
/// `G53 + n`, so `3` "is" `G56` — but a Bantam maps `G54 + n` because its G54 is reserved,
/// so the same `3` is `G57` there. Writing the common answer into the one profile it is
/// wrong for would put the job in the wrong place on the bed, silently, which is the exact
/// failure this whole field change exists to remove. So the value is reported, not moved:
/// `set_origin` refuses to generate against a blank reference and says so in the operator's
/// own words.
///
/// The suggestion in the warning is explicitly labelled an assumption for the same reason.
fn retire_work_coordinate_system(obj: &mut serde_json::Map<String, Value>, file: &str) {
    let Some(retired) = obj.remove("work_coordinate_system") else {
        return;
    };
    // Only a sane ordinal earns a suggestion; anything else is reported bare.
    let suggestion = retired
        .as_u64()
        .filter(|n| (1..=6).contains(n))
        .map(|n| format!(" On most controllers that was 'G{}', but a machine that reserves \
                          low offsets (a Bantam reserves G54) numbers them differently — \
                          check the work offset on the machine itself.", 53 + n))
        .unwrap_or_default();

    warn!(
        "[{file}] dropped the retired 'work_coordinate_system: {retired}'. Set this \
         fixture's Machine Origin Reference to the offset the machine actually uses; \
         generation refuses to run while it is blank.{suggestion}"
    );
}

/// Adapts [`normalize_machining_value`] to the [`load_normalized`] signature; machining
/// normalisation needs no file name.
fn normalize_machining_file(value: &mut Value, _path: &Path) {
    normalize_machining_value(value);
}

/// The per-step keys moved out of the pre-v3 flat top level into a step object.
const STEP_KEYS: &[&str] = &[
    "cnc",
    "fixture",
    "toolset",
    "side_to_machine",
    "operations",
    "routing",
    "drill_locating_pins",
    "drill_pth",
    "drill_npth",
    "route_board",
    "mill_board",
];

/// Converts an on-disk `processing_profiles` value into current `machining.yaml`
/// (schema_version 3) form.
///
/// - A **pre-v3 flat** profile (no `steps`, the whole setup at the top level) is
///   wrapped into a single-step profile: the [`STEP_KEYS`] are lifted into one
///   `steps[0]` entry and stamped `schema_version: 3`. This is lossless — the one
///   setup becomes the profile's only step.
/// - A **v3 stepped** profile is left structurally intact.
///
/// Either way each step is cleaned by [`normalize_step_value`]. The version is
/// stamped to 3 so the datastore's `x-schema-version` gate accepts the file
/// (it hard-rejects a mismatched `schema_version`).
fn normalize_machining_value(value: &mut Value) {
    let Some(obj) = value.as_object_mut() else {
        return;
    };

    obj.insert("schema_version".to_string(), Value::from(3));

    if !obj.contains_key("steps") {
        // Legacy flat: pull the top-level setup into a single step.
        let mut step = serde_json::Map::new();
        step.insert("name".to_string(), Value::String("Machining step".to_string()));
        for key in STEP_KEYS {
            if let Some(field) = obj.remove(*key) {
                step.insert((*key).to_string(), field);
            }
        }
        let mut step_value = Value::Object(step);
        normalize_step_value(&mut step_value);
        obj.insert("steps".to_string(), Value::Array(vec![step_value]));
        return;
    }

    if let Some(steps) = obj.get_mut("steps").and_then(Value::as_array_mut) {
        for step in steps {
            normalize_step_value(step);
        }
    }
}

/// Cleans a single step object:
///
/// - removes each operation object's `enabled` flag — the schema drives
///   enablement from the step's `operations` array, and `additionalProperties:
///   false` would otherwise reject the field;
/// - drops empty-string cnc/fixture/toolset references so an unset binding reads
///   as *absent* (hence incomplete, prompting the user) rather than an invalid
///   UUID.
///
/// Rewrites a step's `route_board` from the edge-only shape into the current one.
///
/// The old block described the board's boundary and nothing else:
///
/// ```yaml
/// route_board:
///   edge: { cut, retention, tabs, tab_width, bite_holes, vgroove_depth }
///   finishing: { clearance, direction }
/// ```
///
/// Four things changed, and all four would be rejected by `additionalProperties: false`
/// if left alone:
///
/// - `edge` became `outline`, with interior `cutouts` beside it — the boundary is not
///   the only thing routed out of a board.
/// - `retention` grew from a bare enum into an object, and its two mouse-bite values
///   folded into a `mouse_bites` flag on a tab. A mouse bite is a perforated *tab*, not
///   an alternative to one.
/// - `bite_holes` is gone: how many holes perforate a tab follows from the tab width and
///   the drill.
/// - `finishing` collapsed from `{clearance, direction}` to the clearance alone. Climb is
///   the only sensible direction for cutting a part out, and the toolpaths take it from
///   the geometry.
fn normalize_route_board_value(step: &mut serde_json::Map<String, Value>) {
    let Some(route_board) = step.get_mut("route_board").and_then(Value::as_object_mut) else {
        return;
    };

    if let Some(Value::Object(mut edge)) = route_board.remove("edge") {
        // `retention: mouse_bites | tabs_with_mouse_bites` both meant "tabs, perforated".
        let old_mode = edge.remove("retention");
        let old_mode = old_mode.as_ref().and_then(Value::as_str).unwrap_or("tabs");
        let mouse_bites = matches!(old_mode, "mouse_bites" | "tabs_with_mouse_bites");
        let mode = if old_mode == "none" { "none" } else { "tabs" };

        let mut retention = serde_json::Map::new();
        retention.insert("mode".into(), Value::from(mode));
        if let Some(count) = edge.remove("tabs") {
            retention.insert("count".into(), count);
        }
        if let Some(width) = edge.remove("tab_width") {
            retention.insert("width".into(), width);
        }
        retention.insert("mouse_bites".into(), Value::from(mouse_bites));
        edge.remove("bite_holes");
        edge.insert("retention".into(), Value::Object(retention));

        route_board.insert("outline".into(), Value::Object(edge));
    }

    // `{clearance, direction}` → the clearance alone.
    if let Some(Value::Object(finishing)) = route_board.get("finishing").cloned() {
        let clearance = finishing.get("clearance").cloned().unwrap_or(Value::from("0.1mm"));
        route_board.insert("finishing".into(), clearance);
    }
}

/// Operation config objects are left in place (always materialized by the
/// loader); only their `enabled` flag is stripped.
fn normalize_step_value(step: &mut Value) {
    let Some(obj) = step.as_object_mut() else {
        return;
    };

    for key in ["drill_locating_pins", "drill_pth", "drill_npth", "route_board", "mill_board"] {
        if let Some(op) = obj.get_mut(key).and_then(Value::as_object_mut) {
            op.remove("enabled");
        }
    }

    // The retired `routing` block (`cut_depth_strategy` / `multi_pass_max_depth`).
    // Routing is single-pass at the tool's rated feed — the feed rating already
    // assumes cutting a board's full thickness — so the setting was removed rather
    // than left as a knob that changed nothing. Every profile written before that
    // still carries the block, and `additionalProperties: false` would reject it on
    // load, so it is dropped here.
    obj.remove("routing");

    normalize_route_board_value(obj);

    // A step's cnc/fixture/toolset was once `{ default: <uuid>, choices: [<uuid>…] }`,
    // for a job-level override that was never built and has since been dropped: a step
    // is one physical setup, so an alternative machine is a second step. Collapse the
    // old shape onto its `default`, which is the only part that ever selected anything.
    //
    // An empty or missing default becomes an absent key — "no profile chosen" — rather
    // than the empty string the old editor wrote, which the `uuid_v7` pattern rejects.
    for key in ["cnc", "fixture", "toolset"] {
        let collapsed = match obj.get(key) {
            Some(Value::Object(binding)) => binding.get("default").and_then(Value::as_str),
            Some(Value::String(id)) => Some(id.as_str()),
            _ => continue,
        }
        .filter(|id| !id.is_empty())
        .map(|id| Value::String(id.to_string()));

        match collapsed {
            Some(reference) => {
                obj.insert(key.to_string(), reference);
            }
            None => {
                obj.remove(key);
            }
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

/// Upgrades pre-existing stock files to the trimmed schema: the old `ref` (catalog
/// reference) object is replaced by a plain `source_catalog` name — all the app
/// ever used from it. Runs on load before validation so files written with the old
/// `ref` still parse (the removed `ref` would otherwise fail `additionalProperties`).
/// Idempotent — tools without `ref` are left untouched.
fn migrate_stock_ref(text: &str) -> String {
    let Ok(mut value) = serde_json::from_str::<Value>(text) else {
        return text.to_string();
    };
    if let Some(tools) = value.get_mut("tools").and_then(Value::as_array_mut) {
        for tool in tools {
            let Some(obj) = tool.as_object_mut() else {
                continue;
            };
            let Some(reference) = obj.remove("ref") else {
                continue;
            };
            if !obj.contains_key("source_catalog") {
                if let Some(catalog) = reference.get("catalog").cloned() {
                    obj.insert("source_catalog".to_string(), catalog);
                }
            }
        }
    }
    serde_json::to_string(&value).unwrap_or_else(|_| text.to_string())
}

/// Fills each stock tool's `overrides` with any `base` field it is missing, so the
/// override object is fully materialized. The stock detail editor edits override
/// fields in place (it cannot insert), so every editable field must already exist.
/// Idempotent — existing overrides win, so user edits are preserved; a field left
/// equal to base simply reads as "unchanged". Runs on load to upgrade tools written
/// before the immutable-base / editable-overrides split.
fn materialize_stock_overrides(text: &str) -> String {
    let Ok(mut value) = serde_json::from_str::<Value>(text) else {
        return text.to_string();
    };
    if let Some(tools) = value.get_mut("tools").and_then(Value::as_array_mut) {
        for tool in tools {
            let Some(base) = tool.get("base").and_then(Value::as_object).cloned() else {
                continue;
            };
            let Some(obj) = tool.as_object_mut() else {
                continue;
            };
            let overrides = obj.entry("overrides").or_insert_with(|| serde_json::json!({}));
            if let Some(over) = overrides.as_object_mut() {
                for (key, val) in base {
                    over.entry(key).or_insert(val);
                }
            }
        }
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

    /// Build guard (schema-centric app): every bundled catalog under
    /// `assets/catalogs`, once run through the app's own `normalize_catalog_fields`
    /// enricher (which backfills id/sku/point_angle/z_min_depth/schema_version —
    /// the bundled catalogs are intentionally terse), validates cleanly against
    /// `catalog.yaml`. This exercises the real seed+backfill pipeline, so a source
    /// catalog the enricher can't rescue (bad unit, unknown tool type, structural
    /// error) fails here — at `cargo test` / CI — instead of degrading into a
    /// silent load-time warning (see `AppData::load`).
    #[test]
    fn bundled_catalogs_validate_against_the_schema() {
        let store = build_datastore();
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets").join("catalogs");

        let mut checked = 0usize;
        for entry in fs::read_dir(&dir).expect("assets/catalogs is readable") {
            let path = entry.expect("readable dir entry").path();
            if !is_yaml(&path) {
                continue; // skip the .xlsx / .txt reference material alongside the YAML
            }
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("catalog");

            let text = fs::read_to_string(&path).expect("catalog file is readable");
            let mut value = parse_yaml_value(&text)
                .unwrap_or_else(|| panic!("catalog {} is not valid YAML", path.display()));
            // Same enrichment the seeding path applies via `backfill_catalog_fields`
            // (inject missing, don't canonicalize typed values).
            crate::catalog_io::normalize_catalog_fields(&mut value, stem, true, false);

            let json = serde_json::to_string(&value).expect("serialize normalized catalog");
            let outcome = store.parse(&[ParseInput {
                schema_id: "catalog.yaml",
                source: Some(path.clone()),
                text: &json,
            }]);
            assert!(
                outcome.errors.is_empty(),
                "catalog {} failed validation after normalization:\n{:#?}",
                path.display(),
                outcome.errors
            );
            checked += 1;
        }
        assert!(checked > 0, "found no catalog YAML under {}", dir.display());
    }

    /// Build guard: every bundled CNC template instantiates into a *complete*,
    /// schema-valid `cnc.yaml` profile and every one of its GTL primitive
    /// templates compiles. Catches a template that has drifted from the schema, or
    /// ships broken GTL, before it can reach a user's "create CNC from template".
    #[test]
    fn bundled_cnc_templates_validate_and_compile() {
        let store = build_datastore();
        // A bare GTL engine is enough for a syntax check: compilation is parse-only,
        // so unresolved calls like `metric()` are fine — we only assert the template
        // grammar and embedded Rhai parse.
        let engine = gtl::Gtl::new();

        for (key, text) in CNC_TEMPLATES {
            let seed = parse_yaml_value(text)
                .unwrap_or_else(|| panic!("CNC template '{key}' is not valid YAML"));

            // Seeds intentionally omit id/schema_version; `instantiate_from` fills
            // those and the schema defaults, then overlays the template body — the
            // same path `create_cnc_from_template` takes.
            let node = store
                .instantiate_from("cnc.yaml", &seed)
                .unwrap_or_else(|| panic!("CNC template '{key}' did not instantiate"));
            assert!(
                node.status.is_complete(),
                "CNC template '{key}' is incomplete after instantiation: {:?}",
                node.status
            );

            let value = node.to_value();

            // Re-validate the materialised profile so bad enums/units on template
            // fields (which survive instantiation) are caught, not just missing keys.
            let json = serde_json::to_string(&value).expect("serialize instantiated node");
            let outcome = store.parse(&[ParseInput {
                schema_id: "cnc.yaml",
                source: Some(PathBuf::from(format!("{key} (cnc template)"))),
                text: &json,
            }]);
            assert!(
                outcome.errors.is_empty(),
                "CNC template '{key}' failed schema validation:\n{:#?}",
                outcome.errors
            );

            // Syntax-check each GTL primitive template.
            if let Some(primitives) = value.get("primitives").and_then(Value::as_object) {
                for (name, primitive) in primitives {
                    if let Some(source) = primitive.as_str() {
                        if let Err(err) = engine.compile(name, source) {
                            panic!("CNC template '{key}' primitive '{name}' has invalid GTL: {err}");
                        }
                    }
                }
            }
        }
    }

    /// Every profile saved before `set_unit` existed has no such key, and none of them
    /// were rewritten — the schema default is what keeps them emitting their unit
    /// statement, and is why moving `G21`/`G20` out of the application needed no
    /// migration pass. If that default is ever dropped, those profiles fall silent
    /// about their units, which this catches.
    #[test]
    fn a_profile_written_before_set_unit_existed_still_states_its_units() {
        let store = build_datastore();
        let mut seed = parse_yaml_value(include_str!(
            "../../assets/cnc_templates/genmitsu_3018.yaml"
        ))
        .expect("the bundled template parses");
        seed.pointer_mut("/primitives")
            .and_then(Value::as_object_mut)
            .expect("the template has primitives")
            .remove("set_unit")
            .expect("which included set_unit before this test removed it");

        let node = store
            .instantiate_from("cnc.yaml", &seed)
            .expect("a profile without set_unit still instantiates");

        assert_eq!(
            node.to_value().pointer("/primitives/set_unit").and_then(Value::as_str),
            Some(r#"`{if metric { "G21" } else { "G20" }}"#),
            "the schema default stands in for the key the old profile never had"
        );
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
             drill_locating_pins: {{ enabled: false }}\n\
             drill_pth: {{ enabled: true, holes: {{ route_fallback: false, drill_first: true, pilot: false, oblong: drill_ends_then_route, oversize: {{ relative: 8%, max: 0.20mm }}, undersize: {{ relative: 8%, max: 0.20mm }} }} }}\n\
             drill_npth: {{ enabled: false }}\n\
             route_board: {{ enabled: false }}\n\
             mill_board: {{ enabled: false }}\n"
        );
        fs::write(proc_dir.join(format!("{id}.yaml")), legacy).unwrap();

        let (mut data, _errors) = AppData::load_from(&data_dir, &dir.path().join("catalogs"));

        // The legacy flat file migrated into a single-step machining doc: `enabled`
        // gone, both `{default, choices}` bindings collapsed onto their default, the
        // empty fixture ref dropped (absent, not invalid) — all now under steps[0].
        let doc = data.get(id).expect("legacy machining loaded");
        assert!(
            matches!(&doc.root.get_pointer("/steps").unwrap().value, NodeValue::Array(a) if a.len() == 1),
            "flat legacy profile becomes one step"
        );
        assert!(doc.root.get_pointer("/steps/0/drill_pth/enabled").is_none(), "enabled should be stripped");
        assert!(doc.root.get_pointer("/steps/0/fixture").is_none(), "empty ref should be dropped");
        let cnc_ref = doc.root.get_pointer("/steps/0/cnc").expect("real ref preserved");
        assert!(
            matches!(&cnc_ref.value, NodeValue::Ref(r) if r.raw == cnc),
            "the binding collapses onto its default, not onto the choices array"
        );

        // Structural edits round-trip: set step 0's fixture reference and operations,
        // then clear the reference again.
        let fixture = uuid::Uuid::now_v7();
        assert!(data.set_step_reference(id, 0, "fixture", Some(fixture)));
        assert!(data.set_step_operations(id, 0, &["drill_pth".to_string(), "route_board".to_string()]));

        let doc = data.get(id).unwrap();
        let stored = doc.root.get_pointer("/steps/0/fixture").expect("fixture set");
        assert!(matches!(&stored.value, NodeValue::Ref(r) if r.raw == fixture));
        let ops = doc.root.get_pointer("/steps/0/operations").unwrap();
        assert!(matches!(&ops.value, NodeValue::Array(a) if a.len() == 2));

        // "No profile" removes the key: an empty string would fail the uuid pattern
        // on the next load, which is how the old editor used to corrupt a file.
        assert!(data.set_step_reference(id, 0, "fixture", None));
        assert!(data.get(id).unwrap().root.get_pointer("/steps/0/fixture").is_none());
    }

    #[test]
    fn machining_creates_with_one_default_step() {
        // A fresh machining profile has exactly one step whose operations and
        // per-op config are materialized; its bindings are absent (incomplete)
        // until the user picks them.
        let dir = tempdir().unwrap();
        let (mut data, _) = load_temp(dir.path());
        let id = data.create(Profile::Machining).expect("create machining");

        let doc = data.get(id).unwrap();
        assert!(
            matches!(&doc.root.get_pointer("/steps").unwrap().value, NodeValue::Array(a) if a.len() == 1),
            "one default step"
        );
        assert!(doc.root.get_pointer("/steps/0/operations").is_some(), "step operations materialized");
        assert!(
            doc.root.get_pointer("/steps/0/drill_pth/holes").is_some(),
            "per-op config materialized within the step"
        );
        assert!(doc.root.get_pointer("/steps/0/cnc").is_none(), "binding absent until picked");
    }

    #[test]
    fn machining_step_add_remove_and_reorder() {
        let dir = tempdir().unwrap();
        let (mut data, _) = load_temp(dir.path());
        let id = data.create(Profile::Machining).expect("create machining");

        let step_count = |data: &AppData| match &data.get(id).unwrap().root.get_pointer("/steps").unwrap().value {
            NodeValue::Array(items) => items.len(),
            _ => 0,
        };

        // Add a second step and configure it distinctly.
        assert!(data.add_step(id));
        assert_eq!(step_count(&data), 2);
        assert!(data.set_step_operations(id, 1, &["route_board".to_string()]));
        assert!(data.set_field(id, "/steps/1/name", NodeValue::Str("Route".into())).unwrap_or(false));

        // Reorder: step 1 → position 0.
        assert!(data.move_step(id, 1, 0));
        let doc = data.get(id).unwrap();
        assert!(
            matches!(&doc.root.get_pointer("/steps/0/name").unwrap().value, NodeValue::Str(s) if s == "Route"),
            "moved step is now first"
        );

        // Remove down to one; removing the last is a no-op.
        assert!(data.remove_step(id, 0));
        assert_eq!(step_count(&data), 1);
        assert!(data.remove_step(id, 0));
        assert_eq!(step_count(&data), 1, "a profile always keeps at least one step");
    }

    /// "+ Add step" must not produce a profile that cannot generate.
    ///
    /// Most operations may be claimed by only one step per board side, and the default
    /// step used to be `drill_pth` unconditionally — so adding a step to the default
    /// profile created an immediate clash with step 1, in the one click that is the only
    /// route to a second step. Each new step takes the first operation still free.
    #[test]
    fn an_added_step_defaults_to_an_operation_no_other_step_has_claimed() {
        let dir = tempdir().unwrap();
        let (mut data, _) = load_temp(dir.path());
        let id = data.create(Profile::Machining).expect("create machining");

        let operations = |data: &AppData, step: usize| -> Vec<String> {
            match &data
                .get(id)
                .unwrap()
                .root
                .get_pointer(&format!("/steps/{step}/operations"))
                .unwrap()
                .value
            {
                NodeValue::Array(items) => items
                    .iter()
                    .filter_map(|item| match &item.value {
                        NodeValue::Str(key) => Some(key.clone()),
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            }
        };

        assert_eq!(operations(&data, 0), vec!["drill_pth".to_string()], "the seeded step");

        assert!(data.add_step(id));
        assert_eq!(operations(&data, 1), vec!["drill_npth".to_string()], "PTH is taken");
        assert!(data.add_step(id));
        assert_eq!(operations(&data, 2), vec!["route_board".to_string()], "and so is NPTH");

        // Past the once-per-side operations it settles on the repeatable one rather
        // than running out — a step must carry at least one operation.
        assert!(data.add_step(id));
        assert!(data.add_step(id));
        assert_eq!(operations(&data, 3), vec!["drill_locating_pins".to_string()]);
        assert_eq!(
            operations(&data, 4),
            vec!["drill_locating_pins".to_string()],
            "pins are repeatable, so they stay available"
        );
    }

    #[test]
    fn job_singleton_references_a_machining_profile() {
        // The Job is a singleton (no id/name) referencing one machining profile;
        // the reference persists next to settings/stock and survives reload.
        let dir = tempdir().unwrap();
        let (mut data, _) = load_temp(dir.path());
        let machining = data.create(Profile::Machining).expect("create machining");

        assert!(data.job().is_some(), "job singleton seeded on a fresh dir");
        assert!(data.job_machining_profile().is_none(), "no machining profile yet");

        assert!(data.set_job_machining_profile(Some(machining)));
        assert_eq!(data.job_machining_profile(), Some(machining));
        data.flush();

        let path = dir.path().join("data").join(JOB_FILE);
        assert!(path.exists(), "expected job file at {}", path.display());

        // The job's reference survives a reload. (The freshly-created machining
        // profile is itself incomplete — no cnc/fixture/toolset picked yet — which
        // is expected and unrelated to the job singleton, so we don't assert a
        // clean reload here.)
        let (reloaded, _errors) = load_temp(dir.path());
        assert_eq!(reloaded.job_machining_profile(), Some(machining), "reference survives reload");
    }

    #[test]
    fn job_board_orientation_defaults_to_zero_and_persists() {
        // The board orientation is live per-job data on the singleton: it defaults
        // to 0 on a fresh job and survives a reload once set (persist-and-project).
        let dir = tempdir().unwrap();
        let (mut data, _) = load_temp(dir.path());

        assert_eq!(data.job_board_orientation(), 0, "fresh job orients at 0");

        assert!(data.set_job_board_orientation(37));
        assert_eq!(data.job_board_orientation(), 37);
        data.flush();

        let (reloaded, _errors) = load_temp(dir.path());
        assert_eq!(reloaded.job_board_orientation(), 37, "angle survives reload");
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
                flute_length: None,
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
                flute_length: None,
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
        // The legacy `ref` object was migrated to a plain `source_catalog` name (and
        // `ref` is no longer a valid property, so a clean load proves it was stripped).
        assert!(stock.root.get_pointer("/tools/0/ref").is_none());
        assert!(
            matches!(
                stock.root.get_pointer("/tools/0/source_catalog").map(|n| &n.value),
                Some(NodeValue::Str(s)) if s == "Manual"
            ),
            "ref.catalog should have been migrated to source_catalog"
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

    /// A settings file predating `gcode_save_directory` must still load. Validation
    /// runs before schema defaults are injected, so listing an added key under
    /// `required` would lock every existing user out of their settings — this pins
    /// that it stays optional.
    #[test]
    fn settings_written_before_the_save_directory_existed_still_load() {
        let dir = tempdir().unwrap();
        let data_dir = dir.path().join("data");
        fs::create_dir_all(&data_dir).unwrap();
        let previous = "schema_version: 1\n\
                        units: mm\n\
                        theme: Dark\n\
                        selected_process_profile_id: null\n\
                        selected_cnc_profile_id: null\n\
                        selected_fixture_profile_id: null\n\
                        selected_toolset_profile_id: null\n";
        fs::write(data_dir.join(SETTINGS_FILE), previous).unwrap();

        let (mut data, errors) = AppData::load_from(&data_dir, &dir.path().join("catalogs"));
        assert!(errors.is_empty(), "settings without the key should load: {errors:#?}");

        // And the first save can then record a directory that survives a reload.
        let saved = dir.path().join("out").to_string_lossy().into_owned();
        let mut value = data.settings().expect("settings loaded").to_value();
        value["gcode_save_directory"] = Value::String(saved.clone());
        assert!(
            data.replace_settings_from_value(&value).is_some_and(|p| p.is_empty()),
            "recording the save directory should not error"
        );
        data.flush();

        let (reloaded, errors) = AppData::load_from(&data_dir, &dir.path().join("catalogs"));
        assert!(errors.is_empty(), "reload should be clean: {errors:#?}");
        let node = reloaded
            .settings()
            .and_then(|doc| doc.root.get_pointer("/gcode_save_directory"))
            .map(|node| node.value.clone());
        assert!(
            matches!(node, Some(NodeValue::Str(ref s)) if *s == saved),
            "the save directory should round-trip, got {node:?}"
        );
    }

    /// The removable-media path is a second, independent history: a "Save to USB" must
    /// record where it went without disturbing where the ordinary Save opens. Same
    /// optional-key rule as above — it was added later than every settings file in the
    /// wild.
    #[test]
    fn the_removable_media_path_round_trips_without_touching_the_save_directory() {
        let dir = tempdir().unwrap();
        let data_dir = dir.path().join("data");
        fs::create_dir_all(&data_dir).unwrap();

        let (mut data, errors) = AppData::load_from(&data_dir, &dir.path().join("catalogs"));
        assert!(errors.is_empty(), "a fresh store should load: {errors:#?}");

        let ordinary = dir.path().join("downloads").to_string_lossy().into_owned();
        let removable = "E:\\jobs".to_string();
        let mut value = data.settings().expect("settings loaded").to_value();
        value["gcode_save_directory"] = Value::String(ordinary.clone());
        value["last_removable_media_path"] = Value::String(removable.clone());
        assert!(
            data.replace_settings_from_value(&value).is_some_and(|p| p.is_empty()),
            "recording both directories should not error"
        );
        data.flush();

        let (reloaded, errors) = AppData::load_from(&data_dir, &dir.path().join("catalogs"));
        assert!(errors.is_empty(), "reload should be clean: {errors:#?}");
        let read = |pointer: &str| {
            reloaded
                .settings()
                .and_then(|doc| doc.root.get_pointer(pointer))
                .map(|node| node.value.clone())
        };
        assert!(
            matches!(read("/last_removable_media_path"), Some(NodeValue::Str(ref s)) if *s == removable),
            "the removable-media path should round-trip"
        );
        assert!(
            matches!(read("/gcode_save_directory"), Some(NodeValue::Str(ref s)) if *s == ordinary),
            "the ordinary save directory must be untouched by it"
        );
    }

    /// A `linear_cut` that cannot emit a feed is repaired; one that can — by variable or
    /// hardcoded — is left exactly as the operator wrote it.
    /// Loading a profile reports what is wrong with it and **changes no template**.
    ///
    /// Every one of these checks was once a repair that wrote a replacement, and every
    /// replacement was machine code this application had chosen — a G1 line, a G2/G3
    /// pair, a G-word expression, an N-word prefix. The profile owns the machine
    /// language; a profile that needs changing gets a warning naming the variable or
    /// behaviour at fault, and its operator makes the edit.
    ///
    /// This also keeps a non-G-code profile safe: an Excellon `linear_cut` carries no
    /// `F` word, and the old feed "repair" would have overwritten it with a G1 line.
    #[test]
    fn loading_a_profile_never_rewrites_its_templates() {
        let untouched = |primitives: Value| {
            let mut value = serde_json::json!({ "primitives": primitives });
            let before = value.clone();
            normalize_cnc_value(&mut value, Path::new("machine.yaml"));
            assert_eq!(value, before, "load must not edit a template");
        };

        // No feed at all — warned about (a router at a drill's plunge feed breaks), kept.
        untouched(serde_json::json!({ "linear_cut": "`G1 X{x} Y{y} Z{z} S{s}" }));
        // The retired `{arc_cmd}` variable — warned about, kept; it fails at render,
        // which is the honest outcome and better than inventing G2/G3 here.
        untouched(serde_json::json!({ "cut_arc": "`{arc_cmd} X{x} Y{y} I{i} J{j}" }));
        // A hardcoded work offset — warned about, kept.
        untouched(serde_json::json!({ "initialise": "`G17 G54 G40 G49 G80 G90" }));
        // A duplicate spindle speed across change_tool/start_spindle — warned, kept.
        untouched(serde_json::json!({
            "change_tool": "`T{slot} M06\n`S{rpm}",
            "start_spindle": "`S{rpm}\n`M3",
        }));
        // An Excellon-style cut with no F word: nothing to repair it into.
        untouched(serde_json::json!({ "linear_cut": "`X{x}Y{y}" }));

        // A profile with no primitives block at all must not panic.
        let mut bare = serde_json::json!({ "name": "no primitives" });
        normalize_cnc_value(&mut bare, Path::new("machine.yaml"));
        assert_eq!(bare, serde_json::json!({ "name": "no primitives" }));
    }

    /// Four fixture blocks were declared in full but never shown, never read and never
    /// implemented; they are gone, and `additionalProperties: false` would reject every
    /// existing fixture that still carries them.
    ///
    /// `origin` was also loosened: both axes accepted all four edge names, so `x0: front`
    /// validated and meant nothing. X can only be zeroed on a left or right edge and Y on
    /// a front or back one — and a value outside that is the schema's old fault, so it is
    /// corrected rather than made to reject the profile.
    #[test]
    fn a_fixture_with_the_retired_blocks_and_a_nonsense_origin_still_loads() {
        let mut value = serde_json::json!({
            "name": "Vice",
            "locating_pins": { "strategy": "two_pin" },
            "keep_out_zones": [{ "x": "0mm" }],
            "occupancy": { "min_board": "10mm" },
            "probing_alignment": { "enabled": true },
            "origin": { "x0": "front", "y0": "left" },
        });
        normalize_fixture_value(&mut value, Path::new("vice.yaml"));

        for key in RETIRED_FIXTURE_KEYS {
            assert!(value.get(*key).is_none(), "'{key}' must not survive into validation");
        }
        assert_eq!(value.pointer("/origin/x0").and_then(Value::as_str), Some("left"));
        assert_eq!(value.pointer("/origin/y0").and_then(Value::as_str), Some("front"));
        assert_eq!(value.get("name").and_then(Value::as_str), Some("Vice"), "the rest is untouched");

        // A fixture that already says something sensible keeps saying it.
        let mut kept = serde_json::json!({ "origin": { "x0": "right", "y0": "back" } });
        normalize_fixture_value(&mut kept, Path::new("vice.yaml"));
        assert_eq!(kept.pointer("/origin/x0").and_then(Value::as_str), Some("right"));
        assert_eq!(kept.pointer("/origin/y0").and_then(Value::as_str), Some("back"));

        // And one with no origin block at all must not panic.
        let mut bare = serde_json::json!({ "name": "Tape" });
        normalize_fixture_value(&mut bare, Path::new("tape.yaml"));
        assert_eq!(bare, serde_json::json!({ "name": "Tape" }));
    }

    /// A fixture written when the machine origin was an ordinal still loads: the retired
    /// key is dropped (`additionalProperties: false` would otherwise reject the profile)
    /// and `origin_reference` is left **unset**.
    ///
    /// Deliberately not converted. `3` maps to `G56` on most controllers but `G57` on a
    /// Bantam, whose G54 is reserved — and this migration cannot see which machine the
    /// fixture is used with, because a fixture does not reference a CNC. Writing the common
    /// answer would put the job in the wrong place on the one profile it is wrong for,
    /// silently, which is the failure the whole field change exists to remove. `set_origin`
    /// refuses to generate against a blank reference and says so.
    #[test]
    fn a_fixture_holding_the_retired_ordinal_loads_with_no_origin_reference() {
        let mut value = serde_json::json!({
            "name": "Pin jig",
            "work_coordinate_system": 3,
            "z_safe": "20mm",
        });
        normalize_fixture_value(&mut value, Path::new("jig.yaml"));

        assert!(
            value.get("work_coordinate_system").is_none(),
            "the retired ordinal must not survive into validation"
        );
        assert!(
            value.get("origin_reference").is_none(),
            "nothing is invented for it — the schema default (empty) applies, and \
             set_origin reports that"
        );
        assert_eq!(value.get("z_safe").and_then(Value::as_str), Some("20mm"), "the rest is untouched");

        // An out-of-range or non-numeric ordinal is dropped just the same — there is no
        // value it could be converted to, and rejecting the profile over a field that no
        // longer exists would be worse.
        for odd in [serde_json::json!(99), serde_json::json!("three"), serde_json::json!(null)] {
            let mut value = serde_json::json!({ "work_coordinate_system": odd });
            normalize_fixture_value(&mut value, Path::new("jig.yaml"));
            assert!(value.get("work_coordinate_system").is_none(), "dropped whatever it held");
        }
    }

    /// The CNC's stored-zero *count* is retired with the fixture ordinal that indexed into
    /// it. Same reason it must be dropped rather than merely ignored: the schema no longer
    /// declares it, and `additionalProperties: false` rejects what it does not declare.
    #[test]
    fn a_cnc_profile_holding_the_retired_offset_count_still_loads() {
        let mut value = serde_json::json!({
            "machine": { "work_coordinate_systems": 6, "atc_slot_count": 8 },
        });
        normalize_cnc_value(&mut value, Path::new("machine.yaml"));

        assert!(
            value.pointer("/machine/work_coordinate_systems").is_none(),
            "the retired count must not survive into validation"
        );
        assert_eq!(
            value.pointer("/machine/atc_slot_count").and_then(Value::as_u64),
            Some(8),
            "the rest of the machine block is untouched"
        );
    }

    /// The detection behind the `G54` warning is whole-word, so a profile is not nagged
    /// about a `G54` that is part of something else. It has to be exact: the warning
    /// tells an operator to go and edit a template, and sending them after a word that
    /// is not there wastes their time.
    #[test]
    fn the_word_check_respects_boundaries() {
        assert!(contains_word("G17 G54 G40", "G54"));
        assert!(contains_word("G54", "G54"), "alone on the line");
        assert!(contains_word("a\nG54\nb", "G54"));
        assert!(contains_word("G540 G54", "G54"), "the second one counts");
        assert!(!contains_word("G540", "G54"), "not a prefix");
        assert!(!contains_word("G54.1", "G54"), "not part of a decimal word");
        assert!(!contains_word("XG54", "G54"), "not a suffix");
        assert!(!contains_word("nothing here", "G54"));
    }

    /// The retired `line_numbering_increment` must still be **removed** —
    /// `additionalProperties: false` would reject the whole profile otherwise — but its
    /// value is no longer turned into a `line_number` template, because an "N" prefix is
    /// a G-code word and this profile may not be G-code.
    ///
    /// Dropping a dead key is not the same as authoring machine code; the difference is
    /// the whole of this test.
    #[test]
    fn the_retired_line_numbering_increment_is_dropped_without_seeding_a_template() {
        let mut profile = serde_json::json!({
            "machine": { "atc_slot_count": 0, "line_numbering_increment": 10 },
            "primitives": {}
        });
        normalize_cnc_value(&mut profile, Path::new("machine.yaml"));

        assert!(
            profile.pointer("/machine/line_numbering_increment").is_none(),
            "the retired field must not survive into validation"
        );
        assert!(
            profile.pointer("/primitives/line_number").is_none(),
            "numbering is the profile's to define; the warning says so"
        );

        // A profile that already has a template keeps it, untouched.
        let mut both = serde_json::json!({
            "machine": { "line_numbering_increment": 10 },
            "primitives": { "line_number": "`/{line}:`" }
        });
        normalize_cnc_value(&mut both, Path::new("machine.yaml"));
        assert!(both.pointer("/machine/line_numbering_increment").is_none());
        assert_eq!(
            both.pointer("/primitives/line_number").and_then(Value::as_str),
            Some("`/{line}:`")
        );
    }

    /// Every profile on disk was written with `{ default, choices }` bindings. They must
    /// collapse onto the chosen profile, not be rejected by `additionalProperties: false`
    /// and not lose the selection.
    #[test]
    fn a_stepped_profile_with_the_old_choices_bindings_collapses_onto_its_default() {
        let dir = tempdir().unwrap();
        let data_dir = dir.path().join("data");
        let machining_dir = data_dir.join(Profile::Machining.dir_name());
        fs::create_dir_all(&machining_dir).unwrap();
        let id = uuid::Uuid::now_v7();
        let cnc = uuid::Uuid::now_v7();
        let other = uuid::Uuid::now_v7();
        let previous = format!(
            "schema_version: 3\n\
             id: \"{id}\"\n\
             name: Old bindings\n\
             steps:\n\
             \x20 - name: Step 1\n\
             \x20   operations: [drill_pth]\n\
             \x20   cnc: {{ default: \"{cnc}\", choices: [\"{cnc}\", \"{other}\"] }}\n\
             \x20   fixture: {{ default: '', choices: [] }}\n"
        );
        fs::write(machining_dir.join("old.yaml"), previous).unwrap();

        let (data, errors) = AppData::load_from(&data_dir, &dir.path().join("catalogs"));
        assert!(
            errors.iter().all(|e| !format!("{e:?}").contains("choices")),
            "the retired choices array must not surface as a load error: {errors:#?}"
        );
        let doc = data.get(id).expect("the profile should still load");
        let stored = doc.root.get_pointer("/steps/0/cnc").expect("cnc kept");
        assert!(
            matches!(&stored.value, NodeValue::Ref(r) if r.raw == cnc),
            "the chosen profile survives; the rejected alternative does not"
        );
        assert!(
            doc.root.get_pointer("/steps/0/fixture").is_none(),
            "a binding with no default becomes absent — 'no profile chosen'"
        );
    }

    /// A machining profile written before the `routing` block was retired must still
    /// load: `additionalProperties: false` would otherwise reject the stale key and
    /// warn on every launch. Caught from a real user profile on disk.
    #[test]
    fn a_machining_profile_with_the_retired_routing_block_still_loads() {
        let dir = tempdir().unwrap();
        let data_dir = dir.path().join("data");
        let machining_dir = data_dir.join(Profile::Machining.dir_name());
        fs::create_dir_all(&machining_dir).unwrap();
        let previous = r#"schema_version: 3
id: 018f0000-0000-7000-8000-0000000000aa
name: Legacy routing
steps:
  - name: Step 1
    operations: [drill_pth]
    routing: { cut_depth_strategy: automatic, multi_pass_max_depth: 1.0mm }
"#;
        fs::write(machining_dir.join("legacy.yaml"), previous).unwrap();

        let (data, errors) = AppData::load_from(&data_dir, &dir.path().join("catalogs"));
        assert!(
            errors.iter().all(|e| !format!("{e:?}").contains("routing")),
            "the retired routing block must not surface as a load error: {errors:#?}"
        );
        let loaded = data.list(Profile::Machining);
        assert_eq!(loaded.len(), 1, "the profile should still load");
        let doc = loaded[0].1;
        assert!(
            doc.root.get_pointer("/steps/0/routing").is_none(),
            "the retired block should have been dropped, not carried forward"
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

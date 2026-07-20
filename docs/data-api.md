# K2G Central Data API (`AppData`)

Status: Design proposal for the data-layer refactor. Complements
[architecture.md](architecture.md). Once approved and implemented, this becomes
the canonical reference for how the application owns, validates, and persists all
configuration data.

## 1. Purpose

Introduce a single facade — `AppData` — that owns **all** persisted application
data through the [`datastore`](../crates/datastore) crate, replacing the bespoke
`config` + `ctx` persistence layers.

It manages:

- **Settings** (singleton)
- **Stock** (singleton)
- **CNC / Fixture / Toolset / Machining** profiles (per-file collections)
- **Tools catalog** (read-only collection)
- **CNC templates** (bundled seeds for creating new CNC profiles)

## 2. Principles

1. **One facade.** The UI and `Context` talk only to `AppData`; nobody touches
   files, schemas, or the writer directly.
2. **Schema-driven.** Every datum is validated, defaulted, unit-decoded, and
   reference-resolved by `datastore` from the YAML JSON-Schemas in
   [resources/schemas/](../resources/schemas). No hand-rolled parsing.
3. **No duplicate persistence.** `datastore`'s background, coalescing, atomic
   writer replaces [`config/persistence.rs`](../src/config/persistence.rs) and
   [`config/manager.rs`](../src/config/manager.rs). Edits *are* saves.
4. **Annotated tree is the model.** The UI renders forms from the `Meta` on each
   `Node` (title, description, kind, constraints, required, default-applied)
   instead of bespoke structs like `MachineProfile`.

## 3. Data taxonomy

| Data | Schema | Shape | `datastore` mechanism |
|---|---|---|---|
| Settings | `settings.yaml` | singleton, no id | one document at a fixed path; `set_value(path, …)` |
| Stock | `stock.yaml` | singleton, tool array, refs → catalog | one document; `add_item` / `clone_item` |
| CNC | `cnc.yaml` | per-file, id'd | `create/clone/edit/remove_document` |
| Fixture | `fixture.yaml` | per-file, id'd | same |
| Toolset | `toolset.yaml` | per-file, id'd | same |
| Machining | `machining.yaml` (was `processing.yaml`) | per-file, id'd | same |
| Catalog | `catalog.yaml` | read-only, many files, tools id'd | `parse_directory`; never edited |
| CNC templates | `cnc.yaml`-shaped, **no id** | bundled seeds | `create_document_from` (see §6) |

The **global identity registry** in `datastore` spans all documents, so a stock
item's `ref` resolves to the catalog tool that owns that UUID automatically, and
`remove_document` refuses (with `RemoveError::InUse`) to delete a profile still
referenced by settings or a job.

## 4. On-disk layout

During migration `AppData` operates **in place** on the existing `configs/`
tree (`AppData::load` → `dirs.configs`), where the legacy layer also keeps its
files, so migrated screens edit the user's real profiles with **no data
migration**. The dir names already line up (`cnc_profiles`, `fixture_profiles`,
`toolset_profiles`, `global.setting.yaml`, `stock.yaml`); coexistence is kept
safe by single-writer-per-realm discipline (a realm is written by exactly one
layer at a time).

```
%APPDATA%\k2g\
  schemas\                     # reference copies of the JSON-Schemas
  catalogs\                    # read-only tool catalogs  → catalog.yaml
  configs\
    global.setting.yaml        # singleton                → settings.yaml
    stock.yaml                 # singleton                → stock.yaml
    cnc_profiles\<uuid>.yaml         → cnc.yaml
    fixture_profiles\<uuid>.yaml     → fixture.yaml      (migrated)
    toolset_profiles\<uuid>.yaml     → toolset.yaml
    processing_profiles\<uuid>.yaml  → machining.yaml   (on-disk dir still
                                       `processing_profiles`; AppData reads
                                       `machining_profiles` — reconcile when the
                                       machining screen migrates, see §7)
```

CNC templates ship **inside the binary** (`include_str!` from
[resources/cnc_templates/](../resources/cnc_templates)); they are not user files.

## 5. The `AppData` API

```rust
pub struct AppData {
    store: ResolvedStore,          // live, auto-persisting
    cnc_templates: Vec<Template>,  // parsed bundled seeds
    settings_path: PathBuf,
    stock_path: PathBuf,
}

/// The four id'd, per-file profile collections.
pub enum Profile { Cnc, Fixture, Toolset, Machining }

impl AppData {
    /// Compile schemas, load every collection + singleton, resolve references.
    /// Returns all non-fatal problems for the diagnostics banner.
    pub fn load(dirs: &AppDirs) -> (Self, Vec<DataError>);

    // ---- singletons -------------------------------------------------------
    pub fn settings(&self) -> &Document;
    pub fn set_setting(&mut self, pointer: &str, value: NodeValue);
    pub fn stock(&self) -> &Document;
    pub fn add_stock_item(&mut self) -> Option<usize>;
    pub fn clone_stock_item(&mut self, index: usize) -> Option<usize>;

    // ---- collections ------------------------------------------------------
    pub fn list(&self, p: Profile) -> Vec<(Uuid, &Document)>;
    pub fn get(&self, id: Uuid) -> Option<&Document>;
    pub fn create(&mut self, p: Profile) -> Result<Uuid, FactoryError>;   // schema defaults
    pub fn clone(&mut self, id: Uuid) -> Result<Uuid, FactoryError>;
    pub fn edit(&mut self, id: Uuid, f: impl FnOnce(&mut Document));
    pub fn set_field(&mut self, id: Uuid, pointer: &str, value: NodeValue);
    pub fn remove(&mut self, id: Uuid) -> Result<(), RemoveError>;

    // ---- catalog (read-only) ---------------------------------------------
    pub fn catalogs(&self) -> impl Iterator<Item = &Document>;
    pub fn tool(&self, handle: Handle) -> Option<&Node>;  // follow a stock ref

    // ---- CNC templates ----------------------------------------------------
    pub fn cnc_templates(&self) -> &[TemplateInfo];               // (key, name)
    pub fn create_cnc_from_template(&mut self, key: &str) -> Result<Uuid, FactoryError>;

    // ---- lifecycle --------------------------------------------------------
    pub fn flush(&self);                       // block until writes drain (shutdown)
    pub fn write_errors(&self) -> Vec<WriteError>;
}
```

`AppData` replaces `AppState`'s persistence fields, `PersistRealm`, the
`save_*` free functions, and `PersistenceWriteManager` entirely.

## 6. CNC templates — `datastore` additions

A CNC template ([genmitsu_3018.yaml](../resources/cnc_templates/genmitsu_3018.yaml))
is a `cnc.yaml`-shaped document **with no `id` and no `schema_version`**.
"Inject a template into a new profile" = *parse the seed → fill defaults → assign
a fresh UUID → store as a new file*. This is `create_document` seeded from a
source, and belongs beside the crate's existing `instantiate` / `create_document`
/ `clone_document` vocabulary:

```rust
impl DataStore {
    /// Unattached instance: schema defaults overlaid with `seed`, fresh ids.
    pub fn instantiate_from(&self, schema_id: &str, seed: &Value) -> Option<Node>;
}
impl ResolvedStore {
    /// `instantiate_from`, stored one-file-per-instance; returns the new id.
    pub fn create_document_from(&mut self, schema_id: &str, seed: &Value)
        -> Result<Uuid, FactoryError>;
}
```

**Semantics**

1. Start from the schema factory value (defaults + `const`, so `schema_version`
   is materialised even though the seed omits it).
2. Deep-overlay the `seed` object onto that value.
3. **Regenerate every identity** (ignore any `id` in the seed), exactly like
   `clone_with_new_ids`.
4. `annotate` (not `parse_document`) to build the tree, then persist.

**Why not `parse_document`:** [parse.rs:76](../crates/datastore/src/parse.rs#L76)
version-gates on the *raw* `schema_version` before defaults are applied, so a
version-less seed would be rejected. The factory/`annotate` route materialises
`schema_version` from its `const` first. This is why templates correctly stay
id-less and version-less.

**App side** then reduces to: parse each bundled template once into a `Value`,
keep `(key, name)` for the picker, and call
`store.create_document_from("cnc.yaml", &seed)`.

## 7. `processing` → `machining` rename

Recommended to do **with** this refactor so the facade ships with final names:

- Schema: `resources/schemas/processing.yaml` → `machining.yaml`; `$id:
  "processing.yaml"` → `"machining.yaml"`.
- Data dir: `configs/processing_profiles/` → `configs/machining_profiles/`
  (one-time folder migration on load, or read the old name as a fallback).
- Code identifiers: `process_profile`, `processing`, `JobProfile` naming, the
  `resolve_schema_path` map in [manager.rs:487](../src/config/manager.rs#L487),
  and `user_path::AppDirs::processing_profiles`.
- Existing user files carry `schema_version: 2`; the rename does not change the
  data shape, only the schema `$id` and folder.

## 8. Migration from the legacy layer

Deleted / absorbed once `AppData` lands:

- [config/manager.rs](../src/config/manager.rs) `YamlConfigManager` — replaced by
  `datastore` parse/validate/default.
- [config/persistence.rs](../src/config/persistence.rs) `PersistenceWriteManager`,
  `PersistSession`, `save_*` — replaced by the `datastore` writer.
- [domain/catalog.rs](../src/domain/catalog.rs) `CatalogManager` — replaced by a
  read-only `catalog.yaml` collection (tools become resolvable ref targets).
- `AppState` persistence fields + `PersistRealm` + `sync_from_app_state` —
  replaced by reads off the annotated `Document` tree.

`Context` keeps orchestration (generation trigger/readiness, PCB, render
counter) and delegates every data read/mutation to `AppData`.

## 9. Open decisions (recommendations in **bold**)

1. Template-seeding home: **add `instantiate_from`/`create_document_from` to
   `datastore`** vs. app-side wiring.
2. Rename timing: **do `processing → machining` now** vs. defer.
3. Directory names: **keep existing `*_profiles` dirs via `set_collection_dir`**
   vs. adopt `datastore`'s `<stem>/` convention (would migrate folders).

## 10. Testing

- **Templates valid:** parse every bundled template against `cnc.yaml` with an
  injected `id`/`schema_version`; assert zero errors + `Complete`. (Prototyped
  and passing; move into the `k2g` crate once `datastore` is a dependency.)
- **`create_from_template` round-trip:** create → assert new id, name preserved,
  `Complete`, file written at the collection path.
- **Ref integrity:** `remove` a referenced profile → `RemoveError::InUse`.
- **Singletons:** `set_setting` persists and re-loads identically.
- **`AppData::load`** on a fresh dir seeds defaults; on a corrupt file surfaces a
  `DataError` without aborting the load.
```

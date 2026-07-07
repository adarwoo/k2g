# CAM Configuration Schemas

This directory contains JSON Schema files written in YAML for the CAM application's persistent configuration and externally authored data files.

The schemas are intentionally split by responsibility:

- **environment schemas** describe user/application-managed objects such as CNC definitions, fixtures, toolsets, and stock;
- **external schemas** describe read-only manufacturer/retailer data such as catalogs;
- **shared schemas** define reusable identifiers and units;
- **processing schemas** bind the environment together and describe default machining/generation behavior.

All schemas are stored as `.yaml` files and use short `$id` values so local references remain simple, for example:

```yaml
$ref: "units.yaml#/$defs/size"
$ref: "id.yaml#/$defs/uuid_v7"
```

---

## Design Principles

### 1. Preserve native user input

Dimensional and machining values are stored as unit-bearing strings whenever possible. This preserves the value as entered or supplied:

```yaml
diameter: "1/32\""
z_feed: "120 mm/min"
spindle: "24000 rpm"
```

The application may parse and normalize values internally, but persisted YAML should keep the original expression where possible.

### 2. Shared units are centralized

All size, feed, angle, RPM, percentage, and cutting-speed validation is shared through `units.yaml`. Individual schemas should not duplicate unit regexes.

### 3. Application-managed objects use UUIDv7

Mutable objects owned by the application use UUIDv7 identifiers:

- CNC definitions
- fixture definitions
- processing definitions
- stock tools
- toolsets
- catalog tool entries when referenced by stock/project files

### 4. Catalogs are read-only

Catalogs are externally authored by manufacturers, distributors, retailers, or external generation tools. The application may validate, index, and display them, but should not normally rewrite them.

### 5. Stock items are catalog-derived but user-customizable

A stock item keeps:

- its own application-managed UUIDv7;
- a reference to the originating catalog tool UUIDv7;
- a `base` snapshot copied from the catalog;
- local `overrides` applied on top.

Effective stock value resolution should be:

```text
current catalog item, if available
else stock.base snapshot
then apply stock.overrides
```

This means a catalog-derived stock item can become orphaned if the catalog is missing, but it remains operational.

### 6. Missing project dependencies should become ghost objects

When opening a project, if referenced environment objects are missing, the application should create temporary **ghost objects** from the persisted project snapshots.

A missing CNC, fixture, toolset, stock tool, or catalog item should not immediately fail project loading and should not automatically mutate the user's environment.

The user should explicitly decide whether to:

- keep using the ghost object for this project;
- replace it with an existing real object;
- import the ghost object into the environment;
- re-link it to a catalog/stock/environment object.

---

## Schema File Overview

### `id.yaml`

Shared identifier definitions.

Defines reusable ID formats such as:

```yaml
$ref: "id.yaml#/$defs/uuid_v7"
```

Use UUIDv7 for persistent application-managed objects.

Typical consumers:

- `cnc.yaml`
- `fixture.yaml`
- `processing.yaml`
- `stock.yaml`
- `toolset.yaml`
- `catalog.yaml` catalog tool IDs

---

### `units.yaml`

Shared unit validators.

Defines reusable unit-bearing or unit-implied value types such as:

```yaml
$ref: "units.yaml#/$defs/size"
$ref: "units.yaml#/$defs/feed"
$ref: "units.yaml#/$defs/rpm"
$ref: "units.yaml#/$defs/angle"
$ref: "units.yaml#/$defs/percent"
$ref: "units.yaml#/$defs/percent_50_100"
```

Typical accepted values include:

```yaml
size: "2.5 mm"
size: "1/8\""
size: "10 thou"
feed: "1200 mm/min"
feed: "50 ipm"
rpm: 24000
rpm: "24000 rpm"
angle: 118
angle: "118 deg"
percent: "8%"
```

Rules:

- units belong in `units.yaml`, not in field names;
- field names should avoid suffixes like `_mm`, `_deg`, `_percent`;
- bare RPM and angle values are allowed because they are unambiguous in context.

---

### `catalog.yaml`

Read-only tool catalog schema.

Catalogs are externally authored and contain manufacturer/retailer tool definitions grouped into sections.

A catalog tool should have a stable UUIDv7 `id`, which can be referenced by stock and project files:

```yaml
id: "01890fdb-4daf-7a37-8f6a-9dc397e5b4ef"
type: drillbit
diameter: "0.8 mm"
point_angle: "130 deg"
z_min_depth: "1.6 mm"
spindle_rpm: "24000 rpm"
z_feed: "120 mm/min"
```

Important behavior:

- catalog files should not normally be edited by the application;
- catalog values are preserved as authored;
- additional manufacturer metadata is allowed;
- catalog tool UUIDs are source references, not user stock IDs.

---

### `stock.yaml`

User tool stock inventory.

Stock tools are mutable user/application-managed objects. Each stock item references a catalog item, stores a persisted base snapshot, and optionally stores local overrides.

Typical structure:

```yaml
tools:
  - id: "01890fdb-4daf-7a37-8f6a-9dc397e5b4ef"
    ref:
      catalog: "kyocera"
      tool_id: "0194fd2c-5f2e-7a9d-9a76-3b2b9fbc3f11"
      section: "Series 100 Micro Drills"
      sku: "KYO-0.80"
    base:
      name: "0.8 mm carbide drill"
      kind: drillbit
      diameter: "0.8 mm"
      point_angle: "130 deg"
      z_min_depth: "1.6 mm"
      spindle: "24000 rpm"
      z_feed: "120 mm/min"
    overrides:
      z_feed: "90 mm/min"
    availability: in_stock
    preference: neutral
```

Important behavior:

- if the catalog exists, current catalog value + overrides defines the effective tool;
- if the catalog is missing, `base` + overrides defines the effective tool;
- removing an override reverts to catalog if linked, otherwise to `base`;
- missing catalog does not make the tool read-only.

---

### `cnc.yaml`

CNC/machine profile schema.

Defines physical machine configuration and GCode/RHAI primitive templates.

Typical responsibilities:

- usable fixture plate size;
- maximum feed rate;
- spindle min/max speed;
- ATC slot count;
- origin orientation;
- scaling multipliers;
- line numbering;
- numeric resolution;
- GCode primitive templates.

Primitive templates are evaluated through the application's RHAI expression path.

Unit-rendering functions should use system names:

```rhai
use_metric()
use_imperial()
```

This is preferred over `use_mm()` or `use_inch()` because it switches the whole output unit system, not only one unit spelling.

---

### `fixture.yaml`

Fixture configuration schema.

Defines how the board is held and how the work reference is established.

Typical responsibilities:

- fixture ID and name;
- board holding method;
- backboard / martyr-board thickness;
- work origin reference;
- locating pins;
- keep-out or clamp zones;
- fixture occupancy;
- probing/alignment parameters.

Fixture is also the natural place for physical safety constraints, such as maximum allowed PCB exit depth or minimum bed clearance, if those are added later.

Processing should not own machine-bed safety limits; the fixture knows what exists below the PCB.

---

### `toolset.yaml`

Toolset management schema.

Defines how tools are managed for a project or process:

- logical slot layout `T1..Tx`;
- fixed/spare/do-not-use slot modes;
- generation policy when tools exceed available slots;
- optional tags.

A fixed slot references a stock/project tool by UUIDv7.

Typical behavior:

- `fixed_toolset`: generation fails if required tools exceed usable slots;
- `allow_reload`: generation may pause for toolset reloads;
- `allow_hybrid`: generation may transition from ATC to manual tool changes.

---

### `processing.yaml`

Processing defaults and bindings.

Defines what needs to be processed and how generation defaults are applied.

It binds together:

- CNC
- fixture
- toolset

Each binding has:

```yaml
default: "<uuidv7>"
choices: any
```

or:

```yaml
default: "<uuidv7>"
choices:
  - "<uuidv7>"
  - "<uuidv7>"
```

Short labels are used to keep configuration readable:

```yaml
cnc
fixture
toolset
operations
strategies
holes
edge
finishing
```

Processing includes:

- enabled operations;
- default machining strategy extensions;
- hole/tool matching rules;
- oblong hole strategy;
- board edge cutting strategy;
- optional finishing pass behavior.

Hole matching uses a relative + maximum cap rule:

```yaml
holes:
  oversize:
    relative: "8%"
    max: "0.20 mm"
  undersize:
    relative: "3%"
    max: "0.05 mm"
```

Interpretation:

```text
allowed_difference = min(required_diameter * relative, max)
```

This avoids oversized tool substitutions for microvias while still allowing larger holes reasonable absolute flexibility.

---

### `settings.yaml`

Application settings schema.

This schema is expected to contain user/application preferences rather than project-specific or manufacturer data.

Recommended contents include:

- default directories;
- default preferred unit system;
- UI preferences;
- recently used profiles or catalogs;
- validation strictness;
- import/export behavior;
- optional migration/version settings.

Settings should not duplicate CNC, fixture, stock, or toolset definitions.

---

## Should There Be a `project.yaml` Schema?

Short answer: **yes, but it should probably be generated from Rust/Serde rather than handwritten in full.**

The project file is the most complex persisted object because it is effectively a portable snapshot of everything needed to reopen, inspect, regenerate, and safely reason about a job.

A project should likely contain:

- project metadata;
- PCB/job input references and extracted board metadata;
- selected processing configuration;
- resolved CNC/fixture/toolset references;
- snapshots of CNC/fixture/toolset used at generation time;
- required tool list;
- stock/catalog references;
- full tool snapshots;
- generated operation plan;
- user overrides;
- warnings or resolution state;
- migration/version information.

That is a lot to maintain manually as JSON Schema.

### Recommended approach

Use Rust structs as the source of truth and generate a project schema from Serde-compatible types:

```text
Rust structs + Serde
        ↓
application persistence
        ↓
generated JSON Schema for documentation/validation/import tests
```

A hand-written project schema would be large, fragile, and likely to drift from the actual Rust persistence model.

For Rust, a common approach is:

- define all persisted project data with `serde::{Serialize, Deserialize}`;
- derive or generate JSON Schema from the same structs, for example using a schema generation crate;
- version the project format explicitly;
- implement migrations in Rust;
- keep the generated project schema as an artifact for documentation, tests, and external tooling.

### What the project schema should validate

The project schema should validate structure, not all application semantics.

Good schema-level validation:

- required fields exist;
- UUIDs have UUIDv7 format;
- unit-bearing strings are valid;
- arrays/objects have expected shapes;
- enum values are valid;
- snapshots contain enough data to create ghost objects.

Application-level validation should handle:

- whether UUIDs resolve in the local environment;
- whether a ghost object is required;
- whether a replacement is compatible;
- whether a selected tool can actually machine the requested operation;
- whether fixture safety and tool geometry allow generated GCode;
- whether defaults are included in allowed choices.

### Recommended project dependency model

Project files should store references plus snapshots:

```yaml
cnc:
  id: "<uuidv7>"
  snapshot:
    # CNC data used when the project was saved/generated

fixture:
  id: "<uuidv7>"
  snapshot:
    # Fixture data used when the project was saved/generated

toolset:
  id: "<uuidv7>"
  snapshot:
    # Toolset data used when the project was saved/generated

tools:
  - stock_id: "<uuidv7>"
    catalog_ref:
      catalog: "kyocera"
      tool_id: "<uuidv7>"
    snapshot:
      kind: drillbit
      diameter: "0.8 mm"
      point_angle: "130 deg"
      z_min_depth: "1.6 mm"
      spindle: "24000 rpm"
      z_feed: "120 mm/min"
```

When loading:

```text
if environment object exists:
    link to real object
else:
    create ghost object from project snapshot
```

Opening a project should not automatically import missing objects into the user's environment. Ghosts should be project-local until the user explicitly imports, replaces, or relinks them.

---

## Recommended Validation Layers

### Schema validation

Run on file load for:

- syntax correctness;
- structural correctness;
- unit string validation;
- UUID/string/enum validation.

### Semantic validation

Run in the application after schema validation:

- default profile is included in choices when choices is an array;
- referenced UUIDs exist or ghost objects can be created;
- tool geometry is compatible with requested operations;
- fixture safety constraints are respected;
- stock/catalog references can be resolved or marked as orphaned;
- RHAI/GCode primitive templates compile and evaluate.

### Generation validation

Run immediately before GCode generation:

- all required operations have tools;
- selected tools can physically perform the operation;
- computed drill depths and point-angle compensation are safe;
- motion does not violate fixture keep-out zones;
- spindle/feed values are within CNC limits;
- output unit system is correct.

---

## Migration and Versioning

Consider adding a simple version field to all user/application-managed schemas:

```yaml
version: 1
```

For project files, versioning is strongly recommended:

```yaml
format_version: 1
application_version: "..."
```

Use Rust migrations to upgrade old project files into the current in-memory model.

Recommended rule:

> Schemas validate persisted shape; Rust migrations preserve compatibility.

---

## Naming Guidelines

Use short names when they remain clear:

```yaml
cnc
fixture
toolset
holes
edge
finishing
```

Avoid encoding units in field names:

```yaml
# Prefer
point_angle
vgroove_depth
oversize

# Avoid
point_angle_deg
vgroove_depth_percent
oversize_mm
```

Avoid using `profile` in field names unless it adds clarity. A schema named `cnc.yaml` can expose `cnc` rather than `cnc_profile`.

---

## Practical Recommendation

Keep the current individual schemas handwritten and readable:

- `catalog.yaml`
- `cnc.yaml`
- `fixture.yaml`
- `id.yaml`
- `processing.yaml`
- `settings.yaml`
- `stock.yaml`
- `toolset.yaml`
- `units.yaml`

For `project.yaml`, prefer **Rust/Serde as the authoritative model** and generate the schema from the Rust structs.

That gives you:

- no drift between code and schema;
- high confidence in project persistence;
- better migration support;
- a schema artifact for documentation, tests, and external validation;
- freedom to keep the project model complex without hand-maintaining a huge YAML schema.

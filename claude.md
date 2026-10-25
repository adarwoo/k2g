# CLAUDE.md

k2g is a computer-aided machining application. It generates the CNC machine code (G-code)
to machine a PCB designed in KiCad: copper isolation, drilling (PTH/NPTH, oblong slots,
locating pins), and routing (cutouts, and the board outline with retaining tabs), on one
face or both. It reads the board **live from a running KiCad** over KiCad's IPC API, so
nothing is exported or re-imported. It is written in Rust, with a Dioxus 0.7 desktop UI,
and licensed GPL-3.0-only.

- The project specification is in `docs/design/Specification.md`.
- The architecture is in `docs/design/architecture.md`. Its *Entity model (as built)* and
  *Program algorithm structure* sections are current. The *Context*, *Viewers* and
  *ConfigPipeline* sections describe an earlier design that the code no longer follows.
- Deeper design docs live in `docs/design/`: `operation-planner.md`, `gcode-engine.md`,
  `gcode-template-language.md`, `gcode-generation.md`, `data-api.md`,
  `schema-versioning.md` and `validation-plan.md`.
- User-facing docs are `docs/user-manual.md` (also compiled into the app's Manual screen)
  and `CHANGELOG.md`.

## Documentation rule

All generated code is properly documented, **private functions included**. Every item
gets a doc comment that says what it is for and why it exists. Match the existing style.
Comments explain *why*: the reasoning behind a decision, the failure that motivated it,
or the trap that made the obvious approach wrong. They don't restate what the code does.
Read a few neighbouring comments before writing new ones.

## Commands

```sh
cargo build --workspace
cargo test --workspace
cargo check --workspace --all-targets --release   # not redundant, see below
cargo run                                         # default-run = k2g; RUST_LOG=debug for verbose logs
```

Running a subset of tests. The app is a bin-only crate named `k2g`, and nearly all tests
are inline `#[cfg(test)]` modules, so filter by test path:

```sh
cargo test -p k2g gcode::planner         # substring filter
cargo test -p pcb isolation
cargo test -p datastore --test persist   # only datastore has tests/ integration files
```

- **Always run the release check.** Dioxus `rsx!` expands differently without
  `debug_assertions`, so markup that compiles in debug can fail in release.
- **Build prerequisites.** CMake is required, because `nng-sys` builds the bundled nng C
  library with it. Linux also needs the GTK3 and WebKitGTK 4.1 dev packages; the list is
  in `.github/workflows/rust.yml`.
- **Formatting and lints.** `cargo fmt` and `clippy` run in CI but don't gate. The
  workspace carries known warnings, several of them in the vendored fork. Don't reformat
  files you aren't otherwise changing.
- **Schema docs.** `cargo run -p xtask` renders `schemas/*.yaml` into `docs/SCHEMAS.md`,
  which is not committed.
- **Releases.** Installers are built only in release CI, by `dx bundle` (dioxus-cli
  0.7.10). `version` in `Cargo.toml` is the single source of the version. Tags are
  `vX.Y.Z-codename`, and CI refuses a tag that doesn't match `Cargo.toml`.
- **Single instance.** k2g takes a per-user lock on its data directory. If another k2g is
  already running (an installed build, say), `cargo run` brings that window forward and
  exits without launching yours. On Windows a running `k2g.exe` also blocks the linker
  from replacing the file.

## Workspace

| Path | Role |
|---|---|
| `crates/units` | Typed quantities (`Length`, `FeedRate`, `Angle`, `RotationalSpeed`) that keep the form they were written in, so `1 1/8"` round-trips. Holds the single rounding and formatting code for both on-screen values and emitted G-code (`units::user_format`, `units::machine`). |
| `crates/datastore` | Generic schema-driven store. Validates YAML against JSON Schema, applies defaults, decodes unit and UUID fields, and resolves `x-ref` references. **Editing a document is saving it**: each edit queues an atomic write-back on a background thread, and edits to the same file are merged into one write. Contains nothing specific to k2g. |
| `crates/gtl` | Generic Template Language: Rhai in which a line starting with a backtick is an *emit line* with `{expr}` interpolation. Emits strings and knows nothing about G-code. |
| `crates/pcb` | **The only crate that knows KiCad** (and clipper2). Covers board acquisition (`KiCad`), the `BoardSnapshot` record, stitching edge shapes into nested `Contour`s, tool-radius offsets, copper extraction, isolation contours and island clearing. |
| `third_party/kicad-ipc-rs` | Vendored fork, patched in through `[patch.crates-io]`. It recovers geometry that upstream drops (pad angle, polygon vertices, zone fills, per-layer pad stacks); `Cargo.toml` explains why each patch exists. Keep it close to upstream. |
| `xtask` | Schema documentation generator. |
| `src/` | The application: the `k2g` binary, plus `k2g-kicad-launcher`, the shim that KiCad's toolbar button runs. |

## Application architecture (`src/`)

Startup in `main.rs`, in order:

1. Answer `--version` or `--help` and exit.
2. Start logging.
3. Take the single-instance lock.
4. Repair the KiCad plugin registration if it points at an old build.
5. Read the board with `runtime::acquire_board()`: at most one board, because KiCad serves
   a single API socket.
6. Start the UI with `ui::launch`.

- **`data/`** – `AppData` is the only reader and writer of persisted data: settings,
  stock, the job, and the CNC, fixture, toolset and machining profile collections. Go
  through `with_appdata` / `with_appdata_mut`. The schemas and the bundled templates are
  `include_str!`'d here, and old files are migrated before they are parsed (for example
  `normalize_machining_value`). `data/model/` holds the typed model.
- **`runtime/`** – live state and orchestration.
  - A global `AppCtx` (`GLOBAL_CTX`, an `RwLock`) holds `AppState` plus the stitched
    board, copper, isolation results and the plan cache.
  - **`state.rs`, `orchestration.rs`, `catalogs.rs` and `generation.rs` are `include!`d
    into `runtime/mod.rs`.** They are not modules: they share `mod.rs`'s imports and
    namespace.
  - `tooling.rs` and `machining_plan.rs` read each machining step from the datastore
    (`/steps/{i}/…`) and call the pure `gcode` functions.
  - Also here: `isolation` (worker thread), `update` (the release check), `kicad_integration`,
    `removable` (save to a USB key or SD card, then eject), `security_log` and
    `single_instance`.
- **`gcode/`** – pure, deterministic machining logic with no I/O and no context access.
  - `assigner` picks tools and shrinks the tool set to fit the rack.
  - `placement` maps board coordinates to machine coordinates.
  - `planner` turns the job into atomic operations, grouped into per-tool blocks and
    ordered by a TSP pass.
  - Toolpath geometry is in `routing`, `outline`, `oblong`, `pins` and `testcut`, with
    `arcfit` (fits arcs back onto offset polylines) and `feeds` beside them.
  - `program` assembles a step's program, and `coder` + `dialect` are the G-code layer
    over `gtl`.
  - `scene` builds the 3D view's geometry, so the JS only owns the camera.
- **`ui/`** – Dioxus desktop.
  - `screens/mod.rs` holds `AppRoot`.
  - `screens/job/` holds the Board (SVG), Machining, 3D (vendored three.js from
    `assets/vendor`, injected into the page head), Code, Tooling and Rack tabs.
  - `bindings.rs` renders forms from the schemas (`SchemaForm`, `use_field`); its writers
    call `bump_render()`.
  - `theme.rs` is the entire stylesheet, as one CSS string (`APP_STYLE`).
  - `help.rs` compiles `docs/user-manual.md` into the app.
- **`catalog_io/`** – loads and normalises tool catalogs. Catalogs are the only persisted
  data still read outside `AppData`.

### From board to program

1. `pcb` returns a `BoardSnapshot` and the copper layers. `AppCtx` stitches the edge once
   per acquisition.
2. The **job** is a singleton (`job.yaml`) that names one **machining profile**.
   - A machining profile is an ordered list of **steps**. Each step is one physical
     setup: exactly one CNC, one fixture and one toolset, plus its operations.
   - The operations are `isolate_traces`, `clear_copper`, `drill_locating_pins`, `drill_pth`, `drill_npth`,
     `route_cutouts` and `route_board`.
   - A step with a missing binding is refused by name, never given a default.
   - The runtime still keeps a step-0 projection (`JobProfile`) for older code paths.
     Per-step data comes from the document.
3. Every mutation goes through `with_ctx_mut`, which clones `AppState` first and then runs
   `AppCtx::sync_after_mutation`:
   1. Re-stitch the board edge if needed.
   2. `validate_tooling`.
   3. The readiness gate, `evaluate_generation_readiness`.
   4. Trigger detection, which fingerprints everything the job references.
   5. `enqueue_generation`.
4. `runtime::tooling` runs the assigner for each step. `runtime::machining_plan` then
   builds the `MachiningPlan`. Block order is drilling → contouring, with the copper
   blocks (engraving, then clearing) either before the drilling or after it, as the step
   lists its operations. The rules for a step's order are `order_fault` in
   `data/model/operations.rs`; the placement is the push order in `plan_step`, guarded by
   `block_order_tests`.
5. A `GenerationInput` snapshot goes to a worker thread that runs one generation at a
   time. A new request cancels the running one, and stale results are discarded by
   generation id.
   - Each step renders **its own program** through its own CNC's templates
     (`program::render_step_program`). A failing step doesn't block the others.
   - Results are published into the context, and a wake channel refreshes the UI.

Isolation contours are computed by a second worker (`runtime::isolation`) that follows the
same rule: the newest request wins.

## Schemas and persisted data

- **Schemas define files and forms.** `schemas/*.yaml` are JSON Schema written in YAML.
  They define the persisted files and much of the UI, since forms are generated from them.
  Read `architecture.md` before changing one.
- **Schemas are compiled in** (`SCHEMAS` in `src/data/mod.rs`), and a test fails if a file
  in `schemas/` is missing from that list. The copies k2g writes into the user's data
  directory are for reference only, never read back, and overwritten at every launch.
- **k2g's schema keywords** are ignored by the validator:
  - `x-ref`: a UUID reference to another document.
  - `x-enum-labels`: display labels for enum values.
  - `x-show-when`: hides a field unless a sibling has a given value. Display only, never
    validation.
  - `x-schema-version`: the schema's version (see the next bullet).
  - CNC primitives in `cnc.yaml` use `x-kind` (generator, callable or filter), `x-category`,
    `x-variables` (the variables a template can use; the single source for that list) and
    `x-fallback`.
- **Schema version bumps need a migration.** The datastore rejects any file whose
  `schema_version` differs from its schema, whether older or newer. The migration has to
  rewrite the document, including its `schema_version`, *before* parsing; see
  `docs/design/schema-versioning.md`.
- **Bundled data.** The templates and catalogs in `assets/` are `include_str!`'d, so the
  binary runs with no files beside it. User data lives in `%APPDATA%\k2g`, `~/.config/k2g`
  or `~/Library/Application Support/k2g`.
- **No G-code is hardcoded in Rust.** Every word k2g emits comes from a CNC profile's GTL
  templates (`docs/design/gcode-template-language.md`), so an unusual controller is a new
  profile, not a code change.
- **Catalog numbers must be sourced.** Every value in a tool catalog must come from a
  vendor datasheet or a measurement, never a guess (`CONTRIBUTING.md`).

## Conventions and traps

- **Tests.** Add the test that fails before the fix and passes after, inline in the same
  file. Where the compiler can't catch a harmful reordering, the codebase uses
  source-scan tests (`main.rs` `argument_tests`, `block_order_tests`). Keep them passing;
  don't weaken them.
- **Units.** Use the `units` types, not bare `f64` millimetres. Board geometry is integer
  nanometres. Equality on unit types is structural, so
  `Length::from_mm(10.0) != Length::from_cm(1.0)`; compare magnitudes through accessors
  such as `as_nm()`.
- **Coordinates.** KiCad's board frame is Y-down; the machine's is Y-up. The flip happens
  once, in `gcode::placement`. Don't flip anywhere else.
- **Determinism.** Planner and assigner output must be reproducible: no clock, no RNG and
  no `HashMap` iteration order (use `BTreeMap`). Diameters and fits are compared at 1 µm.
- **Machine safety.** These programs drive a real spindle. A missing feed rate, binding or
  tool is a hard failure that names what is missing, never a guessed default.
- **`AppState` is deep-cloned on every `with_ctx_mut`.**
  - Large data (copper, isolation results, the plan cache) sits on `AppCtx` behind `Arc`.
  - Frequent events (window geometry, removable-media polling) deliberately bypass
    `with_ctx_mut`.
  - Code that runs inside `with_ctx_mut` must not take the context lock again, or it
    deadlocks silently. `enqueue_generation` and `request_isolation` take no lock.
- **UI reactivity has two sources.**
  - `Signal<AppCtx>` is refreshed by `screens::mutate_ctx` and by the generation wake
    channel.
  - Datastore edits bump `bindings::data_revision()`.
  - A view that depends on both kinds of change must read both.
  - `use_effect` and `use_memo` register their closure once. Read signals *inside* the
    closure, or the view stays frozen at its first render.
- **WebView links.** Rendered Markdown has its links stripped: following an `<a href>`
  replaces the app with a web page in a window that has no Back button. Open external
  URLs with the `open` crate.
- **WebView script errors.** JavaScript errors in the page are caught by `ERROR_TRAP` in
  `ui/mod.rs` and copied into the log, since a failing script otherwise just leaves a
  blank canvas.
- **Privacy.** `runtime::update` makes k2g's only outbound network request. Don't add
  telemetry or other network calls (`PRIVACY.md`).
- **Stale module headers.** Some still describe shipped work as pending: `gcode/mod.rs`
  and `assigner.rs` say the assigner isn't wired in, and `plan.rs` / `planner.rs` call
  routing future work. Trust the code and `CHANGELOG.md`. The exception is
  `gcode/template.rs`, which really is unused work in progress, kept on purpose; don't
  delete it as dead code.
- **CHANGELOG.** User-visible changes go under `[Unreleased]` in Keep a Changelog format,
  written for the person running the machine.

# K2G Architecture and Technical Design

Status: Canonical technical and implementation reference.
Canonical product and UX requirements: Unified_Specification.md

## Core technology decisions

1. The application is written in the Rust programming language
2. The rust used is Rust 2024
3. All configuration files are stored as Yaml files
4. All types of Yaml have a schema file specified in Yaml

# Library and OSS selection
1. The frontend uses the Dioxus API
2. The connection to KiCAD uses kicad-ipc-rs
3. The internal scripting engine uses RHAI
4. The Yaml parsing and schema validation used the serde Rust crate

# UI
The graphical views uses svg

# Libraries
The application provides the follow libraries to the main application

## KiCAD connector
Wrapper to add scanning functionality to kicad-ipc-rs
The connector checks if the env variable is set, else, scan compatible pipes and presents a list

## Config manager
Manages all configurations
 1. Reads all required configuration into memory
 2. Validates all files and report errors
 3. Create local schema files
 4. Create default contents when error detected
 5. Move errored files if overwritting

## Stiching
Stiches lines, arcs, bezier curves into polygons to determine shapes
Ordered shapes to determine enclosing and enclosed
Creates a memory model of the shape, keeping the original artefacts.
The stiching algorithm is used for the purpose of determining line continuity and closed contours.
However, the stiching data results in closed and open contour objects, stored as an ordered collection of native geometry.
These objects are ordered lists of native geometry with a start and end point.
The start point is optional. If not specified, it is the end point of the previous item.

Rationale: Keeping native geometry allows using the closest Gx command of the CNC.
This is done during the program generation phase.
The stiching results in a hierachy of objects (A in B, A overlapping B, C in B etc.)

## Board
Object which stores the raw board information and stiched version.
The stiched version is on demand - just in time stiching.
The board object remains valid until a new board is loaded.

## Tool catalog
Manages the configured tool catalog and provide function to select the correct tool.

## Package and Dependency Architecture

The K2G project integrates multiple Rust crates, internal modules, and frontend frameworks. Three complementary D2 diagrams provide different views of the dependency structure:

### Dependency Diagrams

1. **[package-dependencies.d2](package-dependencies.d2)** — Comprehensive dependency graph
   - External crates (Tokio, Serde, Dioxus, etc.)
   - Internal Rust crates (k2g, kicad-ipc-rs)
   - K2G internal modules (CLI, Board, Catalog, Config, Stitching, UI, etc.)
   - Frontend applications (Dioxus Desktop, Next.js Web)
   - Specific module-to-crate relationships

2. **[package-architecture-high-level.d2](package-architecture-high-level.d2)** — Logical grouping of dependencies
   - Async Runtime (Tokio)
   - Serialization & Schema (Serde, JSON Schema, Protobuf)
   - Validation & Error Handling
   - UI Frameworks (Dioxus, Next.js)
   - Scripting & Processing (Rhai, Clipper2)
   - Tools & Utilities (Clap, Log, RFD, NNG)

3. **[module-dependencies.d2](module-dependencies.d2)** — Internal K2G module relationships
   - Main entry point orchestration
   - Inter-module communication flow
   - External crate usage per module
   - Module ownership and lifecycle

### Key Dependencies Summary

**External Crates:**
- **Async Runtime**: Tokio (v1) with full runtime features
- **Serialization**: Serde (derive, JSON, YAML support)
- **Schema Validation**: jsonschema (v0.28)
- **UI Frameworks**: Dioxus (v0.7, desktop) and Next.js (v16) for web
- **Scripting**: Rhai (v1) expression evaluation
- **IPC Communication**: NNG (v1.0.1) for KiCad connection
- **Protobuf**: Prost (v0.14) for message serialization
- **Geometry**: Clipper2-rust (v1.0) for polygon operations
- **CLI**: Clap (v4) for argument parsing
- **Utilities**: image (v0.24), log (v0.4), rfd (v0.15), thiserror (v2)

**Internal Structure:**
- `kicad-ipc-rs` (v0.4.0) — KiCad IPC API client (published crate, used by k2g with blocking feature)
- `k2g` (v0.1.0) — Main application integrating all modules and crates
- 9 core modules (cli, board, catalog, config, stitching, ui, rhai_parser, units, user_path)

# Operations

## Application initialisation
 * Load all configuration items
 * Load PCB data if possible and create a board object

### Configuration parsing
When the application starts it first parses all configuration files.

- Any error during parsing rejects that file and inserts a transient error in the log.
- If the errored file is internal (bundled), a diagnostic is displayed and the application terminates after the user acknowledges.
- External errored files are renamed by appending `.error` to the filename. If a `.error` file already exists it is deleted first.
- If the rename or delete fails, a transient error is added to the log.

### Loading the board
All items of interests are loaded into memory:
 * Holes
 * Pads
 * Existing locating holes
 * Tracks Top, Bottom and intermediate
 * Edge (outline)

The Edge layer is stiched.
The stiched edge is checked for error:
 * No overlapping contours allowed
 * Single ownership (B in A = OK) (C in B in A != OK)

A valid closed board outline is required before any generation can proceed.
If the outline is invalid, an error is reported and the board is not processed.

Note: trace routing is not processed in this version.

## Generation
The GCode generation is a background activity.
It can be stopped and restarted.
An atomic generation counter is used to indicate the generation value.
Everytime an impacting change occurs, the counter is incremented.
The generation loop must check that the current counter value being generated matches the
current counter value.
If not, the loop resets the generation.
The generation loop shall use thread pools for all RHAI interpretations and
non-interruptable computation activities associated with a generation ID.
On-going activities in the threadpool always go to completion.
When done, they re-integrate with the generation object using the ID.
If the ID is in the past, the outcome is discarted.
When the generation completes, the UI is updated.
When an new generation starts, the UI is updated. Generated items are grayed to indicate a refresh is in progress.

### Program algorithm structure

Generation produces output per project type in this order:

1. Drilling projects — all hole operations (PTH, NPTH, locating, pilot)
2. Contouring projects — routing, scoring, tabs, V-groove
3. Engraving — planned, not yet implemented

Within each project type, operation ordering uses a Travelling Salesman Problem (TSP) sort to minimise tool travel distance. An existing Rust TSP library is used.

### GCode primitives

All project types produce an ordered list of primitives. These are resolved to GCode by the CNC RHAI expressions in the final pass, decoupling the algorithm from machine-specific dialect.

Primitive set:

- `initialise`
- `move_slow(x, y)` — positioning move
- `start_spindle`
- `stop_spindle`
- `drill`
- `peck_drill`
- `cut_arc`
- `cut_bezier` — native on some CNCs (e.g. Siemens G3.4); resolved to arc approximation on others
- `change_tool`
- `conclude`

### Process profile
The process profile is split into 3 parts so frontend and backend concerns remain isolated.

1. JobProfileDefinition (persisted profile)
- Declarative object stored as YAML.
- Includes profile metadata, default settings, references to CNC and fixture profiles, and the list of supported operations.
- Includes an input schema describing editable attributes, types, defaults, ranges, enums, and validation rules.
- Includes a processor kind identifier used by the backend factory (for example: drilling, contouring, scoring).

2. JobInstance (runtime configured project)
- Created from JobProfileDefinition plus user overrides from the UI.
- Fully resolved immutable input for generation.
- Versioned with generation ID so stale compute results can be discarded safely.

3. JobProcessor (backend algorithm)
- Selected by processor kind through a factory.
- Pure backend component that takes JobInstance and board/context data.
- Produces operation primitives and final GCode.
- May provide preview geometry data, but never UI widgets or frontend state.

Frontend behavior:
- The UI does not embed project algorithms.
- The UI renders forms from the input schema and edits overrides only.
- Pan/zoom/rotate and other viewport behavior remain frontend view state and are not part of the process profile.

Testing boundaries:
- Unit-test JobProcessor with fixed JobInstance inputs and expected GCode/operations.
- Unit-test schema validation independently.
- UI tests only verify form rendering/editing from schema and correct submission of overrides.

### Context

The Context is the application orchestrator. It is the single root object that owns and coordinates all major subsystems. The UI layer (Dioxus) subscribes to Context and reads from it; it never drives logic directly.

Context owns:
- `configurations` — config pipeline results and active profiles
- `catalog` — tool catalog
- `pcb` — current board snapshot and stitched model
- `project` — the single active project and generation state (managed by Generator)
- `cnc` — active CNC profile resolved from the process profile
- `fixture` — active fixture profile resolved from the process profile
- `rack` - active tools rack configuration
- `renderer` — render adapter
- `render_counter` — monotonic counter driving reactive refresh in Dioxus

Context responsibilities:
- Receive mutations from the UI and delegate to the appropriate subsystem.
- Increment the generation/render counter on relevant mutations to trigger reactive refresh.
- Expose read-only query methods consumed by Dioxus signal mappings.
- Expose `parse_exp(expression)` to evaluate a RHAI expression using the active project property resolver.
- Route expression property/function resolution through the active project effective values (profile defaults plus runtime overrides).
- Hold no business logic itself; all logic lives in the owned subsystems.

Testing boundary:
- Context is integration-tested, not unit-tested.
- Each owned subsystem is unit-tested independently through its own API.

### RHAI parser

Context and project access for expressions:

- The application context must expose `parse_exp(expression)` which evaluates a RHAI expression and returns a typed result or structured error.
- The evaluator used by `parse_exp` resolves variables/functions through the active project object.
- The project object must provide property accessors for effective values (profile default or active override).
- Effective project values are the source of truth for expression evaluation; profile files are never mutated by runtime evaluation.

### Viewers
Viewers are library that takes a pcb object and render the object in SVG for display.
The are stateless methods provided as a separate library.
The viewer are accessed through the context (which has access to the PCB data - if any), and requires
information about what to render.
The output is svg.
Rendering of the SVG is managed by the client. Scrolling, scaling etc and in the Web document.
The viewer could in some cases

### Dioxus
Dioxus subscribes to changes to the context.

## Condition for render
The PCB view includes all items of interest (in memory).
The render will show elements of interest in a different color based on the active projects configured.

The context must expose at minimum:
- `get_active_projects()` — returns the list of active project instances for the current board.
- `get_generation_state()` — returns the current generation state (Idle, Generating, Failed).
- `get_diagnostics_summary()` — returns the current error/warning list for the persistent banner.

## Module Boundaries and Testability

Each module below has a defined input/output contract and must be testable independently, without requiring a full application context.

### Generator

Orchestrates the single-flight generation loop.

- Input: generation trigger (mutation event) + immutable Context snapshot.
- Output: `GenerationResult` — success with output artifacts, or failure with diagnostics.
- State: monotonic generation ID. A new trigger cancels the in-flight cycle; stale results are discarded.

Testing:
- Verify cancel-on-new-trigger discards stale output and never commits it.
- Verify identical inputs produce identical outputs (determinism).
- Verify `GenerationFailed` clears the program and surfaces diagnostics.
- Verify gray-out signal is emitted on cycle start and cleared on completion or failure.

### OperationPlanner

Pure function: `Board + JobInstance → PrimitivePlan`.

- No side effects, no I/O, no machine dialect awareness.
- Produces an ordered list of GCode primitives grouped by project type (drilling, contouring).
- TSP sort applied within each project type to minimise tool travel.

Testing:
- Unit-test drilling plan for a known board fixture.
- Unit-test contouring plan with tabs, V-groove, mouse-bite holes.
- Verify TSP ordering reduces total travel distance vs. naive order.
- Verify auto-rotation resolves correct angle and writes it back to project state.
- Verify board-too-large raises a fit error for any rotation.

### Coder

Pure function: `PrimitivePlan + CNC RHAI profile → GCode text`.

- No board data; purely a dialect resolver mapping primitives to machine-specific GCode.
- `cut_bezier` falls back to arc approximation for CNCs that do not support native bezier.

Testing:
- Snapshot tests per CNC profile against a fixed PrimitivePlan.
- Verify `cut_bezier` fallback path for non-native CNCs.
- Verify RHAI expression errors surface as typed diagnostics, not panics.
- Verify line numbering toggle and increment produce correct output.

### Tools (Assigner)

Pure function: `hole demand set + stock + allowances + rack capacity → ToolAssignment`.

- Implements the normative algorithm from the specification (candidate build, scoring, rack shrink, deterministic tie-break).
- Tie-break order: smaller diameter wins; if still tied, first in stable ordering wins.
- Numeric precision: 1 µm for all diameter and fit comparisons.

Testing:
- Verify tie-break: smaller diameter wins over larger when scores are equal.
- Verify 1 µm precision boundary: tools within 1 µm treated as equal diameter.
- Verify rack shrink selects minimum-regret removal at each step.
- Verify pilot-hole warning when shrink removes pilot drill but routing coverage holds.
- Verify infeasible hole (empty candidate set) raises immediate error with diagnostics.
- Verify mandatory routing tools ($R_{project}$) are never removed by shrink.

### BoardGeometry

Three sub-modules:

- `Stitcher`: raw geometry (lines, arcs, beziers) → closed and open contour objects.
  - Testing: verify continuity detection, open contour detection, correct ordering.
- `TopologyAnalyzer`: contour set → enclosure/overlap hierarchy.
  - Testing: verify A-in-B accepted, A-overlapping-B detected, depth-3 nesting (C-in-B-in-A) rejected.
- `OutlineValidator`: stitched edge → valid/invalid with structured error details.
  - Testing: verify missing closure, self-intersection, and illegal nesting depth each produce correct errors.

### KicadAdapter

Three sub-modules:

- `PipeDiscovery`: env variable check → pipe list; fallback to OS pipe scan.
  - Testing: mock env and filesystem to verify both discovery paths.
- `Session`: pipe handle → connection lifecycle (connect, active, dropped).
  - Testing: verify disconnect-before-cache triggers failure; disconnect-after-cache is silent.
- `SnapshotLoader`: active session → `BoardSnapshot` (holes, pads, tracks, edge).
  - Testing: verify partial load is discarded on connection failure; verify complete load is cached.

### ConfigPipeline

Four staged sub-modules, each independently testable:

- `Parser`: file bytes → typed config struct or `ParseError`.
- `SchemaValidator`: typed struct → validation result with field-level errors.
- `RepairPlanner`: validation result → repair action list (rename, default-inject).
- `FileApplier`: repair actions → filesystem mutations with error fallback.

Testing:
- Each stage tested with known-good and known-bad input fixtures.
- Integration test: full pipeline from corrupt file to recovered default with correct log entries.
- Verify bundled (internal) file error causes diagnostic display and controlled termination.

### Diagnostics

Typed error/warning model shared by all modules.

Fields: `code`, `severity` (Error / Warning), `scope`, `recoverable`, `message`, `details`.

Testing:
- Verify each module emits the correct severity and code for known failure inputs.
- Verify diagnostics clear automatically when the originating condition is resolved.
- Verify persistent banner reflects the highest-severity active diagnostic.

### RenderAdapter

Pure function: `RenderRequest (board + active projects + viewport params) → SvgScene`.

- No Context access; receives only the data it needs via the request struct.
- Color coding per project type is determined here, not in the UI layer.

Testing:
- Snapshot SVG output for known board + project fixtures.
- Verify each project type produces correct color coding.
- Verify empty board produces valid empty SVG, not an error.

### AppQueryModel

Read-only projection layer consumed by Dioxus signals.

Methods include: `get_active_projects()`, `get_generation_state()`, `get_diagnostics_summary()`.

Testing:
- Verify query results reflect Context state after known mutations.
- Verify Dioxus subscription triggers on render-counter increment.



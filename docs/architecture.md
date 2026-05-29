# Core technology decisions

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

# Operations

## Application initialisation
 * Load all configuration items
 * Load PCB data if possible and create a board object

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

### Job profile
The job profile is split into 3 parts so frontend and backend concerns remain isolated.

1. JobProfileDefinition (persisted profile)
- Declarative object stored as YAML.
- Includes profile metadata, default settings, references to CNC and fixture profiles, and the list of supported operations.
- Includes an input schema describing editable attributes, types, defaults, ranges, enums, and validation rules.
- Includes a processor kind identifier used by the backend factory (for example: drilling, contouring, scoring).

2. JobInstance (runtime configured job)
- Created from JobProfileDefinition plus user overrides from the UI.
- Fully resolved immutable input for generation.
- Versioned with generation ID so stale compute results can be discarded safely.

3. JobProcessor (backend algorithm)
- Selected by processor kind through a factory.
- Pure backend component that takes JobInstance and board/context data.
- Produces operation primitives and final GCode.
- May provide preview geometry data, but never UI widgets or frontend state.

Frontend behavior:
- The UI does not embed job algorithms.
- The UI renders forms from the input schema and edits overrides only.
- Pan/zoom/rotate and other viewport behavior remain frontend view state and are not part of the job profile.

Testing boundaries:
- Unit-test JobProcessor with fixed JobInstance inputs and expected GCode/operations.
- Unit-test schema validation independently.
- UI tests only verify form rendering/editing from schema and correct submission of overrides.

### Context
The context object is used to manage all aspects of the CAM software:
- configurations
- catalog
- pcb
- jobs
- renderer
- render_counter
- cnc
- fixture

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
The render will be show elements of interest in a different color based on the active jobs configured.

=> The context must have a method:
 get_active_jobs()

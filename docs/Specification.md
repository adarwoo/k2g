# K2G Unified Product and UX Specification

Status: Canonical product and UX requirements source.

## 1. Product Context

K2G is a portable desktop application and KiCad plugin that generates CNC GCode from PCB designs.

- Platforms: Windows, Linux, macOS
- Primary users: PCB makers and machinists
- Workflow style: high-feedback, continuously regenerated output

## 2. Core Product Principles

- All relevant settings changes are auto-saved.
- GCode regeneration is automatic and continuous when relevant inputs change.
- The UI must keep machining feedback visible while configuration is edited.
- Errors and warnings are always visible in summary form and drillable for full detail.
- Units are user-configurable and converted/displayed consistently across views.

## 3. Information Architecture

Primary navigation areas:

- Project
----------------
- Process profiles
- CNC profiles
- Fixture profiles
- Toolset profiles
- Stock management
- Catalog (opened as overlay from Stock)

The project is the machining workspace for a selected board.
Within a project, the user can switch between:

- Board View
- Program View
- Split View
- Tooling plan

The product should support rapid switching between:

- Board visualization mode
- Program text mode

## 4. Global Layout Direction

The UI should follow a slicer-style workstation layout (PrusaSlicer/Bambu-like structure, not branding):

- Top bar: board/project context, active profile summary, defaults cog, global status
  - Includes a persistent unit system quick-toggle (mm/in/mil)
  - Allow toggling theme (dark/light)
  - PCB Name or Project Name
  - Process Profile
  - CNC Profile
  - Fixture Profile
  - Toolset Profile

- Main body:
  - Left: primary navigation and section navigation for the active area
  - Center: board/machining viewport or program editor
  - Right: context settings, diagnostics, and editable parameters
- Utility area (bottom or side):
  - generation status
  - action feedback

When the user opens K2G from KiCad, the default landing area is the project view.
Note: The project view starts with selecting the Process profile. If no profile exist, an Error is diplayed, prompting the user to create a process profile.

## 5. Global Interaction Rules

- Auto-save is always on for configuration edits.
- Any relevant mutation retriggers generation automatically.
- Generation is single-flight: at most one generation cycle may run at a time.
  - If a new relevant mutation arrives while generation is in progress, the in-progress cycle is canceled.
  - The newly requested cycle becomes the active cycle and is the only cycle allowed to commit output.
- Errors and warnings are summarized in a persistent banner across screens.
- Clicking the summary opens detailed diagnostics.

### 5.1 Global Measurement Editing Contract

- Unit preferences apply globally, with automatic conversion display where needed.
- A global units service must be used for parse, conversion, and display formatting across screens.
- Length values are persisted with their original explicit unit expression (for example `mm`, `in`, `mil`, including inch fractions).
- Feed-rate values are persisted with explicit units and must always include a feed-rate unit.
- Angle values are displayed with the `°` symbol.
- Rotational-speed values are displayed with the `rpm` label
- Frontend formatting always shows preferred user units first; if persisted unit/expression differs, append the original value and unit in brackets.
- When preference is inch and source value is an inch fraction, show decimal inch first and original fraction in brackets.
- Display precision is global: `mm` at 0.001, `in` at 0.00001, `mil` at 0.1.
- Display-unit mapping is global: `mm -> mm/min`, `in -> in/min`, `mil -> in/min`.
- Angle display is global: degrees are shown consistently as `°` across the UI.
- Rotational-speed display is global: spindle speeds are shown consistently as `rpm` across the UI.
- For any user-entered measurement that accepts units, the current user preference unit is assumed when no unit suffix is provided.
- In non-editing context, editable measurements are shown in the preference unit system; if native/original unit differs, the native/original value is appended in brackets.
- On entering edit mode, matching units are stripped to a raw value; angle and rotational-speed suffixes are always removed; when the active unit does not match the stored length/feed unit, the original expression is preserved for editing.
- While editing, users may enter decimal or fractional values and may optionally override with an explicit unit suffix.
- Enter validates and commits the edited value as the new reference value and exits edit mode.
- If Enter validation fails, an inline error is shown and focus must remain in the same field.
- Escape always cancels the edit, restores the previous valid value, and exits edit mode.
- If generated program text has user edits and regeneration would overwrite them, a confirmation prompt is required.

## 6. Profiles and settings Management

There is no dedicated Setup area.
Persistent assets are managed directly from primary navigation entries (process profiles, CNC profiles, fixture profiles, toolset profiles).
Global defaults are managed from the top-bar cog icon.

### 6.1 Default Settings

The default settings are accessed from the top bar directly. They are:

- Theme:
  - Light
  - Dark

- Display Units:
  - mm: All length values are shown in decimal mm rounded to the nearest 0.001, and feed-rates in mm/min
  - in: All length values are shown in decimal inch rounded to the nearest 0.00001, and feed-rates in in/min
  - mil: All length values are shown in decimal mil rounded to the nearest 0.1, and feed-rates in in/min

No other settings belong in this panel.
Profile selections and machining behavior defaults are defined by profiles and by the active project, not by global defaults.

#### 6.1.1 Profiles reference modes

Profiles may reference other profiles through a constrained-selection model.

A profile reference shall use one of the following modes:

Fixed
- Exactly one profile is assigned.
- The user may not select another profile.

List
- A list of allowed profiles is defined.
- One profile is marked as default.
- The user may select any profile from the list.

Any
- Any compatible profile may be selected.
- One profile is marked as default.

New
- A new profile instance is created for the current project.

This mechanism allows higher-level profiles to constrain lower-level profile selection while preventing users from exceeding the intended operational scope.

For List and Any modes:
- A default profile is mandatory.
- The default must belong to the allowed set.
- Exception: a New default is permitted.

#### 6.1.2 Profile Identity and References

- All persistent profiles shall possess a globally unique immutable identifier.
- Profile identifiers shall be 256-bit UUIDs.
- The UUID is the canonical identity of a profile and shall remain unchanged for the lifetime of that profile.
- Profile names are user-facing labels only and are not used for reference resolution.
- All profile-to-profile references and all project-to-profile references shall be performed using UUIDs.
- Changing a profile name shall not impact references.
- Duplicating a profile shall generate a new UUID.
- Importing a profile whose UUID already exists shall require conflict resolution.
- Profile deletion removes the profile from persistent storage but does not invalidate saved projects.
- Deleting a profile shall never modify already-saved projects.

### 6.2 CNC Profile Management

The CNC Profiles page is split into two vertical regions:

- Top region: action bar and the visible list of available CNC profiles
- Bottom region: editor for the currently selected CNC profile

Top region actions are:

- Add from built-in
- Import
- Export
- Duplicate

Users can:

- Select a CNC profile from the visible profile list
- Create a new added profile
  - Import a profile
  - Duplicate an existing profile
  - Start from a built-in template
- Export the selected profile
- Delete a created profile

Profile origin and editability rules:

- Built-in (stock) profiles are read-only
- Added profiles are editable
- Only added profiles can be edited in the bottom editor region
- When a built-in profile is selected, the bottom region shows it in read-only mode

Deleting a CNC profile is allowed, including when referenced.
Deletion performs a cascading delete of dependent assets and must require explicit confirmation.

### 6.3 Fixture Profile Management

Users can:

- Select a fixture profile
- Create a new fixture profile
  - Clone an existing profile
  - Start from a built-in template when available
- Delete a created profile
- Edit a profile

Deleting a fixture profile is allowed, including when referenced.
Deletion performs a cascading delete of dependent assets and must require explicit confirmation.

Fixture profiles describe how the PCB is physically held and aligned on the machine.
They are persistent configuration assets and are not tied to a single board.
Fixture profiles may influence generated output, but they do so through CNC profile abstractions.
For example, a fixture requests a work offset intent from the CNC profile; it does not directly emit machine-specific GCode.

Fixture fields include at minimum:

- Fixture name
- Supported board holding method
- Work origin/reference definition
- Locating pin strategy and geometry
- Keep-out or clamp zones
- Fixture occupancy (defines the impact on the board size range)
- Optional probing/alignment parameters

Fixture profiles are persisted as YAML and validated by `fixture_profile.schema.yaml`.

### 6.4 New Profile Wizard

Clicking + opens a wizard/modal that:

- Shows built-in templates (for example Generic, Genmitsu3040, Masso)
- Shows existing profiles available for cloning
- Requires a unique profile name
  - Clone default: Copy of "profile name"
  - Template default: My "template profile name"
- Disables New until validation passes
- Displays inline naming conflict errors

After creation, profile editing starts immediately.

### 6.5 CNC Profile Fields

General fields include:

- Fixture plate max size X and Y
- Max feed rate
- Spindle min and max RPM
- Spindle start and stop delay
- Board rotation values and tool point-angle values are displayed with the `°` symbol.
- Tool spindle speeds are displayed with the `rpm` label.
- ATC slot count (0 disables ATC)
- Origin orientation:
  - X0: Left, Right, Front, Back
  - Y0: Front, Back, Left, Right
- XY scaling percent
- Program line numbering toggle and increment value

### 6.5.1 CNC Field Editability by Profile Origin

- For added profiles, fields in section 6.5 are editable.
- For built-in (stock) profiles, fields in section 6.5 are read-only.

### 6.6 CNC Program Section and Customization

CNC profiles include operation-specific and generic program snippets.

- Snippets can include expressions using braces evaluated by the scripting runtime.
- In expression editors, typing opening brace should allow inserting variables/functions.
- A read-only to editable toggle is provided for protected sections.
- Each editable function shows available variables and parsing feedback.
- Resulting output should be validated as GCode.

Sanity check behavior:

- A configured sanity check function returns a string.
- Non-empty output blocks project generation and is shown as an error.

Custom attributes:

- Supported types: bool, string, list, percent, number, date
- Required metadata: unique valid name, type, description, default value
- List attributes support user-managed options with name and description
- Attribute values can be validated and transformed by user-defined expressions

#### Custom GCode generation with RHAI

The application GCode generation can be customize using GCode snippets and a built-in RHAI interpretor.

- Low-level operations (for example, "Drill first hole following tool change") are written as machine-code excerpts.
- Strings between `{}` are interpreted as RHAI.
- RHAI receives all relevant variables/settings.
- In editors, typing `{` should open variable/function insertion suggestions.

Profiles behavior:

- Initial list item is `New`.
- Selecting `New` adds a next step where user chooses a profile to copy.
- Application ships with predefined profiles.
- New machine profiles can be cloned from existing profiles.

All RHAI sections can be dynamically edited by clicking |>. (They are read-only otherwise).
This displays a dialog listing all the variable passed to the function with default values.
Click on the variable adds it at the cursor position in the edit field.
The result value is show automatically, just like parsing errors.
The resulting output is tested to be valid GCode.

Each function comes with a pre-defined list of variables. Custom attributes are automatically added to the scope of all functions.

Examples:
machine:
  program_units:
    length: "mm"
    feedrate: "mm/min"

primitives:
  initialise: |
    (Created by kicad2gcode from '{pcb_filename}' - {timestamp})
    (Reset all back to safe defaults)
    G17 G54 G40 G49 G80 G90
    G21
    G10 P0
    G0 Z{z_safe}
  move_slow: "G0 X{x} Y{y}"
  start_spindle: |
    S{rpm}
    M03
  stop_spindle: "M05"
  drill: "G81 X{x} Y{y} Z{z_bottom} R{z_retract} F{z_feedrate}"
  peck_drill: "G83 X{x} Y{y} Z{z_bottom} R{z_retract} Q{peck} F{z_feedrate}"
  cut_arc: "{arc_cmd} X{x} Y{y} I{i} J{j} F{xy_feedrate}"
  cut_bezier: |
    ; cut_bezier fallback
    ; implementation may expand into one or more arc segments
  change_tool: |
    M05
    {manual_message}
    T{slot} M06
    S{rpm}
  conclude: |
    (end of file)

RHAI parser and expression model:

- The application includes a RHAI expression parser/evaluator used by CNC program snippets and expression-backed profile fields.
- CNC program fragments that are represented as expressions must be stored and evaluated as RHAI expressions.
- Expression evaluation is done against the active project context and must be deterministic for identical project inputs.
- Parse and evaluation failures are surfaced as diagnostics with source location and expression name.
- Expression editing UX must provide parse feedback before apply/save.

Primitive templates as CNC configuration attributes (schema-required):

- Primitive templates are stored as CNC configuration attributes and validated by the CNC profile schema.
- Each attribute is a string template containing GCode text with optional embedded RHAI expressions in `{}`.
- Attribute values are compiled/evaluated through the same `parse_exp` path used by expression editors.

Required primitive attributes:

- `primitives.initialise` -> maps to primitive `initialise`
- `primitives.move_slow` -> maps to primitive `move_slow(x, y)`
- `primitives.start_spindle` -> maps to primitive `start_spindle`
- `primitives.stop_spindle` -> maps to primitive `stop_spindle`
- `primitives.drill` -> maps to primitive `drill`
- `primitives.peck_drill` -> maps to primitive `peck_drill`
- `primitives.cut_arc` -> maps to primitive `cut_arc`
- `primitives.cut_bezier` -> maps to primitive `cut_bezier`
- `primitives.change_tool` -> maps to primitive `change_tool`
- `primitives.conclude` -> maps to primitive `conclude`

Optional primitive attributes:

- `primitives.pause` -> optional pause/message insertion point
- `primitives.banner` -> optional comment/banner insertion point

Compatibility and fallback requirements:

- If a required primitive attribute is missing, schema validation fails for that CNC profile.
- `primitives.cut_bezier` may resolve to a native bezier command or an arc-approximation sequence; fallback behavior must be deterministic.
- Primitive templates may reference custom attributes and active project properties through the RHAI scope.

### 6.7 Process Profile Management

Project profiles define machining defaults and constraints for a family of machining operations on given machining hardware.
Each process profile predefines all relevant machining defaults and constraints, including CNC profile selection and constraints for fixture and toolset selectio

Users can:

- Select a process profile
- Create a new process profile
  - Clone an existing profile
  - Start from a template when available
- Delete a created profile
- Edit a profile

Deleting a process profile is allowed - even if selected by the current project.
Deletion performs a cascading delete of dependent assets and must require explicit confirmation.
If the project is using the process profile, the project page goes back to having to select a process profile, and the generation is reset.

Each process profile includes:

- Fixed CNC profile
- Fixture profile reference
  - Fixed, List, Any
- Toolset profile reference
  - Fixed, List, Any or Auto

- Default enabled operations
- Default machining strategies
- Default tool selection parameters
- Default routing/tab settings
- Feature selection (which machining features/operations are part of the profile)

Process profiles are persisted as YAML and validated by `process_profile.schema.yaml`.

The project requires the user to select a process profile.
As soon as the project selects a process profile, the profile values become the initial effective project values.
The project may override any overridable profile-defined attributes.

#### 6.7.1 CNC Profile selection

CNC profile references are always Fixed.

Process profiles shall not support List, Any or New CNC profile references.

Rationale:
- Generated output depends directly on CNC profile behavior.
- CNC profiles may define machine limits.
- CNC profiles may define coordinate conventions.
- CNC profiles may define GCode dialects and machine-specific primitives.
- CNC profiles may define machine-specific RHAI customization logic.

Allowing runtime CNC profile substitution is considered unsafe because a user may unintentionally generate or execute output intended for a different machine.

To support multiple CNC types, separate process profiles should be created.

### 6.8 Profile Deletion and Cascade Rules

Because dependencies are hierarchical (Project-> Process profile -> CNC profile + fixture profile), deleting a profile may delete additional assets.

Cascading deletion is allowed for CNC profiles, fixture profiles, and process profiles.
Before final confirmation, the UI must show a warning dialog that lists every asset that will be deleted.

The warning list must include, as applicable:

- The explicitly selected profile(s)
- Dependent process profiles that reference a deleted CNC or fixture profile
- Live/open project instantiated from any process profile that will be deleted

Confirmation requirements:

- The warning dialog must show total impacted asset counts by type and an explicit itemized list
- Delete action is disabled until user explicitly confirms
- Cancel leaves all assets unchanged

## 7. Stock and Catalog

### 7.1 Stock Page

The stock page lists tools available for machining.
Stock tools are independent records in stock, even when created from a catalog tool.
The stock list is presented as one unified table for all tool families.
Universal filtering and sorting apply across the full stock list.

Single-table behavior requirements:

- The table supports faceted filtering by tool family (drill, router, engraver, v-bit)
- The table exposes an explicit type filter with: All, Drill, Router, V-bit, Engraving
- Multiple filter facets can be combined with text search
- Users can save and reapply filter presets
- The table supports configurable columns so type-specific fields can be shown without splitting into sublists
Each stock item includes:

- Tool family shown in the list as one of: Drill, Router, V-bit, Engraving
- Tool family is color-coded in the list: blue for Drill, green for Router, yellow for V-bit, red for Engraving
- Name display always starts with the composite tool label; if the user supplies a practical name in tool properties, it is appended after a dash
- Source catalog
- Source SKU (read-only when catalog-derived)
- Auto-generated summary (type + diameter)
- Availability status:
  - In stock
  - Out of stock
- Preference property for tool selection:
  - Preferred
  - Neutral
  - Not preferred
- Indicator (green dot) to show the tool is expected to be in the ATC rack (ATC only)
- Tool metadata copied from catalog and editable after import in stock
- Editable stock properties

Stock table columns:

- Default visible columns, in order: type, diameter, name, source catalog, preference, status
- Default row ordering is insertion order with the most recently added stock item shown first
- The list can be sorted by type while preserving deterministic within-type ordering
- ATC indicator column (shown only when the selected CNC has ATC): green dot with slot index (T1, T2, etc.) if tool is assigned, dash if unassigned
- Additional configurable columns: spindle RPM, Z feed, ATC expected, usage counter, source SKU, manufacturer, SKU
- Type-specific columns (available and filterable when relevant): XY/table feed, point angle, tip diameter, flute length, minimum depth, max hits/life limit
- Type-specific columns may be empty for non-applicable tool families and must not block sorting/filtering

Selection, detail, and bulk action behavior:

- Every stock row is selectable by checkbox, with a header checkbox for selecting all visible rows
- Deleting selected stock tools requires explicit confirmation
- Double-clicking a stock row opens a dedicated tool detail view for that tool
- The detail view presents label-left and value-right rows in a vertically scrollable panel
- The detail view allows editing only: custom name, diameter, tip geometry, feed rate, spindle speed, status, and preference
- Runtime values like the current ATC slot are not shown as tool properties in the detail view
- Catalog-derived metadata shown in the detail view is read-only
- The detail view exposes a clone action for the current tool
- The list supports multi-selection
- Multi-selection supports bulk delete

Field editing and validation behavior:

- For editable stock detail fields, pressing Enter is the only action that validates and commits the current field value
- Pressing Escape reverts the current field to the last committed valid value and exits edit mode for that field
- If no stock detail field is currently in edit mode, pressing Escape exits the tool detail view and returns to the stock list
- Losing focus without pressing Enter must not implicitly commit an edited value
- When Enter validation fails, focus remains in the current field and an inline popup message explains the validation error
- Validation popup clears when the field is reverted with Escape or successfully committed with Enter
- Stock detail field changes are applied immediately on commit or selection change; no separate Save Changes action exists
- For measurement fields specifically:
  - In non-editing context, show preference-unit value first, with native/original in brackets when different
  - On entering edit mode, render only the raw numeric value in the active preference unit system
  - Accept decimal and fractional numeric input, with optional explicit unit suffix
  - Do not transfer focus to other controls while a measurement field is in invalid edit state; only Enter (valid commit) or Escape (revert) completes the sequence
- For catalog-derived editable fields, if the current value differs from the catalog baseline, show the original catalog value in orange to the right of the user value and show a revert icon
- Clicking the revert icon immediately restores the catalog baseline value for that field
- This override affordance (orange original value + revert icon) is a shared UI pattern and should be used consistently in other override surfaces, including project settings

### 7.1.1 Stock Field Validity Rules

Stock detail field validity requirements:

- Diameter:
  - Required
  - Must parse as a valid length expression
  - Accepts decimal and fraction forms
  - Accepts explicit unit suffixes (for example `1mm`, `3/4in`)
  - If unit suffix is omitted during editing, current global unit mode is applied before validation
  - Value must be strictly greater than zero
- Feed rate:
  - Optional
  - Empty value is valid and clears feed rate
  - Non-empty value must parse as a valid feed-rate expression
  - Accepts decimal and fraction forms with optional unit suffix
  - If unit suffix is omitted during editing, current global unit mode feed unit is applied before validation
  - Non-empty value must be non-negative
- Tip geometry:
  - Required
  - Must parse as numeric
  - Value must be greater than 0 and at most 180 degrees
- Spindle speed:
  - Optional
  - Empty value is valid and clears spindle speed
  - Non-empty value must parse as numeric
  - Non-empty value must be non-negative

Ordering behavior:

- User can order the unified list by tool size
- User can order the unified list by addition precedence
- Addition precedence is internal ordering metadata and is not shown as a visible table column

Adding/editing rules:

- Plus action supports adding from catalog
- Adding from catalog creates stock copies, not references
- All catalog metadata is copied into the stock tool at add time
- Because stock tools are copies, deleting a catalog or catalog entry has no impact on existing stock tools
- Apply is disabled when uniqueness validation fails
- Any stock mutation (add, edit, clone, delete, or reorder) is a relevant change and triggers regeneration

Catalog add UX requirements:

- A detailed table view lists catalog tools with key fields visible in columns
- Users can multi-select any number of catalog tools from the table
- The primary add action imports all selected tools into stock in one operation
- Selecting one catalog tool opens a full detail view for that tool
- In the detail view, an Add button imports that single tool directly to stock
- Stock tool detail entry fields for diameter and feed rate follow the global unit toggle (mm/in/mil) for both display and input parsing
- Diameter values entered with an explicit unit suffix (for example `3/4in`) are treated as unit-bound values and are not auto-converted in-place when the global unit toggle changes

### 7.2 Catalog Overlay

The catalog is opened from stock add flow as an overlay.

- Libraries are read-only and can be added externally via YAML files
- Tool detail includes:
  - Manufacturer and SKU
  - Name and description
  - Diameter with unit expression support (mm/in/mil, fraction/float)
  - Recommended RPM
  - Recommended Z feed
  - Recommended horizontal feed (routers)
  - Geometry/type
  - Referenced flag
- Detailed table supports multi-selection before adding to stock
- Add action imports selected tools into stock and keeps copied metadata as informational-only fields
- Back action closes overlay
- In stock tool detail editor:
  - Catalog tool diameters are treated as explicit-unit values
  - Diameter display includes unit suffix when not actively editing (for example `0.024in`)
  - If a user enters a diameter number without a unit suffix, the current global unit mode suffix is automatically applied
  - Persisted stock keeps the original explicit unit representation for unchanged catalog-derived diameters

When a tool from a catalog is highlighted, the detail view is shown.
The detail view includes an Add button to import that tool directly to stock.

## 8. Project Workspace

The project is the live execution context for machining one selected board.
It combines:

- One board
- One selected process profile
- The CNC and fixture profiles referenced by that process profile
- Runtime overrides of profile-defined values (except CNC and fixture)
- Generated machining outputs and diagnostics

The project is the heart of the product and is the default focus when launched from a PCB.

### 8.1 Project Context and Runtime Model

- A project is created when the user selects or opens a board.
- Exactly one active project exists at a time in the application context.
- The board and the active process profile are the primary context objects for that project.
- CNC and fixture are normally inherited from the selected process profile.
- CNC and fixture cannot be overridden on the live project.
- Changing CNC or fixture requires selecting or editing a process profile.
- All other profile-defined attributes may be overridden on the live project.
- Overrides affect the current project instance and do not silently mutate the saved profile.
- The active project references one selected process profile and carries all runtime overrides as effective values.
- The active project exposes property access APIs used by RHAI evaluation and generation.

#### 8.1.1 Saved Projects

Projects may be saved and reopened independently of KiCad.

A saved project contains:
- PCB snapshot data
- Selected process profile
- Resolved profile selections
- Runtime overrides
- Generation settings
- Generated outputs
- Diagnostics metadata

The PCB snapshot shall contain sufficient information to allow regeneration without a live KiCad connection.
When opening a project, the connection to KiCad is turned off to prevent confusion.

Profile references point to reconstructed temporary profiles.

#### 8.1.2 Project Packaging

A saved project is a self-contained container.

When a project is saved, all referenced profiles shall be embedded in the project file.

This includes:
- Process profile
- CNC profile
- Fixture profile
- Toolset profile
- Any additional profile types introduced in future versions

Embedded profiles are stored together with:
- Their UUID
- Their complete configuration
- Their version metadata

Rationale:
  A project shall remain reproducible even when the external profile database changes.

Saving a project captures the exact profile definitions used when the project was saved.

#### 8.1.3 Profile Resolution During Project Load

When opening a project, profile references shall be resolved using UUIDs.

Resolution shall follow the following order:

1. Search the profile database for a profile with the same UUID.
2. If found, and if the version metadata matches the database then use the database profile.
3. When the version is different:
   - Warn the user that the profile has been changed
   - Ask the user to resolve the situation by:
      - Apply current profile
      - Update current profile from the project profile
      - Use a temporary profile
   
3. If not found, create a temporary profile from the embedded definition.

#### 8.1.4 Temporary Profile Behavior

When a referenced profile cannot be found in the profile database, the system shall automatically create a temporary profile using the embedded project copy.
If the temporary profile UUID already exists in the profiles database, the a new UUID is assigned, the project reference is updated.

- Temporary profiles exist only for the lifetime of the opened project.
- Temporary profiles shall behave identically to normal profiles from the perspective of generation and editing.
- Temporary profiles shall be visually identified.

Recommended indicators include:
- "Temporary"
- "Embedded"
- Missing-profile warning badge

The user shall always be informed that the profile does not currently exist in the profile database.

#### 8.1.5 Temporary Profile Promotion

Temporary profiles may be promoted to persistent profiles.

Promotion is performed from the corresponding profile management page.

Promotion creates:
- A new persistent profile
- Using the UUID from the temporary profile

#### 8.1.6 Project Close Behavior

Temporary profiles are not persisted in the profile database automatically.

When a project is saved, all temporary profiles remain embedded within the project together with their current modifications.

When the project is closed:

- Temporary profiles are removed from the active profile registry.
- Temporary profiles are not written to the profile database.
- Promoted profiles remain in the profile database.

Closing a project shall never automatically create persistent profiles.

#### 8.1.7 Visual indication

When a temporary profile is active within a project, the top bar shall display a visual warning indicator.

This indicator remains visible until the profile is promoted or the project is closed.

#### 8.1.8 Temporary profiles during project save

Temporary profiles participate in Project save operations exactly like persistent profiles.

The distinction between temporary and persistent profiles only affects profile database storage


### 8.2 Core Project Controls

- Selected board / PCB source
- Selected process profile with quick link to profile editor
  - Last used process profile is selected automatically on open
  - On startup, generation proceeds immediately with the reused profile.
  - A profile is never treated as inherently incompatible at selection time.
  - Feasibility is determined dynamically by generation.
  - If constraints are violated (for example board size limits, missing tools, fixture constraints), generation raises detailed errors in diagnostics.
- Derived CNC profile with quick link to CNC profile editor
- Derived fixture profile with quick link to fixture profile editor
- Production operations (multi-select):
  - Drill locating pins
  - Drill PTH
  - Drill NPTH
  - Route board
  - Mill board
- Side selection: Component, Solder
  - Note: By default, the PCB is laid on the CNC bed in the same orientation as displayed in KiCad. The actual coordinate system origin (X0, Y0) is defined by the CNC profile.
- Board rotation:
  - Auto
    - Generation determines the best fit rotation automatically.
    - The resolved rotation value from the most recent successful generation is written back to the project state and shown in the UI.
    - If the board cannot fit within machine constraints for any rotation, generation raises an explicit fit error.
  - Manual:
    - 0
    - 90
    - Free entry -180.0 to +180.0

Note: The board rotation default is defined by the active process profile and may be overridden in the live project.

### 8.3 Override UX Rules

- The UI clearly distinguishes profile defaults from live project overrides.
- When a value is overridden, it is shown in orange with a small revert icon beside it.
- Reverting restores the profile default for that field.
- CNC profile and fixture profile are not overridable at the live project level.
- If the user wants to change the saved default rather than the current project, the UI should offer an explicit Edit Profile action.

### 8.4 Project Workspace Composition

The live project workspace should keep the board and machining result at the center at all times.

- Left area: project navigation and section switching
  - Board
  - Program
  - Split
  - Rack when relevant
- Center area: board viewport or GCode editor
- Right area: live project controls grouped by meaning
  - Operations
  - Placement/orientation
  - Tooling strategy
  - Routing/tab settings
  - Diagnostics and warnings

Profile editing from the project should use overlays, drawers, or focused subpages that preserve the current board context whenever possible.
The user should never feel they have left the machining task just to adjust a profile.

### 8.5 ATC Strategy (when ATC is available)

ATC strategy and rack/tooling constraints are defined by the rack system in section 11.
Generation must apply the selected rack policy and surface warnings/errors accordingly.

### 8.6 Routing Controls (when routing enabled)

- Tab count (0 to feasible max)
- Tab width
- Mouse-bite holes toggle
- Hole size selector from stock
  - Disabled when no compatible stock tool exists
- Holes per tab (1 to feasible max)
- Computed center-to-center spacing display in preferred units

If tab count is 0, VGroove options are shown:

- Tool selection from stock
- Depth percent from 50 to 100 [defaults to 80%]

### 8.7 Tool Selection Strategy

- Oversize allowance percent (for hole matching, example default 5%)
- Undersize allowance percent (for fallback matching, example default 10%)
- Allow routing holes toggle
  - When enabled: large holes without a matching drill can be routed
  - Drill-then-route sub-option: drill to nearest smaller size first, then enlarge by routing
  - Pilot hole sub-option: for holes that are routed because no suitable drill exists, drill a pilot hole first, then route
    - Pilot hole uses the largest available drill bit that is strictly larger than the router bit and valid for the hole geometry
    - If no suitable pilot drill is available, route the hole without a pilot

Algorithm definition (normative):

1. Build the hole demand set.
   - Collect all drillable holes requested by enabled operations (PTH, NPTH, locating).
   - For each hole, normalize to an effective target diameter:
     - Circular: nominal drill diameter.
     - Slot/oval: minor axis for drill suitability, full geometry retained for routing fallback.
   - Account for the plating thickness for PTH

2. Build candidate tool sets per hole.
   - For each hole $h$ with target diameter $d_h$, build candidate set $C_h$ from in-stock and available tools:
     - Drill candidates satisfy: $d_t \in [d_h(1-u),\ d_h(1+o)]$, where:
       - $d_t$ is tool diameter
       - $u$ is undersize allowance
       - $o$ is oversize allowance
     - Router candidates are included only when `Allow routing holes` is enabled and geometry/process allows routing for that hole kind.
     - Pilot-hole candidates are evaluated only for holes assigned to routing due lack of suitable drill candidates.
   - Add all tools already required by non-hole routing operations (board contour, internal cutouts, tabs, V-groove) to a mandatory routing set $R_{project}$.
   - Effective candidate set for assignment is $C'_h = C_h \cup (C_h \cap R_{project})$; this ensures routers already needed by contouring are always considered for hole fallback.

3. Initialize the working tool universe.
   - Start from union of all feasible tools plus required routers:
     - $U_0 = R_{project} \cup \bigcup_h C'_h$
   - If any hole has $C'_h = \varnothing$, raise an immediate generation error with actionable diagnostics (hole id/type/size and closest stock tools).

4. Compute preferred per-hole assignment (before rack shrinking).
   - For each hole, rank candidates by score and pick the best available tool in $U$:
     - Primary strategy weight: drilling preferred over routing when `Drill-then-route` is enabled.
     - Size fit: absolute normalized diameter error.
     - Stock preference property: Preferred > Neutral > Not preferred.
     - Stability tie-breaker: prefer tools already in $R_{project}$ to reduce tool changes.
     - Tie-break rule: when candidates are still tied, smaller diameter tools win.
     - Final tie-break rule: if still tied, keep first candidate by stable ordering.
     - Numeric precision rule: all diameter/fit comparisons are evaluated at 1 um precision.
   - For holes assigned to routing because no suitable drill exists:
     - If pilot-hole option is enabled, attempt to add a pilot drill pass before routing.
     - Pilot selection rule: choose the largest valid drill bit with diameter strictly greater than the selected router bit diameter.
     - If pilot selection fails, fallback is full routing only (router plunge then elliptical interpolation to cut the hole).
   - Recommended score form:
     - $S(h,t)=W_s\cdot strategy(h,t)+W_f\cdot fit(h,t)+W_p\cdot pref(t)+W_r\cdot reuse(t)$
     - with $W_s$ dominant so drilling wins unless no valid drill exists.

5. Shrink to rack capacity iteratively (ATC mode).
   - Let rack capacity be $K$ and current set be $U$.
   - If $|U| \le K$, finish.
   - Otherwise, repeatedly remove one non-mandatory tool $t \in U \setminus R_{project}$ with minimum global regret:
     - Regret of removing $t$ is the weighted loss after reassigning all holes currently using $t$ to their next-best candidate in $U \setminus \{t\}$.
     - Infeasible reassignments add a prohibitive penalty (effectively infinite regret).
     - Additional penalty applies when removal forces drill -> route transitions.
   - Recompute assignments after each removal.
   - Stop when $|U| = K$ or no removable tool preserves feasibility.

6. Failure and degradation behavior when rack is too small.
   - If no feasible shrink reaches $K$, return:
     - Error: required tool count exceeds rack capacity.
     - Minimal feasible count $K_{min}$.
     - Holes that become uncovered under current constraints.
   - If rack shrinking removes pilot drill tools but routed-hole coverage remains feasible:
     - Do not raise an error.
     - Raise a warning that pilot holes were disabled by rack capacity.
     - Affected holes are machined as full-route holes (router plunge followed by elliptical hole cut).
   - Suggested remediations (in UI order):
     - Increase rack slots.
     - Enable routing fallback for holes.
     - Increase oversize/undersize allowances.
     - Disable optional operations.

7. Determinism and explainability requirements.
   - Given identical inputs, selected tool set and per-hole assignments must be deterministic.
   - Deterministic ranking uses the following tie-break order after strategy and fit scoring:
     - Smaller tool diameter wins.
     - If still tied, first candidate in stable ordering wins.
   - Numeric precision for tool-diameter and fit comparisons is 1 um.
   - For each hole assignment, store explainable reason fields:
     - selected tool id
     - strategy (drill/route)
     - fit error
     - whether assignment changed due to rack shrink
   - For each removed tool during shrink, store removal reason and incremental regret.

## 9. Board View

Board view shows machinable elements with direct feedback.

- Auto-refresh on relevant changes
- Filter toggles for holes, routes, and paths
- Pan/zoom and reset-to-fit
- Editable tab markers:
  - Visual position display
  - Drag and numeric adjustment
  - Algorithmic placement assistance

## 10. Program View

Program view exposes generated GCode with controlled editing and export workflows.

- Monospace editor is available only after generation completes successfully.
- During an active generation cycle, program editing is disabled.
- User can add, remove, and comment sections within the generated program.
- Program edit workflow:
  - When program is modified after generation, a warning is displayed indicating changes will be lost on regeneration.
  - User can save changes (persisting edits locally) or cancel (discarding edits).
  - Program is invalidated as soon as a user mutation triggers a new generation cycle.
  - If generation fails, the current program is deleted and the editor shows empty state with error diagnostics.
- Program actions:
  - Save to file
  - Save to removable media
  - Eject media (when applicable)
  - Send over network to CNC

## 11. Toolset Specification

### 11.1 Goals

The toolset defines:

- preferred tooling
- allowed tooling
- ATC tooling strategies
- reusable tool sets
- machining-time tooling behavior

The system must support:

- automatic tool changers (ATC)
- manual CNCs
- hybrid/manual reload workflows

The toolset profile acts as:

- a tooling template
- a toolset preload definition
- a generation constraint system

### 11.2 Core Concepts

#### Toolset Profile

A toolset profile defines:

- A list of tool assignments for tools slots T1..Tx
- generation constraints and tooling policies

A toolset profile:

- may contain fixed tools
- may contain empty/spare slots
- may forbid use of specific slots
- may exceed physical CNC capacity

Toolset profiles are reusable across processes.

### 11.3 Toolset Slot Model

Each toolset slot corresponds to T1..Tx.

Each slot may contain one of:

#### Fixed Tool

A specific tool is permanently assigned.

Example:

```text
T1 -> 0.8mm carbide drill
T2 -> 30deg V-bit
```

Purpose:

- pre-defined in the CNC
- stable ATC setup
- avoid reloading
- production consistency

#### Spare

The slot is intentionally left available.

Purpose:

- placeholder for any additional required tools as generated dynamically from the program generator
- allows flexible project-specific expansion

Example:

```text
T5 -> Spare
```

#### Do Not Use

The slot is disabled and must never be allocated.

Purpose:

- broken holder
- reserved position
- inaccessible slot
- intentionally skipped tools

Example:

```text
T7 -> Do Not Use
```

### 11.4 Toolset Size Behavior

Toolset profiles are not limited in size.

Example:

- profile defines 20 slots
- machine physically has 12 ATC slots

Behavior:

- profile remains valid
- warning is generated if strict mode (see policies) is ON
- generation may still proceed depending on policy

This allows:

- portability between machines
- future expansion
- logical rack supersets

### 11.5 Generation Policies

The toolset profile defines tooling behavior during GCode generation.

#### Policy: Single Rack Only

Behavior:

- generator must use only tools available in the rack
- no manual intervention allowed
- no reload allowed

If additional tools are required:

- generation fails with error

Use case:

- unattended machining
- production runs
- industrial ATC workflows

#### Policy: Allow Reload

Behavior:

- generator initially uses rack contents
- when rack capacity is exceeded:
- machining pauses
- user reloads rack
- machining resumes

Use case:

- small ATC systems
- larger processing than rack capacity
- semi-attended machining

#### Policy: Allow Manual Changes

Behavior:

- rack tools are used first
- when exhausted:
- last ATC tool is returned to rack
- user manually inserts next requested tool
- machining resumes

This effectively transitions ATC to manual operation.

Use case:

- hybrid machines
- limited ATC capacity
- low-cost PCB routers

### 11.6 Toolset Profiles in Process Profiles

A process profile constrains the project into selecting a tooling profile using the global process referencing strategy.
However, the ad'hoc profile 'Auto' is added.

#### Auto semantic

A project with a toolset set as 'Auto' means:

- no predefined tooling assumptions
- generator creates optimal tool allocation for the current project

This may include:

- ordering optimization
- slot minimization
- operation grouping

### 11.9 UI Requirements

#### Toolset Profile Editor

Must support:

- arbitrary slot counts
- drag/drop or editable rows
- visual slot states

Each slot displays:

- slot index
- assigned tool
- slot mode

Recommended visual states:

- Fixed tool: tool icon
- Spare: dashed or empty state
- Do Not Use: disabled or red state

### 11.10 Validation Rules

#### Validation: Physical Rack Size

If:

```text
rack profile slots > machine ATC capacity
```

Generate:

- warning
- not hard failure

#### Validation: Generation Feasibility

If policy is:

```text
Single Rack Only
```

and required tooling exceeds available tooling:

Generate:

- hard error

#### Validation: Default toolset

For List and Any:

- default toolset must exist
- default toolset must belong to allowed set
- unless default is New

### 11.11 Conceptual Model

A Toolset Profile defines:

preferred tooling
allowed tooling
tool assignment order
operator workflow constraints
optimization constraints
tool-change strategy

The ATC is a capability of the CNCk, not of the toolset.

## 12. Error and Action UX

Error/warning behavior:

- Persistent cross-screen summary banner
- Severity color coding:
  - Error: red
  - Warning: orange
- Summary view is compact and always visible
- Detailed diagnostics are available on click
- Errors and warnings clear automatically when conditions are resolved
- Initial expected empty-state error includes No tools in stock

Action feedback behavior:

- User actions and system events are logged
- Brief action summaries are surfaced in a bottom status/log area

## 13. KiCad Connection and PCB Selection Flow

Startup flow:

- Detect KiCad connection context
- If launched from KiCad PCB context, board is preselected and locked into the current project
- Otherwise, show selectable list of active KiCad PCB instances
- PCB list should refresh when dropdown is opened
- After selection, import PCB data and open or refresh the project's data
   - PCB data shall includde
      - Boards layers to determine effective thickness
      - Copper thickness, used to compensate for undersize of PTH holes
      - All 'Edge' layers
      - All holes, drills and vias - oblong and round, NPTH and PTH
         - Only through holes are manage.
         - Generate an error if burried vias are detected.
      - Locating holes meta information

Startup routing rules:

- Start with the project view
- If required profiles are missing, show blocking readiness tasks with direct links to create or select CNC, fixture, and process profiles.
- The system should avoid forcing a disconnected admin-first experience when a board is already open.

Refresh behavior:

- A refresh control retriggers board data acquisition and generation cycle

Failure behavior:

- If KiCad disconnect occurs after all required board/project input data has already been cached in the app, generation and UI state continue without interruption.
- If KiCad disconnect occurs before required input data is fully cached, the acquisition is treated as a connection failure:
  - Incomplete data is discarded.
  - Current acquisition attempt is aborted.
  - An explicit error is shown in diagnostics.

## 14. First-Run and Installation Flow

Fresh install minimum profile readiness:

1. CNC profile selection or creation (stock profile acceptable)
2. Add stock tools from catalog (required)
3. Fixture profile selection or creation
4. Process profile selection or creation

At minimum, valid generation requires one usable CNC profile, one fixture profile, one process profile, and sufficient stock tools for the requested operations.

First-run UX rules:

- The first meaningful destination is still the project workspace when a board is present.
- Missing required profiles are presented as a readiness checklist, not as an abstract admin task.
- The first project is expected to be iterative: users may bounce between project and profile editors while tuning CNC, fixture, and process profile definitions.
- The product should preserve board context while the user edits profiles for the first time.
- When no board is present, the product may start in profile/readiness mode.

## 15. Regeneration State Model

The UI should clearly represent generation states:

- Idle
- Generating
- Generation failed
- Generation paused/stopped

Generation performance expectation:

- Typical regeneration latency target is 1 to 2 seconds for normal board/project edits.

Generation output should refresh atomically per completed cycle.

## 16. UI Delivery Expectations

For UI generation/design workflows, deliverables should include:

- Information architecture map
- High-level wireframes for major screens
- Reusable component inventory with state variants
- Interaction flows for first-run, profile creation, live project editing, review, and export/send
- Error-state and empty-state variants
- Responsive behavior definition

Responsive rules:

- Desktop-first layout
- On narrower widths:
  - Collapse right settings panel into tabs/drawer
  - Preserve viewport priority
  - Keep quick access to error banner and primary actions

## 17. Out of Scope for This Document

This is a product and UX requirements document only.

Behavioral requirements are in scope here, including normative rules for selection, fallback, warnings, and error conditions.

- Internal algorithm implementation design (data structures, optimization approach, and execution internals)
- Internal threading/concurrency design
- Primitive rendering internals
- Data structure and module implementation details

These belong in architecture and engineering design documents.

## 18. Document Relationships

- Canonical source for product and UX requirements: this document
- Canonical source for technical design and implementation: architecture.md
- Derived, non-canonical UI brief: UI_Generator_Brief.md

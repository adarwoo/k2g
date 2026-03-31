# KiCad CNC GCode Plugin Specification

## 1. Introduction
This document specifies a portable KiCad plugin for Windows, Linux, and macOS.

- Implementation language: Rust
- KiCad integration: KiCad IPC library
- Core function: generate CNC GCode from the active KiCad PCB board

## 2. User Interface
The user interface is implemented in Rust with a React-like component model.

## 3. General UX Principles
The UI enforces the following principles:

- All actions and configuration are saved automatically.
- Existing errors and warnings are summarized at the top of the screen.
- Clicking an error or warning opens full context/details.
- Code generation is continuous and automatic.
   - Any relevant change triggers regeneration.
   - Generation can be stopped and restarted.
   - There is no explicit "Generate" button.
- Users can configure preferred units.
   - If the native value unit differs from user preference, conversion is shown automatically.

## 4. Screen UX Direction
The visual intent is similar to PrusaSlicer/Bambu Studio: a large viewport with configuration panels.

- The machining surface should be shown whenever applicable (for CNC workflows).
- Users should immediately see the effect of configuration changes.

### 4.1 Board View vs Program View
Both visual board feedback and raw GCode review are important.

- Board view and generated program view should be available on most screens.
- Alternating between views is acceptable.
- In some cases, users need to inspect board and program side-by-side.

## 5. Setup
Setup is opened from a wheel icon on the main page.
On first launch, setup is shown automatically.

### 5.1 General Settings
General settings include:

1. Units
    - Metric
       - Sizes: `mm`, `um`
       - Speeds: `mm/min`, `cm/min`, `m/min`
    - Imperial
       - Sizes: `mil`, `thou`, `in`
       - Speeds: `in/min`, `ipm`
2. Color scheme
    - Light
    - Dark

### 5.2 CNC Machine Management
Users can:

- Select a CNC profile
- Create a new CNC profile
   - Clone an existing profile
   - Start from a built-in template
- Delete a CNC profile
- Edit a CNC profile

When freshly installed, no machine exists, so the user must create one.

#### New CNC Profile Wizard
Clicking `+` opens a mini wizard:

- Shows stock templates (for example: Generic, 3040, Masso)
- Shows existing profiles available for cloning
- Requires a unique profile name
   - Clone default: `Copy of <profile name>`
   - Template default: `My <template profile name>`
- `New` button creates the profile
   - Disabled if name is not unique
   - Inline error explains why

After creation, profile editing starts immediately.
When a machine is selected, a `>` icon appears to return to the main screen.

### 5.3 CNC Machine Profile Fields
General section fields:

- Fixture plate max size: `X`, `Y`
- Max feed rate
- Spindle min/max RPM
- ATC slot count (`0` means ATC off)
- `X0` origin orientation: `Left` (or `Right`, `Front`, `Back`)
- `Y0` origin orientation: `Front` (or `Back`, `Left`, `Right`)
- Scaling `x`, `y` in `%` (default example: `100.0`, `100.0`)
- Program line numbering: `Yes/No`
   - Increment value (for example: `10`)

## 6. Program Section
The program section is organized around:

- Individual machining operations
- Generic function sub-sections

This section customizes per-operation GCode snippets.

- Low-level operations (for example, "Drill first hole following tool change") are written as machine-code excerpts.
- Strings between `{}` are interpreted as RHAI.
- RHAI receives all relevant variables/settings.
- In editors, typing `{` should open variable/function insertion suggestions.

Profiles behavior:

- Initial list item is `New`.
- Selecting `New` adds a next step where user chooses a profile to copy.
- Application ships with predefined profiles.
- New machine profiles can be cloned from existing profiles.

### 6.1 Sanity Check
The `Sanity check` function generates a string from current configuration.

- If output is non-empty text, job generation is blocked.
- The returned text is shown as an error.

### 6.2 Custom Attributes
Users can define CNC-specific custom attributes for jobs.

Supported attribute types:

- Boolean (`yes/no`)
- Free string
- List

Each attribute requires:

- Type (`bool`, `string`, `list`)
- Description
- Default value
- RHAI validation function
   - Receives all values
   - Returns a warning string

## 7. Main Application Flow
The application may notify startup errors in a popup.

Simplest/optimum flow (plugin already configured):

1. User starts the plugin.
2. User configures the job.
3. Program is generated automatically.
4. User downloads the program or sends it directly to the CNC.

## 8. Tooling and Job Pages

### 8.1 Stock Page
The stock page lists tools available for machining.
Each stock item includes:

- Name
- Auto-generated summary (`router bit`, `drill bit`, `end mill`, including V bits/grooving bits) + diameter
- Status: `In stock`, `In rack`, `Out of stock`, `New`
   - `In stock`: available and usable by program
   - `In rack`: currently present in rack based on last job
   - `Out of stock`: unavailable, should be reordered
   - `New`: recently added from catalog; becomes `In stock`/`In rack` once used
- Not prefered: A boolean to indicate the tool can be used if no other choice
- Operation counter
   - Routers/end mills: cumulative machining distance
   - Drill bits: number of holes
- Source item SKU

Adding tools:
- `+` button allows adding from catalog
   - The catalog offers 'generic' tools for drilling, milling, etc. which can be used to create custom tools
- Once added, the tool can be edited
- The tool name must be unique
   - When adding a tool, a unique name is generated
   - If changing the name, the <apply> is greyed if the name if not unique and the name become red -with the error message showing

- For catalog tools, properties are prefilled.
   - Some properties remain editable.
   - Manufacturer and SKU are read-only.
- If catalog item disappears, source info is greyed out.
- If a property differs from catalog default, original value is shown in grey (in brackets).

### 8.2 Catalog
The app includes a generic tool catalog (drill bits and routers, standard sizes).

- Catalog is organized in libraries (vendor/manufacturer/category grouping).
- Example libraries:
   - UnionFab drill bits metric
   - UnionFab router bits imperial
   - Generic drill bits
- Libraries cannot be edited in the UI; only used.
   - Libraries are YAML files and can be added manually.

Each catalog tool includes:

- Manufacturer + SKU
- Name/description
- Diameter with explicit unit expression
   - Supports metric/imperial and float/fraction forms
   - Examples: `0.65mm`, `1/8\"`, `150um`
   - Also displayed in user's preferred units if entered differently
- Recommended RPM
- Recommended Z feed rate
- Recommended horizontal feed rate (routers only)
- `Referenced` flag
   - If enabled, tool appears in stock (starts at 0 items)
- End mill type and geometry (required)

A bit graph is shown.

Catalog access:

- Opened from stock page while adding tools
- Supports selecting multiple tools
- Shown as overlay; closed with `<` back action

### 8.3 Job Page
The job page is where users decide what to produce.

Fields and controls:

- Selected CNC machine (click navigates to machine configuration)
- Job production types (any combination)
   - Drill locating pins
   - Drill PTH holes
   - Drill NPTH
   - Route board
   - Mill board
- CNC profile parameters (for example machine coordinate)
- Side to drill
   - Front
   - Back
- Board rotation
   - Auto (uses CNC preference)
   - Angle (`-180` to `+180`)
- If ATC is available, rack generation mode
   - Use manual tool change
   - Reuse rack
   - Overwrite rack (new rack for this job)
- Routing options (when routing selected)
   - Number of tabs (`0-n`)
      - `n` constrained to feasible values
      - If `0`, VGroove option becomes available
   - Tab width
   - Mouse-bite holes (`yes/no`)
   - Hole size (from stock)
      - Disabled if no stock tool smaller than route
   - Number of holes per tab (`1-n`)
      - Feasibility-constrained
      - Show center-to-center spacing in preferred unit
   - VGroove (if tabs = 0)
      - Tool selection from stock
      - Depth in `%` (`50-100`)
- Tool selection strategy
   - Oversize allowance `%` (example default `5%`)
      - Allows tool diameter up to oversize tolerance above target hole size
   - Undersize allowance `%` (example default `10%`)
      - Allows smaller tools for holes
   - Allow routing holes
      - If true, large holes can be routed when suitable router exists
      - Drill-then-route option
         - If true, drill first then enlarge by routing
   - Pilot hole
      - If no valid drill solution exists, use largest bit

### 8.4 Board View
Board graph shows all machinable elements.

- Any relevant change refreshes graph automatically.
- Holes, routes, and machining paths are filterable.
- Board view supports pan/zoom and "best fit" reset.
- Tabs can be moved.
   - Position is shown.
   - Position can be adjusted by click/drag.
   - Algorithm computes tab positions.

## 9. Data Generation and Program Tab
GCode generation is automatic.

- Any configuration change retriggers generation.
- Generation may fail; top-level error message indicates problems.

`Program` tab capabilities:

- Editable generated program window
- User can add/remove/comment sections
- Program can be:
   - Reviewed and edited
   - Saved to file
   - Saved to removable media (then eject button is available)
   - Sent over the air to a CNC machine

If user edits program manually and later makes a configuration change that would overwrite edits,
show a confirmation prompt so user can cancel and save work first.

## 10. Rack Configuration
Available only when selected CNC supports ATC.

Rack configuration allows users to:

- Define rack organization
- See changes required for current job
- Set explicit slot contents

Each slot can:

- Reference a stock tool
   - Optional lock to prevent replacement when rack is overwritten
- Be disabled (for example broken slot)

## 11. Error Notifications
An error/warning banner may appear on all pages.

- Initial expected error: `No tools in stock`
- Because generation is automatic, configuration changes may trigger new errors/warnings.
- Errors/warnings are cleared automatically when resolved.
- Warning severity is supported.
- Colors:
   - Error: red
   - Warning: orange
- Banner shows summary only (no scrolling required).
- Detailed messages are shown in the data generation view.
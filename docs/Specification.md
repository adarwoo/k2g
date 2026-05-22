# KiCad CNC GCode Plugin Specification

## 1. Introduction
This document documents all requirements identified for the K2G application.
This application is a portable application which doubles as a KiCad plugin.
The application/plugin targets Windows, Linux, and macOS environments.

The application core function is to generate a CNC GCode from a KiCad PCB board data.

## 2. Core technology
- Implementation language: Rust
- Configuration files and libraries format: YAML and YAML schemas.
- KiCad integration: KiCad IPC library
- The user interface is implemented in Rust with a React-like component model based on the Dioxus API.

## 3. General UX Principles
The UI enforces the following principles:

- All actions and configuration are saved automatically
- Existing errors and warnings are summarized at the top of the screen
- Changes are reported at the bottom of the screen
- Clicking an error or warning opens full context/details
- Code generation is continuous and automatic
   - Any relevant change triggers regeneration
   - There is no explicit "Generate" button
- Users can configure preferred units
   - If the native value unit differs from user preference, conversion is shown automatically.

## 4. Screen UX Direction
The visual intent is similar to PrusaSlicer/Bambu Studio: a large viewport with configuration panels.

- The machining surface should be shown whenever applicable (for CNC workflows).
- Users should immediately see the effect of configuration changes.

Note: Here are all the functions to include in the UX
1. Global setup
Units, language etc. at the application level
2. Job configuration
After selecting a job profile, allow overridding some of the pre-selections
3. Profiles
The profiles allow configuring the correct machining environment.
We have:
  - CNC profiles     - Defines a CNC machine
  - Fixture profiles - Defines how the PCB is fixed to the machining bed
  - Job profiles     - Define what should be machined
4. Job profile - Allow editing a given job profile
5. CNC profile - Allow editing a given CNC profile
Includes spindle properties (RPM etc.), rack or not etc.
6. Fixture profile - Allow editing a given fixture profile
7. Tools catalogs - Allow viewing, copying, cloning tools catalog
8. Stock - The stock is made of tools imported from tools catalogs
9. Job setup - Selection of the job profile.
The job profile includes what to do, the CNC and fixture.
In the job, it is possible to override some of the profiles settings.
10. Job code review
The generated GCode can be reviewed and edited. The changes are meta defined
11. Job machining review
Review what will be machined. A further iteration might display the operations.
12. PCB view
View the raw PCB data of interest - as imported from KiKAD.
13. Error log
Some of the profiles may generate errors (job profile tolerances, lack of tools etc.)
The pending errors and warning should show. Click one should expand to a full view.
14. Actions log
All actions shall be added to a log and shown breifly at the bottom of the screen.
15. PCB selection
If the application is started outside of KiCAD, the user should be able to select which PCB to machine.
This requires for 1 or more running KiCAD PCB instances.
As soon as a PCB is selected, the data is imported from KiCAD, and the GCode generated if possible.

### 4.1 Board View vs Program View
Both visual board feedback and raw GCode review are important.

- Board view and generated program view should is available on most screens.
- Alternating between views is done in the view port (top right selection toggling buttons).
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
   - Import a profile
   - Clone an existing profile
   - Start from a built-in template
- Delete a CNC profile (only created profiles. Stock profiles are readonly)
- Edit a CNC profile

#### New CNC Profile Wizard
Clicking `+` opens a mini wizard:

- Shows stock profiles (Generic, Genmitsu3040, Masso)
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
- Spindle start/stop delay. This is a RHAI field so it can be calculated.
- ATC slot count (`0` means ATC off)
- `X0` origin orientation: `Left` (or `Right`, `Front`, `Back`)
- `Y0` origin orientation: `Front` (or `Back`, `Left`, `Right`)
- Scaling `x`, `y` in `%` (default example: `100.0`, `100.0`)
- Program line numbering: `Yes/No`
   - Increment value (for example: `10`)

## 5.4 CNC profile Program Section
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

All RHAI sections can be dynamically edited by clicking |>. (They are read-only otherwise).
This displays a dialog listing all the variable passed to the function with default values.
Click on the variable adds it at the cursor position in the edit field.
The result value is show automatically, just like parsing errors.
The resulting output is tested to be valid GCode.

Each function comes with a pre-defined list of variables. Custom attributes are automatically added to the scope of all functions.

### 6.1 Sanity Check
The `Sanity check` function generates a string from current configuration.

- If output is non-empty text, job generation is blocked.
- The returned text is shown as an error.

### 5.5 Custom Attributes
Users can define CNC-specific custom attributes for jobs.

Supported attribute types:

- Boolean (`yes/no`)
- Free string
- List

Each attribute requires:

- Name (must be a valid RHAI variable name, and unique)
- Type (`bool`, `string`, `list`, `percent`, `number`, `date`)
- Description
- For the list type, the user can '+' values. This takes a name and a description
- Default value. For list, it must be chosen from the list, or no value which sets the content to ().
- RHAI validation/convertion function
   - Receives all values
   - Returns a value (could be the same) or throw an error
   Example: If a percentage is expected:
   Type: percent
   Description: Apply x,y scaling factor (%)
   Name: xy_scaling
   Validation:
      A RHAI function which gets executed when the value is edited.
      The function must throw a string on error.
      It can optionally return a value which becomes the value used.
      Example:
      if xy_scaling < 0.0 {
         throw `Percentage out of range: ${s}`;
      }
      Other example with transformation:
      if xy_scaling.is_float(): // Round the number
         int(xy_scaling)

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
- Source item SKU (Read-only - copied over from the catalog)
- Auto-generated summary (`router bit`, `drill bit`, `end mill`, including V bits/grooving bits) + diameter
- Status: `In stock`, `Out of stock`
   - `In stock`: available and usable by program
   - `Out of stock`: unavailable, should be reordered
- Prefered / Neutral / Not prefered: A selection to help the engine select the best tool
   - `Prefered` : If several tool are a good match, the 'Prefered' ones gets selected
   - `Neutral` : The default. Gets selected over 'Not prefered' but not over 'Prefered'
   - `Not prefered` : The engine will try to avoid this tools
- Operation counter (This counter can be reset)
   - Routers/end mills: cumulative machining distance
   - Drill bits: number of holes
- All properties of the tool can be edited

Adding tools:
- `+` button allows adding from catalog
   - The catalog offers 'generic' tools for drilling, milling, etc. which can be used to create custom tools
- Once added, the tool can be edited
- The tool name must be unique
   - When adding a tool, a unique name is generated
   - If changing the name, the <apply> is greyed if the name if not unique and the name become red -with the error message showing

- For catalog tools, properties are prefilled.
   - Some properties remain editable.
   - Catalog source and SKU are read-only.
- If a property differs from catalog default, original value is shown in grey (in brackets) and a 'refresh' icon button is added to allow reverting the change.

### 8.2 Catalog
The app includes some tool catalogs (drill bits and routers, standard sizes).

- A catalog has libraries (vendor/manufacturer/category grouping).
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

### 8.3 Job profiles Page
The job profiles page is where users decide what to produce.

Fields and controls:

- Selected CNC profile (click navigates to machine configuration)
   - Selected fixture for the PCB from the CNC profile list
      - Includes the backing board thickness
      - Includes the entry plate
   - CNC profile parameters (for example machine coordinate)
- Job production types (any combination)
   - Drill locating pins
   - Drill PTH holes
   - Drill NPTH
   - Route board
   - Mill board
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

## 12. The flow

### 12.1 Simple flow

When launched, the application needs to connect to a KiCAD instance which has a PCB available
This implies:
 - Scanning for all KiCad socket (unless passed in environment variable)
 - Checking for PCBs loaded
Unless started from a KiCAD PCB (the environment variable is set), the user should select the PCB from a dropdown list.
This list should be refreshed when the user drops it down.
If the application was started from KiCAD, the selection is greyed out.
A refresh icon is shown once the PCB data has been collected. Clicking on the button, restart a computation cycle:
 - Collection of the PCB data
 - Creation of the job data

Possible errors:
 - If the KiCad instance goes away whilst the data is being collected, the job is dropped, an error is reported (lost connection whilst ..) and the user needs to select from the list.

Once the PCB data is collected, the user can configure the job.
A job is created from a Job profile. (the job profile must have been created previously).
The Job profile preconfigures all that can be pre-configured.
The user can override any of the job profile settings in the job.
Any override shows by changing the color of the attribute to orange, and a small 'revert' icon gets added.

Example use case 1 - Drill PTH for a PCB
From KiCAD, the user clicks on the Plugin icon.
The application opens and the PCB data is automatically imported. The PCB name is shown in the list, but it cannot be changed. A refresh icon is added next to the name.
The user selects a job profile at the top of the screen from a dropdown list.
The last job profile is automatically selected. (Either the last used, or the last created, whichever comes last)
The job detail is shown with machining detail.
The user can review the settings, the generated code and rack, and send the GCode to the CNC (if the CNC allows it).

### 12.2 From installation

When the application is installed fresh, the user must create the following:
1. A CNC profile (optional - stock profile can be used)
2. A stock - mandatory.
   The user selects tools from stock catalogs
3. A job profile (optional - standard profiles exists, using standard CNC profiles)

So as a minimum, the user must add tools to the stock from catalog.

# Technical implementation details

1. The code shall be fully coded in Rust
All resource files are built in the library
2. All dynamic parsing in the application shall be managed by RHAI
3. When the source of a PCB becomes known, all data are read and stored in memory.
4. A stiching algorithm is used to connect all items on the same layer (arc, bezier, lines)
5. The GCode generation uses a travelling salesman problem sorting algorithm leveraging existing library
6. KiCAD is accessed using the IPC mode. The library used is kicad-ipc-rs
7. All configuration data, stock, machines etc are specified in Yaml
All Yaml calls for a schema file in Yaml too

## Application startup

### Parsing of configuration
When the application starts it first parses all the configuration files.
Any error during parsing rejects the file and insert an transient error in the log. If the errored file is internal, an error diagnostic is display and the application terminates once the user has acknoledged the error.
The external errored file is renamed (appending .error). If a file exist, it is deleted.
If during the processing of the error file, an error occurs (cannot delete, rename etc.), a transient error is added to the log.

### Processing of PCB data
As soon as the PCB data can be transfered (application started with the KiCAD environment varaible set) or selection of the PCB change by the user, the data is acquired.
The application checks for a valid board outline (Stiching is applied to the edge items).
An error is generated if the outline is not valid - and the board cannot be processed.
The following data is of interest:
 * All holes for drilling (pads and holes)
 * All Edge items (lines, arcs, bezier)

Note: This version does do the trace routing for now.


### Program generation
Once the user selects job operations, the GCode program is automatically generated.
The generation is a background tasks to prevent blocking the UI during generation. The generation could be dropped if the user is making a change requiring to regenerated.
The generation can generate errors.
These errors are linked to the generation.
They are cleared when a new generation is started.
When the generation completes, the result is loaded.
The results are clear when a new generation starts.

#### Program algorithm
The jobs are organised to yeild a generation driven for per job:
1 - Drilling jobs (all about holes)
2 - Contouring jobs (routing, scoring, tabs)
3 - Engraving will be added later on

#### Primitives
All operations generated from the job types generate primitive instrutions.
These primitives are only converted by the CNC using the RHAI expression in the final pass.
A primitive is like:
 * initialise
 * Move slow to x,y
 * Start spindle
 * Drill
 * PeckDrill
 * CutArc
 * CutBezier
 * Start spindle
 * Change tool
 * conclude

Example: CutBezier
Some CNC can accept bezier G3.4 on Siemens - others cannot.
The primitive converts into GCode which the CNC can execute.
For Bezier, the RHAI can call a built-in primitive bezier-to-arcs.

#### Primitive rendering
Primitive are used in given context: CNC used, tool used.
When the primitive renders, it leverages the context.
The feed rate is obtained from the current tool:
G1 23 23 F${cuttingTool.feedrate.mmmin}


#### Drilling
1. A list of holes is created
2. Holes are mapped to tools - including holes that needs routing
3. For each tool, TSP creates the drilling order

Then the program is generated using:
Header + foreach hole : [TC + Hole (drill/route)] + Footer

The code generation generates an ordered list of primitives.

#### Countouring
The is cutting the edge.
The number of passes is calculated.


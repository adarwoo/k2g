# AI UI Generator Brief

## 1. Product Context
Create a desktop-style interface for a KiCad plugin that generates CNC GCode from PCB designs.

- Platforms: Windows, Linux, macOS
- Primary users: PCB makers, machinists
- Workflow style: high-feedback, continuously regenerated output

## 2. UX Goals

- Prioritize a large visual machining viewport.
- Keep job settings easy to edit from side panels.
- Surface validation issues early and globally.
- Auto-save all settings and avoid explicit save flows for configuration.
- Auto-regenerate GCode on changes (no Generate button).

## 3. Visual Direction
Use a slicer-inspired workstation layout (PrusaSlicer/Bambu-like in structure, not branding).

- Dense professional tool UI
- Viewport-dominant composition
- Persistent status + error layer
- Fast switch between board visualization and program text

## 4. Global Layout

- Top bar
  - Project context
  - CNC profile selector (or quick link)
  - Setup (wheel icon)
  - Global status indicators
- Main body
  - Left panel: navigation and major workflow sections
  - Center: board/machining viewport or GCode editor
  - Right panel: context settings and editable parameters
- Bottom or side utility area
  - Generation status
  - Job progress/queue indicator if needed

## 5. Core Navigation

- Setup
- Stock
- Catalog (overlay from Stock)
- Job
- Board View
- Program
- Rack (only when ATC supported)

Support rapid switching between:

- Board visualization mode
- Program text mode

## 6. Shared Interaction Rules

- All field edits persist automatically.
- Relevant edits retrigger generation automatically.
- Generation can be stopped/restarted.
- Errors and warnings are summarized in a persistent banner.
- Clicking banner summary opens detailed diagnostics.
- Unit preferences apply globally with automatic conversion display.

## 7. Setup Screen Requirements

### 7.1 General Settings

- Unit system selector
  - Metric (`mm`, `um`; speeds `mm/min`, `cm/min`, `m/min`)
  - Imperial (`mil`, `thou`, `in`; speeds `in/min`, `ipm`)
- Theme selector
  - Light
  - Dark

### 7.2 CNC Machine Management

- List of machine profiles
- Actions:
  - Select
  - Create
  - Clone
  - Delete
  - Edit
- First launch behavior:
  - If no machine exists, force onboarding to machine creation

### 7.3 New Machine Wizard (modal/popup)

- Source options:
  - Built-in templates (for example Generic, 3040, Masso)
  - Existing profiles (clone)
- Name input with uniqueness validation
  - Clone default naming: `Copy of <profile name>`
  - Template default naming: `My <template profile name>`
- `New` CTA disabled until valid
- Inline validation message for name conflicts

### 7.4 Machine Profile Form

Fields:

- Fixture plate max size (`X`, `Y`)
- Max feed rate
- Spindle min/max RPM
- ATC slot count (`0` disables ATC)
- Origin orientation
  - `X0`: Left/Right/Front/Back
  - `Y0`: Front/Back/Left/Right
- XY scaling percent
- Program line numbering toggle + increment

## 8. Stock Screen Requirements

- Tool list/table with rich status
- Per-tool attributes:
  - Name
  - Auto summary (type + diameter)
  - Status (`In stock`, `In rack`, `Out of stock`, `New`, `Not preferred`)
  - Operation counter (distance or hole count)
  - Catalog link/source state
- Add tool action (`+`)
  - Add from catalog
  - Add manual
- Editing behavior
  - Required fields
  - Unique name validation
  - Catalog-derived fields prefilled
  - Manufacturer + SKU read-only
  - If value diverges from catalog default, show previous value in muted style

## 9. Catalog Overlay Requirements

- Opened from Stock add flow
- Tool library browser (read-only libraries)
- Tool detail includes:
  - Manufacturer + SKU
  - Name/description
  - Diameter with explicit units/fractions support
  - Recommended RPM
  - Recommended feed rates
  - Geometry/type where applicable
  - Referenced toggle
- Supports selecting multiple tools before returning
- Close behavior: back action (`<`)

## 10. Job Screen Requirements

- Show selected CNC profile with quick link to configuration
- Production operation toggles (multi-select):
  - Drill locating pins
  - Drill PTH
  - Drill NPTH
  - Route board
  - Mill board
- Side selection: Front/Back
- Rotation:
  - Auto
  - Manual angle (`-180` to `+180`)
- ATC rack strategy (if ATC available):
  - Reuse rack
  - Overwrite rack

Routing subpanel (when routing enabled):

- Tab count (`0-n`, feasibility-limited)
- Tab width
- Mouse-bite toggle
- Mouse-bite hole size selector
  - Disabled if no compatible stock tool
- Holes per tab (`1-n`, feasibility-limited)
- Computed center-to-center distance display
- If tab count is `0`, show VGroove options:
  - Tool selector
  - Depth percent (`50-100`)

Tool selection strategy controls:

- Oversize allowance percent (default example `5%`)
- Undersize allowance percent (default example `10%`)
- Allow routing holes toggle
- Drill-then-route toggle
- Pilot hole fallback behavior

## 11. Board View Requirements

- Large interactive PCB machining visualization
- Layers/filter toggles for holes, routes, paths
- Pan/zoom controls + reset-to-fit
- Editable tab markers
  - Drag to reposition
  - Numeric/manual adjustment
  - Algorithmic placement preview

## 12. Program Tab Requirements

- Monospace code editor for generated GCode
- User can add/remove/comment sections
- Primary actions:
  - Save to file
  - Save to removable media
  - Eject media (when applicable)
  - Send over network to CNC
- Overwrite protection
  - If user edited code and regeneration would replace edits, show confirmation dialog

## 13. Rack Configuration Screen Requirements
Show only when machine has ATC enabled.

- Rack slot grid/list
- Per-slot options:
  - Assign stock tool
  - Lock slot
  - Disable slot (broken/unavailable)
- Job impact panel showing required rack changes

## 14. Error and Warning UX

- Persistent banner visible across screens
- Color coding:
  - Errors: red
  - Warnings: orange
- Summary-only compact banner
- Click to open detailed diagnostics panel
- Initial common empty-state error: `No tools in stock`
- Errors/warnings auto-clear when conditions are resolved

## 15. State and Regeneration Model

- Auto-save: always on
- Regeneration trigger: any relevant configuration mutation
- Regeneration states:
  - Idle
  - Generating
  - Generation failed
  - Generation paused/stopped
- UI should clearly indicate current state and allow stop/restart

## 16. Components Inventory (for generator)

- App shell (top bar, nav, panels)
- Setup modal/screen
- Machine profile wizard modal
- Form controls (unit-aware numeric inputs, segmented controls, toggles)
- Stock data table with status chips
- Catalog overlay with searchable list and detail pane
- Job configuration accordion/panels
- Board viewport canvas with overlays
- GCode editor panel
- Rack slot matrix component
- Global error/warning banner
- Detailed diagnostics drawer/modal

## 17. Responsive Behavior

- Desktop first
- On narrower widths:
  - Collapse right settings panel into tabs or drawer
  - Keep viewport usable with priority sizing
  - Preserve quick access to error banner and primary actions

## 18. Output Expectations for UI Generator
Generate:

- Information architecture map
- High-level wireframes for each major screen
- Reusable component library with states
- Interaction flow for first-run setup, tool selection, job setup, review, and export/send
- Error-state variants and empty-state variants

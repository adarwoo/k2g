# K2G Unified Product Specification

Status: Canonical product, UX, technical, and architectural requirements source.

## 0. Document Control

### 0.1 Status

This document is the canonical requirements source for K2G.

- `FR-CTRL-001` This specification shall define the normative product behavior for K2G across product, UX, technical, and architectural concerns.
- `FR-CTRL-002` Where this document conflicts with ad hoc notes or generated briefs, this document shall take precedence.

### 0.2 Scope

K2G is a portable desktop application and KiCad plugin that generates CNC GCode from PCB designs.

- `FR-CTRL-003` The product scope shall include PCB acquisition, profile management, stock and catalog management, project configuration, machining preview, GCode generation, and project save/load workflows.
- `FR-CTRL-004` The product shall support Windows, Linux, and macOS.
- `FR-CTRL-005` This specification shall cover behavioral requirements, warnings, fallback behavior, and user-facing constraints.
- `FR-CTRL-006` Low-level implementation details, internal threading design, and module internals are out of scope for product sections unless elevated into technical or architectural requirements in Part III or Part IV.

### 0.3 Definitions of Requirement Levels

- `FR-CTRL-007` `shall` and `must` indicate mandatory requirements.
- `FR-CTRL-008` `should` indicates a strong recommendation that may only be violated with explicit rationale.
- `FR-CTRL-009` `may` indicates an allowed optional behavior.
- `FR-CTRL-010` Requirement identifiers shall be stable and use these families: `FR-*` for functional, `UX-*` for UI and UX, `TR-*` for technical, and `AR-*` for architectural requirements.

### 0.4 Relationship to architecture.md and UI_Generator_Brief.md

- `FR-CTRL-011` [docs/architecture.md](/workspaces/k2g/docs/architecture.md) shall remain the canonical implementation and system design companion to this document.
- `FR-CTRL-012` `UI_Generator_Brief.md`, when present, shall be treated as a derived, non-canonical UI brief generated from this document.
- `FR-CTRL-013` Architecture and UI brief documents shall not weaken or override requirements defined here.

---

# Part I - Product and Functional Requirements

## 1. Product Context

- `FR-CTX-001` K2G shall provide a high-feedback workflow for converting PCB designs into CNC machining output.
- `FR-CTX-002` The primary user groups shall be PCB makers and machinists.
- `FR-CTX-003` The default operating model shall assume continuous reconfiguration and frequent regeneration while the user edits inputs.

See also:
- `UX-PROJ-001` for project workspace behavior.
- `AR-PIPE-001` for generation pipeline architecture.

## 2. Product Principles

- `FR-PRIN-001` Relevant settings changes shall be auto-saved.
- `FR-PRIN-002` Relevant input mutations shall retrigger GCode generation automatically.
- `FR-PRIN-003` The product shall keep machining feedback visible while configuration is edited.
- `FR-PRIN-004` Errors and warnings shall remain visible in summary form and drillable to full detail.
- `FR-PRIN-005` Units shall be user-configurable and displayed consistently across the application.
- `FR-PRIN-006` The system shall preserve deterministic output for identical effective inputs.

See also:
- `UX-DIAG-001` for diagnostics visibility.
- `TR-DET-001` for determinism requirements.

## 3. Domain Terminology

The following domain terms are normative for this specification.

- `Project`: The live machining workspace for one selected board.
- `Process profile`: A persistent definition of machining defaults, constraints, supported operations, and profile references.
- `CNC profile`: A persistent machine definition including machine limits, primitive templates, and GCode customization.
- `Fixture profile`: A persistent definition of how the PCB is held, aligned, and constrained physically.
- `Toolset profile`: A persistent tooling allocation and tooling policy definition.
- `Stock tool`: A concrete tool instance available to the user for machining.
- `Catalog tool`: A read-only library tool definition that can be copied into stock.
- `Temporary profile`: A project-scoped profile reconstructed from an embedded project copy when a persistent profile cannot be resolved safely.
- `Effective value`: The resolved runtime value visible to generation after applying profile defaults and current project overrides.

## 4. Core Domain Model

- `FR-DOM-001` Exactly one active project shall exist at a time in the application context.
- `FR-DOM-002` A project shall combine one board, one selected process profile, referenced CNC and fixture profiles, optional toolset profile selection, runtime overrides, generated outputs, and diagnostics.
- `FR-DOM-003` Persistent profiles shall be reusable assets not tied to a single board.
- `FR-DOM-004` Stock tools shall be independent records even when originally copied from a catalog entry.
- `FR-DOM-005` Runtime overrides shall affect the active project instance without silently mutating the underlying persistent profile.

See also:
- `TR-PROFILE-001` for UUID identity.
- `AR-STATE-001` for state ownership rules.

## 5. Project Functional Requirements

- `FR-PROJ-001` The project shall be the default machining workspace when the application is launched from an open PCB.
- `FR-PROJ-002` The project shall require one selected process profile before valid generation can proceed.
- `FR-PROJ-003` The last used process profile should be reused on reopen when still available.
- `FR-PROJ-004` A profile shall not be rejected at selection time merely because current board constraints may later fail generation.
- `FR-PROJ-005` Feasibility shall be determined during generation, with detailed diagnostics when constraints are violated.
- `FR-PROJ-006` The project shall expose controls for enabled operations, side selection, board rotation, tooling strategy, routing settings, and diagnostics.
- `FR-PROJ-007` Board rotation shall support auto mode, fixed right-angle selections, and free-entry values from `-180.0` to `+180.0` degrees.
- `FR-PROJ-008` In auto rotation mode, the resolved rotation from the last successful generation shall be written back to project state and shown in the UI.
- `FR-PROJ-009` CNC and fixture profiles shall be inherited from the process profile and shall not be overridden directly on the live project.
- `FR-PROJ-010` Other overridable profile-defined attributes may be overridden on the live project.
- `FR-PROJ-011` If a selected process profile is deleted while in use, the project shall return to process-profile-selection state and reset generation.

See also:
- `UX-PROJ-001` for workspace composition.
- `UX-OVR-001` for override presentation.
- `AR-LIFE-001` for active project lifecycle.

## 6. Process Profile Functional Requirements

- `FR-PROC-001` A process profile shall define machining defaults and constraints for a family of machining operations.
- `FR-PROC-002` A process profile shall include a fixed CNC profile reference.
- `FR-PROC-003` A process profile shall include constrained fixture profile references using `Fixed`, `List`, or `Any` modes.
- `FR-PROC-004` A process profile shall include constrained toolset profile references using `Fixed`, `List`, `Any`, or `Auto` modes.
- `FR-PROC-005` A process profile shall define default enabled operations, machining strategies, tool selection parameters, routing and tab settings, and supported feature selection.
- `FR-PROC-006` Process profiles shall be creatable by clone or template when available.
- `FR-PROC-007` Deleting a process profile shall be allowed even when referenced, but it shall require cascade analysis and explicit confirmation.
- `FR-PROC-008` Process profiles shall be persisted as YAML and validated against `process_profile.schema.yaml`.
- `FR-PROC-009` Process profiles shall initialize the effective project values when selected.
- `FR-PROC-010` Process profiles shall not support `List`, `Any`, or `New` modes for CNC profile references.

See also:
- `TR-REF-001` for profile reference modes.
- `TR-SCHEMA-001` for persistence requirements.
- `AR-CASCADE-001` for cascade deletion architecture.

## 7. CNC Profile Functional Requirements

- `FR-CNC-001` CNC profiles shall be manageable as persistent assets from the primary navigation.
- `FR-CNC-002` CNC profiles shall support add-from-built-in, import, export, duplicate, edit, and delete actions.
- `FR-CNC-003` Built-in CNC profiles shall be read-only.
- `FR-CNC-004` Added CNC profiles shall be editable.
- `FR-CNC-005` CNC profile general fields shall include machine envelope, feed limits, spindle limits, spindle delays, ATC slot count, origin orientation, XY scaling percent, and program line numbering controls.
- `FR-CNC-006` A CNC profile shall contain machine primitive templates and expression-backed program sections used to render final GCode.
- `FR-CNC-007` CNC profile custom attributes shall support `bool`, `string`, `list`, `percent`, `number`, and `date` types with metadata, defaults, and optional validation or transformation expressions.
- `FR-CNC-008` A configured CNC sanity check function shall return a string, and any non-empty result shall block generation and surface as an error.
- `FR-CNC-009` Deleting a CNC profile shall be allowed when referenced only with explicit cascade confirmation.

See also:
- `UX-CNC-001` for CNC profile management UX.
- `TR-RHAI-001` for the expression model.
- `TR-PRIM-001` for primitive template requirements.

## 8. Fixture Profile Functional Requirements

- `FR-FIX-001` Fixture profiles shall describe how the PCB is physically held and aligned on the machine.
- `FR-FIX-002` Fixture profiles shall be persistent reusable configuration assets rather than board-specific state.
- `FR-FIX-003` Fixture influence on generated output shall flow through CNC abstractions rather than direct machine-specific emission.
- `FR-FIX-004` Fixture fields shall include fixture name, holding method, work origin definition, locating pin strategy and geometry, clamp or keep-out zones, fixture occupancy, and optional probing or alignment parameters.
- `FR-FIX-005` Fixture profiles shall support select, create, clone, template-based creation when available, edit, and delete actions.
- `FR-FIX-006` Deleting a fixture profile shall be allowed when referenced only with explicit cascade confirmation.
- `FR-FIX-007` Fixture profiles shall be persisted as YAML and validated against `fixture_profile.schema.yaml`.

See also:
- `UX-FIX-001` for fixture profile UX.
- `AR-CASCADE-001` for cascade deletion behavior.

## 9. Toolset Profile Functional Requirements

- `FR-TOOLSET-001` Toolset profiles shall define preferred tooling, allowed tooling, ATC tooling strategies, reusable tool sets, and machining-time tooling behavior.
- `FR-TOOLSET-002` A toolset profile shall define slots `T1..Tx` and the policy behavior for generation.
- `FR-TOOLSET-003` Each toolset slot shall be in exactly one state: fixed tool, spare, or do not use.
- `FR-TOOLSET-004` Toolset profiles may exceed the physical ATC capacity of the current CNC.
- `FR-TOOLSET-005` Toolset generation policies shall include `Single Rack Only`, `Allow Reload`, and `Allow Manual Changes`.
- `FR-TOOLSET-006` Under `Single Rack Only`, generation shall fail when required tooling exceeds available tooling.
- `FR-TOOLSET-007` Under `Allow Reload`, generation shall permit paused workflows that require operator rack reload actions.
- `FR-TOOLSET-008` Under `Allow Manual Changes`, generation shall permit transitioning from ATC-managed tools to manual insertion workflows.
- `FR-TOOLSET-009` Process profiles shall support the ad hoc toolset reference value `Auto`.
- `FR-TOOLSET-010` `Auto` toolset selection shall permit project-specific optimal tool allocation without predefined slot assumptions.

See also:
- `UX-TOOLSET-001` for toolset editing UX.
- `TR-VAL-001` for validation rules.

## 10. Stock Functional Requirements

- `FR-STOCK-001` Stock tools shall be presented in one unified table across all tool families.
- `FR-STOCK-002` The stock table shall support faceted filtering, text search, configurable columns, sorting, and saved filter presets.
- `FR-STOCK-003` Tool families shall include at minimum drill, router, V-bit, and engraving.
- `FR-STOCK-004` Each stock tool shall track tool family, composite label, optional practical name, source catalog, source SKU, availability status, selection preference, copied metadata, and editable stock properties.
- `FR-STOCK-005` Catalog-derived stock tools shall be copies rather than live references.
- `FR-STOCK-006` Deleting a catalog or catalog entry shall not modify existing stock tools.
- `FR-STOCK-007` Stock shall support multi-selection and bulk delete with explicit confirmation.
- `FR-STOCK-008` Double-clicking a stock item shall open a dedicated detail editor for that tool.
- `FR-STOCK-009` Editing shall validate and commit on Enter, revert on Escape, and retain focus on validation failure.
- `FR-STOCK-010` Any stock mutation that affects generation inputs shall trigger regeneration.
- `FR-STOCK-011` Stock field validity shall enforce positive diameter, valid optional feed rates, bounded tip geometry, and non-negative optional spindle speed.
- `FR-STOCK-012` When the active CNC has ATC capability, stock shall surface expected rack assignment indicators.

See also:
- `UX-STOCK-001` for stock and catalog UX.
- `TR-UNIT-001` for measurement entry rules.
- `TR-VAL-001` for validation semantics.

## 11. Catalog Functional Requirements

- `FR-CAT-001` The catalog shall open from the stock add flow as an overlay or equivalent transient surface.
- `FR-CAT-002` Catalog libraries shall be read-only in the application and supplied externally via YAML files.
- `FR-CAT-003` Catalog detail views shall show manufacturer, SKU, name, description, geometry, diameter, and recommended machining values.
- `FR-CAT-004` The catalog shall support multi-selection and bulk import into stock.
- `FR-CAT-005` Selecting one catalog tool shall expose a detail view with a direct add action.
- `FR-CAT-006` Imported metadata shall remain informational in the resulting stock copy unless explicitly editable in stock.

See also:
- `UX-STOCK-006` for catalog overlay behavior.
- `TR-SCHEMA-004` for catalog YAML and schema expectations.

## 12. PCB Acquisition Functional Requirements

- `FR-PCB-001` On startup, the system shall detect KiCad connection context.
- `FR-PCB-002` When launched from KiCad PCB context, the current board shall be preselected and locked to the active project.
- `FR-PCB-003` When not launched from KiCad PCB context, the product shall allow selecting from active KiCad PCB instances.
- `FR-PCB-004` The PCB selector shall refresh its candidate list when opened.
- `FR-PCB-005` Board acquisition shall include board layers, effective thickness inputs, copper thickness used for PTH compensation, all edge layers, supported through-hole geometries, and locating-hole metadata.
- `FR-PCB-006` Buried vias shall raise an explicit error for the current version.
- `FR-PCB-007` A refresh action shall retrigger PCB acquisition and generation.
- `FR-PCB-008` If KiCad disconnects after required data has been cached, the project shall continue operating on cached data.
- `FR-PCB-009` If KiCad disconnects before required data is cached, incomplete acquisition data shall be discarded and an explicit error shall be raised.

See also:
- `AR-KICAD-001` for KiCad connection architecture.
- `AR-OFFLINE-001` for offline reproducibility.

## 13. GCode Generation Functional Requirements

- `FR-GEN-001` Relevant project, profile, stock, or board mutations shall trigger automatic regeneration.
- `FR-GEN-002` Generation shall run in a single-flight model where only one cycle may commit output.
- `FR-GEN-003` When a new relevant mutation arrives during generation, the in-progress cycle shall be cancelled or superseded and only the newest cycle may commit results.
- `FR-GEN-004` Generation shall produce output by project type in this order: drilling, contouring, then engraving when implemented.
- `FR-GEN-005` Within each project type, operation ordering shall minimize travel through deterministic route optimization.
- `FR-GEN-006` All project types shall emit an ordered list of machining primitives before final GCode rendering.
- `FR-GEN-007` The primitive set shall include `initialise`, `move_slow`, `start_spindle`, `stop_spindle`, `drill`, `peck_drill`, `cut_arc`, `cut_bezier`, `change_tool`, and `conclude`.
- `FR-GEN-008` The board outline shall be valid and closed before generation may proceed.
- `FR-GEN-009` Generation failures shall produce actionable diagnostics and shall prevent stale or partial new output from being committed.
- `FR-GEN-010` Typical regeneration latency should target one to two seconds for normal edits.

See also:
- `TR-PRIM-001` for primitive template requirements.
- `TR-TOOL-001` for tool selection algorithm requirements.
- `AR-PIPE-001` for generation pipeline architecture.

## 14. Saved Project Functional Requirements

- `FR-SAVE-001` Projects shall be savable and reopenable independently of a live KiCad connection.
- `FR-SAVE-002` A saved project shall contain a PCB snapshot, selected process profile, resolved profile selections, runtime overrides, generation settings, generated outputs, and diagnostics metadata.
- `FR-SAVE-003` A saved project shall embed every referenced profile required for reproducibility.
- `FR-SAVE-004` Embedded profiles shall include UUID, complete configuration, and version metadata.
- `FR-SAVE-005` Project load shall first attempt UUID-based resolution against the profile database and shall fall back to embedded copies when necessary.
- `FR-SAVE-006` If a database profile exists with differing version metadata, the user shall be offered resolution options to use the current profile, update from the project copy, or use a temporary profile.
- `FR-SAVE-007` Missing referenced profiles shall be reconstructed as temporary profiles for the lifetime of the opened project.
- `FR-SAVE-008` Temporary profiles shall not be persisted automatically when the project is closed.
- `FR-SAVE-009` Closing a project shall remove active temporary profiles unless they were explicitly promoted.
- `FR-SAVE-010` When a project is reopened, KiCad connection shall be turned off by default to avoid confusion about the authoritative PCB source.

See also:
- `UX-TOPBAR-003` for temporary-profile indication.
- `TR-PKG-001` for packaging requirements.
- `AR-LOAD-001` for load and resolution architecture.

---

# Part II - UI and UX Requirements

## 15. Information Architecture

- `UX-IA-001` User interface requirement: Primary navigation shall expose Project, Process Profiles, CNC Profiles, Fixture Profiles, Toolset Profiles, Stock, and Catalog access.
- `UX-IA-002` User interface requirement: Catalog access should appear as part of the stock workflow rather than as a detached primary workspace.
- `UX-IA-003` User interface requirement: Project subviews shall include Board View, Program View, Split View, and tooling-related views when relevant.
- `UX-IA-004` User interface requirement: The information architecture shall prioritize active machining work over administrative setup flows.

## 16. Global Layout

- `UX-LAYOUT-001` User interface requirement: The desktop layout shall follow a slicer-style workstation structure with persistent context and a dominant central workspace.
- `UX-LAYOUT-002` User interface requirement: The top area shall present active context, profile summary, units, theme access, and global status.
- `UX-LAYOUT-003` User interface requirement: The main body shall place primary navigation on the left, viewport or program content in the center, and context settings plus diagnostics on the right.
- `UX-LAYOUT-004` User interface requirement: A utility area shall expose generation status and brief action feedback.
- `UX-LAYOUT-005` User interface requirement: Entering from KiCad shall land the user in the project view rather than a detached settings area.

## 17. Navigation Model

- `UX-NAV-001` User interface requirement: There shall be no separate setup workspace detached from primary navigation.
- `UX-NAV-002` User interface requirement: Persistent assets shall be managed directly from their primary navigation destinations.
- `UX-NAV-003` User interface requirement: Project-initiated profile editing should use overlays, drawers, or focused subpages that preserve board context when practical.
- `UX-NAV-004` User interface requirement: The user should not feel forced to leave the machining task merely to adjust a profile.

## 18. Top Bar and Active Context Display

- `UX-TOPBAR-001` User interface requirement: The top bar shall display PCB or project name, active process profile, active CNC profile, active fixture profile, active toolset when relevant, unit quick-toggle, theme control, and global status.
- `UX-TOPBAR-002` User interface requirement: Global defaults shall be accessed from a top-bar settings control and shall contain only theme and display-unit preferences.
- `UX-TOPBAR-003` User interface requirement: When a temporary profile is active, the top bar shall show a visible warning indicator until the profile is promoted or the project is closed.
- `UX-TOPBAR-004` User interface requirement: Active context display shall remain visible across major screens.

See also:
- `TR-TEMP-001` for temporary profile semantics.
- `FR-SAVE-007` for temporary-profile creation.

## 19. Project Workspace UX

- `UX-PROJ-001` User interface requirement: The project workspace shall keep the board and machining result visually central at all times.
- `UX-PROJ-002` User interface requirement: The left side shall expose project navigation and section switching.
- `UX-PROJ-003` User interface requirement: The center shall show the board viewport, program editor, or split mode content.
- `UX-PROJ-004` User interface requirement: The right side shall group live project controls by meaning, including operations, placement, tooling strategy, routing, and diagnostics.
- `UX-PROJ-005` User interface requirement: Missing required profiles shall appear as blocking readiness tasks with direct actions rather than as abstract admin instructions.
- `UX-PROJ-006` User interface requirement: The user shall be able to jump from project context to relevant profile editors quickly.

## 20. Board View UX

- `UX-BOARD-001` User interface requirement: Board View shall display machinable elements with direct visual feedback.
- `UX-BOARD-002` User interface requirement: Board View shall auto-refresh on relevant changes.
- `UX-BOARD-003` User interface requirement: Board View shall support filter toggles for holes, routes, and paths.
- `UX-BOARD-004` User interface requirement: Board View shall support pan, zoom, and reset-to-fit.
- `UX-BOARD-005` User interface requirement: Tab markers shall support both visual adjustment and numeric editing.

## 21. Program View UX

- `UX-PROGRAM-001` User interface requirement: Program View shall expose generated GCode in a monospace editor only after successful generation.
- `UX-PROGRAM-002` User interface requirement: Program editing shall be disabled during active generation.
- `UX-PROGRAM-003` User interface requirement: When user edits exist and regeneration would overwrite them, the user shall receive an explicit confirmation prompt.
- `UX-PROGRAM-004` User interface requirement: If generation fails after program invalidation, the editor shall show an empty or failed state together with diagnostics rather than silently retaining stale content.
- `UX-PROGRAM-005` User interface requirement: Program actions shall include save to file, save to removable media, eject media when applicable, and send over network to CNC.

## 22. Process Profile Management UX

- `UX-PROC-001` User interface requirement: Process profile management shall support select, create, clone, template-based creation when available, edit, and delete flows.
- `UX-PROC-002` User interface requirement: Creating a new profile shall use a wizard or modal that shows templates, clone sources, default naming, and inline validation.
- `UX-PROC-003` User interface requirement: New-profile confirmation shall remain disabled until validation passes.
- `UX-PROC-004` User interface requirement: After creation, editing shall begin immediately.
- `UX-PROC-005` User interface requirement: Deletion confirmation shall enumerate dependent assets and impacted live projects before the final destructive action is enabled.

## 23. CNC Profile Management UX

- `UX-CNC-001` User interface requirement: The CNC Profiles page shall use a split layout with actions and profile list above or beside the editor.
- `UX-CNC-002` User interface requirement: Built-in profiles shall display in read-only mode.
- `UX-CNC-003` User interface requirement: Added profiles shall display in editable mode.
- `UX-CNC-004` User interface requirement: Expression-backed sections shall surface parsing feedback and available variables while being edited.
- `UX-CNC-005` User interface requirement: Protected or built-in sections may use an explicit read-only to editable toggle where applicable.

## 24. Fixture Profile Management UX

- `UX-FIX-001` User interface requirement: Fixture profile management shall support select, create, clone, template-based creation when available, edit, and delete flows.
- `UX-FIX-002` User interface requirement: Fixture editing shall present alignment, hold-down, occupancy, and optional probing parameters in a way that makes the physical setup legible.
- `UX-FIX-003` User interface requirement: Destructive deletion shall use the same cascade-warning pattern as other profile types.

## 25. Toolset Profile Management UX

- `UX-TOOLSET-001` User interface requirement: The toolset editor shall support arbitrary slot counts.
- `UX-TOOLSET-002` User interface requirement: Toolset slots shall be editable through rows, drag and drop, or an equivalently direct slot-management interaction.
- `UX-TOOLSET-003` User interface requirement: Each slot shall show slot index, assigned tool, and slot mode.
- `UX-TOOLSET-004` User interface requirement: Slot states shall be visually distinct for fixed tool, spare, and do-not-use states.

## 26. Stock and Catalog UX

- `UX-STOCK-001` User interface requirement: The stock list shall be a single unified table rather than separate lists per tool family.
- `UX-STOCK-002` User interface requirement: The stock table shall allow sorting by type, size, and addition precedence with deterministic within-type ordering.
- `UX-STOCK-003` User interface requirement: Tool-family color coding shall remain visible in the list.
- `UX-STOCK-004` User interface requirement: Selecting one stock tool shall open a vertically scrollable detail view with label-left and value-right rows.
- `UX-STOCK-005` User interface requirement: Catalog-derived field overrides shall show the original catalog value in orange together with a revert affordance.
- `UX-STOCK-006` User interface requirement: The catalog overlay shall support both detailed table import and single-item detail import flows.
- `UX-STOCK-007` User interface requirement: Enter shall be the only commit action for editable stock fields, Escape shall revert, and loss of focus shall not silently commit pending edits.

## 27. Override UX

- `UX-OVR-001` User interface requirement: The UI shall clearly distinguish inherited profile defaults from live project overrides.
- `UX-OVR-002` User interface requirement: Overridden values shall be displayed in orange with a revert affordance.
- `UX-OVR-003` User interface requirement: Reverting an override shall restore the profile default for that field immediately.
- `UX-OVR-004` User interface requirement: CNC and fixture profile references shall not present override controls at the live project level.
- `UX-OVR-005` User interface requirement: When the user intends to change the underlying saved default rather than the live override, the UI should offer an explicit edit-profile path.

See also:
- `FR-PROJ-009` for non-overridable profile types.
- `TR-UNIT-006` for edit-mode measurement behavior.

## 28. Diagnostics and Error UX

- `UX-DIAG-001` User interface requirement: Errors and warnings shall be summarized in a persistent cross-screen banner.
- `UX-DIAG-002` User interface requirement: Severity shall be color-coded with at least distinct error and warning states.
- `UX-DIAG-003` User interface requirement: Clicking or opening the summary shall expose detailed diagnostics.
- `UX-DIAG-004` User interface requirement: Errors and warnings shall clear automatically when the underlying condition is resolved.
- `UX-DIAG-005` User interface requirement: The application shall surface brief action summaries in a status or log area.
- `UX-DIAG-006` User interface requirement: Initial readiness messaging may include expected empty-state errors such as missing stock tools.

## 29. First-Run UX

- `UX-FIRSTRUN-001` User interface requirement: When a board is present, the first meaningful destination shall still be the project workspace.
- `UX-FIRSTRUN-002` User interface requirement: Missing required profiles and stock shall be presented as a readiness checklist.
- `UX-FIRSTRUN-003` User interface requirement: The first project flow shall support iterative movement between project and profile editors while preserving board context.
- `UX-FIRSTRUN-004` User interface requirement: When no board is present, the product may start in readiness or profile mode.

## 30. Responsive Behavior

- `UX-RESP-001` User interface requirement: The UI shall be desktop-first.
- `UX-RESP-002` User interface requirement: On narrower widths, the right settings panel shall collapse into tabs, drawers, or an equivalent compact control model.
- `UX-RESP-003` User interface requirement: Responsive behavior shall preserve viewport priority.
- `UX-RESP-004` User interface requirement: Error banner access and primary actions shall remain quick to reach on narrower widths.
- `UX-RESP-005` User interface requirement: UI delivery outputs should include wireframes, reusable component states, interaction flows, empty states, and responsive definitions derived from this specification.

---

# Part III - Technical Requirements

## 31. Measurement and Unit Handling

- `TR-UNIT-001` Technical requirement: A global units service shall handle parsing, conversion, formatting, and display across screens.
- `TR-UNIT-002` Technical requirement: Length values shall persist their original explicit unit expression, including `mm`, `in`, `mil`, and inch fractions.
- `TR-UNIT-003` Technical requirement: Feed-rate values shall persist explicit units and shall always include a feed-rate unit.
- `TR-UNIT-004` Technical requirement: Display precision shall default to `0.001` for `mm`, `0.00001` for `in`, and `0.1` for `mil`.
- `TR-UNIT-005` Technical requirement: Display-unit mapping shall be `mm -> mm/min`, `in -> in/min`, and `mil -> in/min`.
- `TR-UNIT-006` Technical requirement: In non-editing context, measurement values shall show preferred units first and append the original value in brackets when different.
- `TR-UNIT-007` Technical requirement: On entering edit mode, matching unit suffixes shall be stripped to raw values; angle and RPM suffixes shall always be removed.
- `TR-UNIT-008` Technical requirement: When a user enters a measurement without an explicit suffix, the current preferred unit shall be assumed.
- `TR-UNIT-009` Technical requirement: Enter shall validate and commit a measurement edit, while Escape shall cancel and restore the previous valid value.
- `TR-UNIT-010` Technical requirement: Validation failure shall keep focus in the same field and surface an inline error.

## 32. Profile Identity and UUID Rules

- `TR-PROFILE-001` Technical requirement: All persistent profiles shall have globally unique immutable identifiers.
- `TR-PROFILE-002` Technical requirement: Profile identifiers shall be 256-bit UUIDs.
- `TR-PROFILE-003` Technical requirement: The UUID shall be the canonical identity of a profile for its lifetime.
- `TR-PROFILE-004` Technical requirement: Profile names shall be user-facing labels only and shall not drive reference resolution.
- `TR-PROFILE-005` Technical requirement: Duplicating a profile shall generate a new UUID.
- `TR-PROFILE-006` Technical requirement: Importing a profile whose UUID already exists shall require conflict resolution.

See also:
- `FR-SAVE-005` for project load resolution.
- `AR-DB-001` for profile database lifecycle.

## 33. Profile Reference Modes

- `TR-REF-001` Technical requirement: Profile references shall support the modes `Fixed`, `List`, `Any`, and `New` unless a higher-level profile type constrains the allowed subset.
- `TR-REF-002` Technical requirement: `Fixed` shall assign exactly one profile and disallow user substitution.
- `TR-REF-003` Technical requirement: `List` shall constrain selection to an allowed set with one mandatory default.
- `TR-REF-004` Technical requirement: `Any` shall allow any compatible profile with one mandatory default.
- `TR-REF-005` Technical requirement: `New` shall create a new profile instance for the current project flow.
- `TR-REF-006` Technical requirement: For `List` and `Any`, the default shall belong to the allowed set unless the default is explicitly `New`.

## 34. Profile Schema and Persistence

- `TR-SCHEMA-001` Technical requirement: All persisted configuration files shall be stored as YAML.
- `TR-SCHEMA-002` Technical requirement: Each persisted configuration type shall be validated by its corresponding schema file.
- `TR-SCHEMA-003` Technical requirement: Built-in and external configuration parsing failures shall be handled differently according to severity and source.
- `TR-SCHEMA-004` Technical requirement: Catalog, CNC profile, fixture profile, process profile, stock, toolset, and global settings schemas shall be maintained as first-class compatibility contracts.
- `TR-SCHEMA-005` Technical requirement: If an external configuration file fails validation at load time, it shall be rejected and handled according to configuration error policy.

## 35. Project Packaging

- `TR-PKG-001` Technical requirement: A saved project shall be a self-contained container.
- `TR-PKG-002` Technical requirement: Embedded project contents shall be sufficient to regenerate without a live KiCad connection.
- `TR-PKG-003` Technical requirement: Generated outputs may be stored with the project, but reproducibility shall be based on embedded source data and embedded profiles rather than trusting stale outputs.

## 36. Embedded Profile Requirements

- `TR-EMBED-001` Technical requirement: Saving a project shall embed the exact profile definitions in use at save time.
- `TR-EMBED-002` Technical requirement: Embedded profiles shall retain their UUIDs and version metadata.
- `TR-EMBED-003` Technical requirement: Embedded profiles shall preserve enough information to recreate temporary profiles when the database copy is missing or unsuitable.

## 37. Temporary Profile Technical Semantics

- `TR-TEMP-001` Technical requirement: When a referenced embedded profile cannot be resolved safely to a persistent profile, the system shall create a temporary profile from the embedded copy.
- `TR-TEMP-002` Technical requirement: Temporary profiles shall behave identically to persistent profiles for editing and generation.
- `TR-TEMP-003` Technical requirement: If a temporary profile UUID collides with an existing persistent UUID during reconstruction, a new UUID shall be assigned and project references updated consistently.
- `TR-TEMP-004` Technical requirement: Temporary profiles shall be marked distinctly from persistent profiles in runtime state.
- `TR-TEMP-005` Technical requirement: Promoting a temporary profile shall create a persistent profile that preserves the temporary profile content and intended identity when safe.

## 38. RHAI Expression Model

- `TR-RHAI-001` Technical requirement: The application shall include a RHAI expression parser and evaluator used by CNC snippets and expression-backed profile fields.
- `TR-RHAI-002` Technical requirement: Strings inside `{}` in CNC primitive templates shall be evaluated as RHAI expressions.
- `TR-RHAI-003` Technical requirement: Expression evaluation shall run against active project effective values and relevant contextual properties.
- `TR-RHAI-004` Technical requirement: Parse and evaluation failures shall include source location and expression identity in diagnostics.
- `TR-RHAI-005` Technical requirement: Expression editing surfaces shall provide parse feedback before apply or save.
- `TR-RHAI-006` Technical requirement: Custom attributes shall be added automatically to the expression scope where applicable.

## 39. CNC Primitive Template Requirements

- `TR-PRIM-001` Technical requirement: CNC primitive templates shall be stored as schema-validated CNC configuration attributes.
- `TR-PRIM-002` Technical requirement: Required primitive attributes shall include `primitives.initialise`, `primitives.move_slow`, `primitives.start_spindle`, `primitives.stop_spindle`, `primitives.drill`, `primitives.peck_drill`, `primitives.cut_arc`, `primitives.cut_bezier`, `primitives.change_tool`, and `primitives.conclude`.
- `TR-PRIM-003` Technical requirement: Missing required primitive attributes shall cause schema validation failure for the CNC profile.
- `TR-PRIM-004` Technical requirement: Optional primitive attributes may include pause and banner insertion points.
- `TR-PRIM-005` Technical requirement: `cut_bezier` fallback behavior shall be deterministic when a native machine command is unavailable.

## 40. Tool Selection Algorithm

- `TR-TOOL-001` Technical requirement: Tool selection shall build a normalized demand set from all enabled drillable holes and routing-required features.
- `TR-TOOL-002` Technical requirement: Candidate drill tools shall be filtered using configured oversize and undersize allowances.
- `TR-TOOL-003` Technical requirement: Router candidates for hole fallback shall only be considered when routing fallback is enabled and geometry permits it.
- `TR-TOOL-004` Technical requirement: Required routing tools shall always be preserved in the mandatory routing set during selection and rack-shrinking decisions.
- `TR-TOOL-005` Technical requirement: If any hole has no feasible candidate tool set, generation shall fail with actionable diagnostics.
- `TR-TOOL-006` Technical requirement: Initial per-hole assignment shall prefer drilling over routing when configured, then fit quality, then tool preference, then reuse, then deterministic tie-break rules.
- `TR-TOOL-007` Technical requirement: Pilot-hole selection shall choose the largest valid drill strictly larger than the chosen router when pilot drilling is enabled.
- `TR-TOOL-008` Technical requirement: In ATC mode, rack shrinking shall iteratively remove the lowest-regret non-mandatory tool while preserving feasibility when possible.
- `TR-TOOL-009` Technical requirement: If no feasible rack shrink satisfies capacity, generation shall return the minimum feasible slot count and uncovered holes.
- `TR-TOOL-010` Technical requirement: When rack shrinking disables pilot-hole tools but routed coverage remains feasible, the system shall emit a warning rather than an error.

See also:
- `FR-GEN-004` for overall generation ordering.
- `TR-DET-002` for explainability data.

## 41. Routing Algorithm Requirements

- `TR-ROUTE-001` Technical requirement: Routing controls shall support tab count, tab width, optional mouse-bite holes, routing tool selection, and V-groove depth when applicable.
- `TR-ROUTE-002` Technical requirement: If tab count is zero, the system shall expose V-groove options instead of tab placement controls.
- `TR-ROUTE-003` Technical requirement: Computed center-to-center tab or hole spacing shall be available in preferred units.
- `TR-ROUTE-004` Technical requirement: Board geometry stitching shall preserve native geometry sufficiently to emit the closest valid machine primitive later.

## 42. Validation Rules

- `TR-VAL-001` Technical requirement: Validation shall distinguish hard generation failures from warnings and informational diagnostics.
- `TR-VAL-002` Technical requirement: For toolset profiles, exceeding physical rack size shall produce a warning rather than a hard failure unless policy forbids execution.
- `TR-VAL-003` Technical requirement: Under `Single Rack Only`, required-tool overflow shall be a hard error.
- `TR-VAL-004` Technical requirement: For profile reference sets using `List` or `Any`, the default profile shall exist in the allowed set unless explicitly set to `New`.
- `TR-VAL-005` Technical requirement: Stock and project edit validation shall be immediate on commit and shall preserve the prior valid value on cancellation.

## 43. Determinism and Explainability

- `TR-DET-001` Technical requirement: Identical effective inputs shall produce identical selected tool sets, primitive sequences, and final GCode.
- `TR-DET-002` Technical requirement: Per-hole assignment records shall capture selected tool ID, strategy, fit error, and whether assignment changed due to rack shrinking.
- `TR-DET-003` Technical requirement: Rack shrink steps shall capture removal reasons and incremental regret values.
- `TR-DET-004` Technical requirement: Numeric diameter and fit comparisons shall be evaluated at `1 um` precision.

---

# Part IV - Architectural Requirements

## 44. Application Runtime Model

- `AR-RUNTIME-001` Architecture requirement: The application shall be implemented in Rust.
- `AR-RUNTIME-002` Architecture requirement: The primary runtime context shall act as the orchestration root that owns or coordinates configuration, catalog, PCB state, project state, profile selections, and rendering adapters.
- `AR-RUNTIME-003` Architecture requirement: The UI layer shall subscribe to application context state and shall not directly own business logic.
- `AR-RUNTIME-004` Architecture requirement: The application shall remain portable across supported desktop operating systems.

## 45. Active Project Lifecycle

- `AR-LIFE-001` Architecture requirement: Project creation shall begin when a board is selected or opened.
- `AR-LIFE-002` Architecture requirement: Exactly one active project shall exist in the application context at a time.
- `AR-LIFE-003` Architecture requirement: Relevant project mutations shall increment generation or render state in a way that invalidates stale work.
- `AR-LIFE-004` Architecture requirement: Closing a project shall clear its temporary runtime-only profile state unless profiles were explicitly promoted.

## 46. Profile Database Lifecycle

- `AR-DB-001` Architecture requirement: Persistent profiles shall be loaded from the profile database into runtime state during initialization.
- `AR-DB-002` Architecture requirement: Built-in configuration failures shall be treated as fatal after surfacing diagnostics.
- `AR-DB-003` Architecture requirement: External configuration failures shall quarantine the invalid file rather than mutating valid persisted data silently.
- `AR-DB-004` Architecture requirement: The profile database lifecycle shall preserve immutable UUID identity and explicit conflict handling.

## 47. Project Load and Profile Resolution Architecture

- `AR-LOAD-001` Architecture requirement: Project load shall resolve profile references using UUID lookup before falling back to embedded copies.
- `AR-LOAD-002` Architecture requirement: Version mismatches between embedded and database profiles shall force an explicit resolution path rather than silent replacement.
- `AR-LOAD-003` Architecture requirement: Resolved active project state shall expose effective values for expression evaluation and generation.

## 48. Temporary Profile Lifecycle

- `AR-TEMP-001` Architecture requirement: Temporary profiles shall be created lazily during project load only when required by missing or unsuitable persistent profiles.
- `AR-TEMP-002` Architecture requirement: Temporary profiles shall be registered in active runtime state for the life of the project.
- `AR-TEMP-003` Architecture requirement: Temporary profiles shall never be persisted automatically to the profile database.
- `AR-TEMP-004` Architecture requirement: Promotion of a temporary profile shall be an explicit state transition initiated by the user.

## 49. Generation Pipeline Architecture

- `AR-PIPE-001` Architecture requirement: GCode generation shall run as a background activity with explicit generation identity.
- `AR-PIPE-002` Architecture requirement: The generation pipeline shall use monotonic generation IDs so stale compute results can be discarded safely.
- `AR-PIPE-003` Architecture requirement: Non-interruptible subcomputations may complete, but their results shall only commit if their generation ID is still current.
- `AR-PIPE-004` Architecture requirement: Generation completion and generation restart events shall update UI state atomically.
- `AR-PIPE-005` Architecture requirement: Program generation shall remain decoupled from machine dialects through primitive emission followed by CNC-template rendering.

## 50. KiCad Connection Architecture

- `AR-KICAD-001` Architecture requirement: KiCad integration shall use a connection layer that can detect explicit environment-based connections and scan compatible IPC endpoints when needed.
- `AR-KICAD-002` Architecture requirement: Board acquisition shall produce both raw board data and stitched or derived geometry views needed for generation.
- `AR-KICAD-003` Architecture requirement: The board object shall remain valid until a new board is loaded.

## 51. Dependency and Cascade Deletion Architecture

- `AR-CASCADE-001` Architecture requirement: Deletion workflows for dependent profiles shall compute the full set of directly and transitively impacted assets before confirmation.
- `AR-CASCADE-002` Architecture requirement: Cascading deletion shall be allowed for CNC, fixture, and process profiles.
- `AR-CASCADE-003` Architecture requirement: The deletion system shall treat live or open projects instantiated from deleted process profiles as impacted assets.

## 52. State Ownership and Mutation Rules

- `AR-STATE-001` Architecture requirement: Business logic shall live in owned subsystems rather than the UI layer.
- `AR-STATE-002` Architecture requirement: The application context shall expose read-only query methods for UI consumption.
- `AR-STATE-003` Architecture requirement: The expression evaluation entry point shall resolve properties and functions through active project effective values.
- `AR-STATE-004` Architecture requirement: Runtime evaluation shall never silently mutate persisted profile definitions.

## 53. Offline/Reproducibility Architecture

- `AR-OFFLINE-001` Architecture requirement: Saved projects shall remain reproducible without a live KiCad connection.
- `AR-OFFLINE-002` Architecture requirement: Embedded profile and PCB data shall be sufficient to regenerate deterministically offline.
- `AR-OFFLINE-003` Architecture requirement: Disconnect from KiCad after successful acquisition shall not invalidate already-cached project work.

---

# Appendices

## Appendix A - Glossary

- `Active project`: The one runtime project currently owned by the application context.
- `ATC`: Automatic tool changer capability of the CNC.
- `Built-in profile`: A bundled read-only profile shipped with the application.
- `Effective value`: The runtime value after default resolution and override application.
- `Generation cycle`: One execution of the regeneration pipeline for a specific generation ID.
- `Mandatory routing set`: The set of tools required by non-optional routing operations and therefore protected during rack shrinking.
- `Temporary profile`: A runtime profile reconstructed from an embedded project copy.

## Appendix B - Requirement ID Index

- `FR-CTRL` -> Document control requirements in Section 0.
- `FR-CTX`, `FR-PRIN`, `FR-DOM` -> Product context, principles, and core domain model in Sections 1-4.
- `FR-PROJ`, `FR-PROC`, `FR-CNC`, `FR-FIX`, `FR-TOOLSET`, `FR-STOCK`, `FR-CAT`, `FR-PCB`, `FR-GEN`, `FR-SAVE` -> Functional requirement groups in Sections 5-14.
- `UX-IA`, `UX-LAYOUT`, `UX-NAV`, `UX-TOPBAR`, `UX-PROJ`, `UX-BOARD`, `UX-PROGRAM`, `UX-PROC`, `UX-CNC`, `UX-FIX`, `UX-TOOLSET`, `UX-STOCK`, `UX-OVR`, `UX-DIAG`, `UX-FIRSTRUN`, `UX-RESP` -> UX requirement groups in Sections 15-30.
- `TR-UNIT`, `TR-PROFILE`, `TR-REF`, `TR-SCHEMA`, `TR-PKG`, `TR-EMBED`, `TR-TEMP`, `TR-RHAI`, `TR-PRIM`, `TR-TOOL`, `TR-ROUTE`, `TR-VAL`, `TR-DET` -> Technical requirement groups in Sections 31-43.
- `AR-RUNTIME`, `AR-LIFE`, `AR-DB`, `AR-LOAD`, `AR-TEMP`, `AR-PIPE`, `AR-KICAD`, `AR-CASCADE`, `AR-STATE`, `AR-OFFLINE` -> Architectural requirement groups in Sections 44-53.

## Appendix C - Profile Reference Examples

`Fixed`

```text
Process Profile -> CNC Profile: Masso G3 Production
User selection disabled.
```

`List`

```text
Process Profile -> Fixture Profile: [2-pin vise, vacuum bed, edge clamp]
Default: vacuum bed
User may choose only from the listed fixtures.
```

`Any`

```text
Process Profile -> Toolset Profile: any compatible toolset
Default: Auto
User may choose any compatible toolset in the database.
```

`New`

```text
Project flow -> Fixture Profile: New
User creates a project-specific fixture definition during setup.
```

## Appendix D - Project Save/Load Scenarios

1. Saved project reopened with unchanged referenced profiles.
   Result: UUID resolution binds to database profiles and regeneration proceeds normally.

2. Saved project reopened with a changed CNC profile version.
   Result: User chooses between current database version, updating from embedded copy, or using a temporary profile.

3. Saved project reopened after referenced fixture profile deletion.
   Result: Embedded fixture copy becomes a temporary profile and is flagged visibly.

4. Saved project reopened offline with no KiCad connection.
   Result: Embedded PCB snapshot and embedded profiles allow deterministic regeneration.

## Appendix E - Error and Warning Catalog

- Error: no process profile selected for active project.
- Error: required primitive template missing from CNC profile.
- Error: board outline invalid or not closed.
- Error: no feasible tool candidate exists for one or more required holes.
- Error: `Single Rack Only` policy exceeded by required tooling.
- Error: KiCad acquisition failed before minimum project data was cached.
- Warning: temporary profile active from embedded project copy.
- Warning: pilot-hole optimization disabled by rack shrinking.
- Warning: toolset profile exceeds physical rack size but policy still permits execution.
- Warning: database profile version differs from embedded project profile.

## Appendix F - Migration Notes from v1

- This revision replaces the previous mixed product and UX narrative with a four-part canonical specification.
- Previous unnumbered or section-local rules are now assigned stable requirement identifiers.
- Product, UX, technical, and architectural rules are separated explicitly to support downstream design and implementation documents.
- Cross references should use requirement IDs rather than former section numbers where possible.
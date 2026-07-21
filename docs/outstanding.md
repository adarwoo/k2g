# Outstanding Work — resume point

Status: **living roadmap** (2026-07-22). What is built, what is left, and the
order to tackle it. Companion to the design docs: `gcode-template-language.md`
(GTL surface), `gcode-engine.md` (the Coder), `gcode-generation.md` (the
generation pipeline/lifecycle), and `architecture.md` (module contracts).

---

## 0. Where we are — the foundation that exists

Built and compile/test-verified (not yet runtime-verified — see §5.2):

- **`gtl` crate** (`crates/gtl`) — the generic template engine: transpile → compile
  → run → capture emitted strings; `Gtl::{new, engine_mut, writer, compile, run}`,
  `Template`, `GtlError`. 17 tests. Domain-agnostic (host injects the dialect).
- **Generation pipeline** (`src/runtime/generation.rs`, `gcode-generation.md`) —
  off-UI-thread worker + queue, single-flight cancellation (monotonic id +
  `Arc<AtomicBool>`), publish into the ctx, `tokio::sync::watch` → Dioxus
  `use_future` wake bridge, `GenerationState` = `Idle/Running/Failed`. The compute
  step `run_generation` is a **placeholder** (emits a header comment).
- **Stitch cleanup** — one stitch per board acquisition; the full `pcb::StitchResult`
  (contours + errors) is cached on the ctx; stitcher debug moved behind `log`.
- **Change-detection fix** — `with_ctx_mut` snapshots state *before* the mutation
  (`sync_after_mutation`), so board re-stitch and the regeneration trigger actually
  fire (they were previously inert).
- **Reload PCB** action + PCB name display (top bar).

Everything below is outstanding.

---

## 1. The generation algorithm

The headline item. Today only the `run_generation` placeholder and
`pcb::{stitch_edge_shapes, routing_offset}` exist. Four pieces, per
`architecture.md`:

1. **OperationPlanner** — `Board + JobInstance → PrimitivePlan` (pure).
   Drilling plan; contouring with tabs, mouse-bite holes, V-groove; TSP travel
   ordering; auto-rotation + board-fit check (writes the resolved angle back).
   The largest chunk. Start with **drilling** — simplest and snapshot-testable.
2. **Tools / Assigner** — `hole demand + stock + allowances + rack → ToolAssignment`
   (pure). The normative candidate-build / scoring / rack-shrink / deterministic
   tie-break algorithm (1 µm precision). Feeds both the plan and the Rack view.
3. **The Coder** — the app-side layer on `gtl`: register the **GCode dialect**
   (unit-typed `fmt` over `units::machine::*`; `metric()`/`imperial()` that emit
   `G21`/`G20` via `Gtl::writer` and set a mode flag the `fmt` reads), build the
   three-layer scope + **namespaced job context** (`gcode-engine.md` §2), and drive
   the `PrimitivePlan`. Lives in `src/gcode/` (supersedes the WIP `template.rs`).
4. **The real `JobInstance` snapshot** — today `GenerationInput` is lean (names +
   op labels). The planner/coder need the fully-resolved job: effective profile
   values, fixture backing board, toolset sizes, machine limits, and the CNC
   primitive templates. Building that immutable snapshot replaces the placeholder
   input.

Then: **wire it into `run_generation`** (replace the placeholder), emitting typed
`CoderError` diagnostics through the existing Failed path.

---

## 2. Output & editing UI

- **Code view** (`src/ui/screens/job/code.rs`) — already an editable textarea +
  stat strip. Needs: the GCode-editing policy (§4) wired in, and **Export to
  `.nc`** (see §3, item 1).
- **Rack view** (`src/ui/screens/job/rack.rs`) — exists but is **toolset-driven**;
  rewire it to display the generation's `ToolAssignment` (tool → slot) plus
  load/unload steps.
- **Machinist instructions** — new: a human-readable setup/run sheet (load T1, flip
  board, snap tabs by hand, …), derived from the plan + assignment.

---

## 3. Cross-cutting / supporting

1. **GCode export to file** — no action writes the program to disk today
   (`save_filename` exists; nothing uses it for the program). Add an Export/Save
   `.nc` action (the app already uses `rfd::FileDialog` for YAML profile export).
2. **GTL catalogs** (`gcode-engine.md` §9, phase 2) — variable + built-in
   registries. Prerequisite for *safe* namespaced job-context (flags unknown
   `ns.field` at load) and for the editor.
3. **Expression editor pop-up** (`gcode-engine.md` §10, phase 3) — GTL-aware
   highlight, variable/function panels, live verify, dry-run. This is the rich way
   to "edit the machining instructions" (the CNC primitive templates).
4. **Tests** — snapshot tests per CNC profile against a fixed `PrimitivePlan`;
   unit tests for the planner and assigner (`architecture.md` testing notes).
5. *(Optional)* **Board-view toolpath overlay** — draw the planned operations on
   the board in `job/board.rs`.

---

## 4. Decision — GCode is editable, with a phased regen policy

**Settled (2026-07-22):** the generated GCode **can be manually edited**, and the
reconciliation with regeneration is staged:

- **Phase 1 (now):** a regeneration **overwrites** manual edits. This is the
  current behavior — the worker publishes a new program and the wake bridge
  re-syncs the Code view. `gcode_modified` already tracks the dirty flag; surface
  it (e.g. a "manually edited — will be replaced on regenerate" hint) so the
  overwrite is not a surprise.
- **Phase 2 (later):** on a regeneration while the program is manually edited, the
  app **merges or prompts** — either a 3-way auto-merge or a "keep mine / take
  regenerated / merge" prompt.

  **Data requirement to enable this — record now if convenient:** a 3-way merge
  needs the **last *generated* program retained as the merge base** (so we can diff
  user-edits-vs-base against new-generation-vs-base). So the publish path should
  keep two strings: `gcode` (what the user sees/edits) and a separate
  `generated_base` (the last unedited generation). Cheap to add early; painful to
  retrofit once editing is live.

---

## 5. Notes carried forward

### 5.1 Open decisions still parked

- **Coder lifetime** (`gcode-engine.md` §12.5) — build per run vs. pool compiled
  ASTs. Doesn't block; decide when the Coder exists.
- **Completion toasts** — currently a toast per successful run; on a live tool that
  may be chatty. Option: failures-only toasts, let the updated Code view + status
  bar confirm success. (Start toast already removed.)

### 5.2 Runtime-verification debt (do before building more on top)

Everything from the 2026-07-21/22 work is **compile + unit-test verified only**
(the `machine.exe` lock blocks running the app). A click-test pass is owed,
especially:

- the worker thread + `tokio::sync::watch` → `use_future` wake bridge (does the
  Code view refresh on its own after a run?);
- change-detection now firing on **every** edit (any unexpected regen storms?);
- the new top-bar UI (Reload glyph, PCB name, `Generating GCode…` status).

---

## 6. Suggested sequencing

1. **Runtime-verify** the current pipeline (§5.2) — cheap, de-risks everything.
2. **`JobInstance` snapshot** — the real `GenerationInput`.
3. **OperationPlanner: drilling** — smallest real plan; add a snapshot test.
4. **Tools/Assigner** — needed for real drilling (tool per hole) and the Rack view.
5. **The Coder** — register the dialect + scope; drive the drilling plan; replace
   the `run_generation` placeholder. First real end-to-end GCode.
6. **Export to `.nc`** + surface `gcode_modified` (§4 Phase 1).
7. **Contouring** incrementally — outline, tabs, mouse-bites, V-groove — each with
   a per-CNC-profile snapshot test.
8. **Rack view** from `ToolAssignment`; **machinist instructions**.
9. **Catalogs**, then the **expression editor**.
10. **Merge/prompt** on regen (§4 Phase 2) — once editing is established and the
    `generated_base` is retained.

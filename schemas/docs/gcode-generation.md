# GCode Generation — orchestration & lifecycle

Status: **design draft** — the loop *around* the engine. Where `gtl` +
`gcode-engine.md` (the Coder) and the OperationPlanner (`operation-planner.md` —
demand → ordered atomic ops) do the work, this document covers **when** generation
runs, **how** it runs (off the UI thread, cancellably), and **how** its results reach
the UI. It refines `architecture.md` §Generator and records the concrete clean-up this
pass is targeting.

---

## 1. The pipeline in one line

```
acquire board → stitch (once) → [readiness gate] → on change: enqueue request
             → worker runs Planner + Coder (cancellable, single-flight)
             → publish {GCode, Rack, operations, diagnostics} → UI + notify
```

Two invariants hold throughout:

- **One stitch per board acquisition.** The stitched model (contours *and*
  errors) is computed once, cached, and read by everyone. See §3 — today it is
  computed two-to-three times, which is the bug that prompted this pass.
- **Single-flight with a monotonic generation id.** At most one run is in flight;
  a newer request cancels the older; only the latest id's result is committed.

---

## 2. Preconditions — the readiness gate

Generation may start **only** when all three hold. These are the user's stated
conditions, mapped onto the existing `evaluate_generation_readiness`
(`runtime/orchestration.rs`):

1. **The PCB data was acquired correctly and fully, and is valid.** The board
   snapshot exists and its stitched outline is clean: **no open contours, no
   floating islands** (and no other stitching error). This holds *even if the
   selected machining does not use all of the data* — a board that cannot be
   stitched into closed, properly-nested contours is not a board we will cut.
2. **A job is set up with no errors.** A machining profile is selected and it, plus
   everything it references (CNC + fixture + toolset profiles, and the toolset's
   tools), is complete and usable (no `pending_required_fields`, `usable == true`).
3. **No ongoing unresolved errors.** No blocking runtime error is present
   (`app.errors` has no `is_error` entry).

`readiness.is_ready` is the AND of the three; `readiness.nogo_reasons` lists what
is missing, for display. The gate is evaluated on every state sync; a trigger that
fires while the gate is closed is **recorded but not started** (it logs why), and
the eventual pass that closes the gap re-triggers.

> The gate is a *guard on starting*, not a substitute for the trigger. A change
> still has to happen to request generation (§5); the gate only decides whether
> that request may run now.

---

## 3. Board acquisition and the single stitch

### 3.1 What is wrong today

`stitch_edge_shapes` is called at **three** sites, two of which run at startup on
identical data:

| Site | Purpose today | Problem |
|------|---------------|---------|
| `main.rs` (startup) | stitch the first snapshot to `println!` a diagnostic | result **discarded**; pure duplicate work |
| `orchestration.rs` `from_launch` | stitch to fill `StitchedBoardData` | re-stitches the same board |
| `orchestration.rs` `sync_from_app_state` (on `board_changed`) | re-stitch when the board changes | correct trigger, but see below |

On top of the duplication, `StitchedBoardData` stores only `{ error_count, errors }`
— it **throws away the `contours`**, which the OperationPlanner needs, forcing a
*third* stitch at generation time. And `crates/pcb/src/stitching/mod.rs` prints a
wall of `[stitch] …` lines on every call, which is what made the double-run
visible.

### 3.2 Target

- **Stitching is part of acquisition.** Acquiring a board (from KiCad, at startup
  or on reload) produces a `StitchedBoard { contours, errors }` **once**, and that
  full model — contours included — is what gets cached (on the board snapshot / in
  the ctx). Nothing downstream re-stitches.
- **`main.rs` stops stitching.** The startup diagnostic is derived from the cached
  result (or emitted by the acquisition step), not from a throwaway call.
- **`sync_from_app_state` re-stitch stays as the *single* re-stitch point** — but
  it only fires on a genuine board *replacement* (a reload, §4), and since
  acquisition already stitched, this becomes "a new, already-stitched board
  arrived" rather than "stitch again."
- **Quiet the stitcher.** The `println!`/`eprintln!` debug in the `pcb` crate moves
  behind `log`/`tracing` at `debug`/`trace`, so normal runs are silent and a single
  stitch is not mistaken for many.

Net: **one stitch per acquisition**, contours retained, no console spam.

---

## 4. PCB reload (new)

Add an explicit **Reload PCB** action (user-invoked, and the hook for future
auto-refresh):

1. Re-acquire the snapshot from KiCad.
2. Stitch once (§3) into a fresh `StitchedBoard`.
3. Replace the board in state.

The replacement is a board change, so it flows through `sync_from_app_state`'s
`board_changed` path and raises the `PcbLoadedOrReloaded` trigger (§5). Reload is
therefore just "acquire a new board"; the rest of the pipeline is unchanged.

---

## 5. Regeneration triggers — change → request

A **regenerate request** is enqueued whenever data that the output depends on
changes *and is sent for persistence*. The dependency set (already computed by
`detect_generation_trigger` + `collect_mutation_changes` + the reference
fingerprints):

- **PCB (re)loaded** — a new board snapshot (§4).
- **Job configuration** — any field of the active `JobConfig`.
- **Selected machining profile** — the selection itself changes.
- **A referenced dependency** — the selected profile or anything it references
  (CNC / fixture / toolset profiles, or the toolset's tools) changes value.
- **Stock** — a tool the job references changes.

Each request captures, at enqueue time:

- a **monotonic generation id** (the single-flight key),
- the **trigger cause** (for status/telemetry), and
- an **immutable input snapshot** — the fully-resolved `JobInstance` (effective
  values: profile defaults + runtime overrides) plus the cached `StitchedBoard`.
  Because the input is snapshotted, edits made *during* a run cannot corrupt it;
  they simply enqueue the next request.

Rapid edits **coalesce** through single-flight, not a debounce: every change
enqueues *immediately*, and because a new request cancels the in-flight run and
supersedes any earlier pending one, a burst collapses to at most one in-flight run
plus one pending. No debounce delay — k2g is a live tool, so a change starts
regenerating at once (§11.4).

---

## 6. Execution — worker, queue, cancellation

Generation runs **off the UI thread**, fed by a queue/channel, as a **single-flight**
loop:

- **One worker consumes requests.** When a new request arrives while one is in
  flight, the in-flight run is **cancelled** and the new one starts. Intermediate
  requests that were superseded before starting are dropped (only the newest
  pending request matters).
- **Cooperative cancellation at checkpoints.** The run is not killed mid-write; it
  checks a cancel signal at natural boundaries — between plan steps, and between
  primitive `expand` calls in the Coder loop (a board is thousands of primitives,
  so checkpoints are dense and cancellation is near-immediate). This is the
  "clean coroutine cancellation" the design calls for: cancellation is a checked
  flag at await/step points, never a torn write.
- **Generation-id tagging.** Every result carries its request's id. When a run
  finishes, its result is committed **only if its id is still the latest**;
  otherwise it is discarded (matches architecture: "cancel-on-new-trigger discards
  stale output and never commits it").
- **Determinism.** Same `JobInstance` → same GCode (the Planner and Coder are pure
  functions), which is what makes snapshot tests meaningful.

The **mechanism is a dedicated OS worker thread** with an `Arc<AtomicBool>` cancel
flag, checked at the checkpoints above (§11.1). Generation is CPU-bound, so an
async runtime buys nothing; the properties that matter — checkpointed
cancellation, single-flight, id-tagging — are all the thread needs. Because the
worker is always waiting on the queue, enqueue→run is instantaneous (there is no
`Queued` state, §8).

---

## 7. Result publication

On a run reaching a terminal state (and only if its id is still current):

- **Succeeded** — atomically publish into state, as one update: the **GCode text**,
  the **Rack** assignment (`rack_slots`), the **operations summary**, and any
  non-blocking diagnostics. The UI's Job views (Code / Rack) read the new values.
  State settles back toward `Idle`/ready.
- **Failed** — surface the typed diagnostics and **clear everything**: the program
  and all derived outputs (GCode, Rack, operations). k2g is a live tool, so an
  empty result is correct and a stale one is a lie (architecture:
  "`GenerationFailed` clears the program and surfaces diagnostics").
- **Cancelled** — commit nothing; leave the prior results untouched. The
  superseding run owns the next publish.

Publication is a single `with_ctx_mut` commit so the UI never observes a
half-updated program.

---

## 8. Status & notification

`GenerationState` (collapsed to just `Idle` during the recent dead-code sweep) is
**re-expanded** to a minimal three — `Idle`, `Running`, `Failed` — driving both
the persistent status and the transient notifications:

| State | Meaning | UI |
|-------|---------|----|
| `Idle` | nothing running; last program (if any) is current | "Ready" / diagnostics count |
| `Running` | the worker is generating | spinner + **gray-out** of the program |
| `Failed` | last run errored | error pill + diagnostics; program cleared |

There is **no `Queued` state**: the worker is always waiting, so enqueue→run is
instantaneous and a request goes straight to `Running`. Cancellation is not a
resting state either — a cancelled run is immediately followed by the superseding
`Running`, so it never surfaces.

Two surfaces, both already present in the shell:

- **Persistent status** — the `StatusBar` pill and `DiagnosticsBanner`
  (`ui/screens/shell.rs`) already switch on `generation_state`; they gain the new
  states. A **gray-out** signal is emitted on `Running` and cleared on terminal,
  so the displayed program visibly de-emphasises while regenerating.
- **Notifications** — completion and failure post an `AppEvent` via `log_event`
  (already the mechanism for the event toasts), e.g. "Generated 3 operations,
  1,240 lines" or "Generation failed: <reason>".

Optional: per-operation progress (drilling / contouring …) as the plan executes,
for large boards.

---

## 9. Threading & Dioxus boundary

- The `gtl` engine is single-threaded per instance (it holds an `Rc` output
  buffer), so the Coder is **built on the worker** from the input snapshot;
  immutable `Template` ASTs may be shared, but the simplest model builds per run.
- The worker communicates results back over a channel; a Dioxus `use_future` /
  coroutine on the UI side receives them and commits via `with_ctx_mut` (§7), so
  all state mutation stays on the UI thread and the worker stays pure compute.
- The global ctx (`GLOBAL_CTX`) remains the single source of truth; the worker
  never writes it directly.

---

## 10. Clean-up checklist — as built (2026-07-22)

All items implemented; `cargo check --workspace` warning-clean, 36 bin tests pass.
Not runtime-verified (the `machine.exe` lock blocks running the app); the worker
thread + Dioxus wake bridge in particular need a click-test.

1. ✅ **Killed the double stitch.** `main.rs` no longer stitches (it only collects
   the snapshot); the single stitch happens when the board is cached in the ctx.
2. ✅ **Retained contours.** `AppCtx.stitched_board_data` is now the full
   `pcb::StitchResult` (contours + errors), so the Planner will not re-stitch.
3. ✅ **Single re-stitch point.** `sync_after_mutation` re-stitches only on a real
   `board_changed`; the redundant second acquisition in `AppRoot`'s `use_effect`
   was removed (the startup board comes from the boot payload).
4. ✅ **Silenced the stitcher.** The `crates/pcb` stitching `println!`s are now
   `log::debug!`/`trace!`.
5. ✅ **Replaced the stub.** `report_generation_started` builds an immutable
   `GenerationInput`, sets `Running`, and enqueues to the worker
   (`runtime/generation.rs`); the worker runs, then publishes via the §7 path.
   The generation *compute* (`run_generation`) is itself still a **placeholder**
   emitting a header program — it slots out for the OperationPlanner + Coder.
6. ✅ **Re-expanded `GenerationState`** to `Idle / Running / Failed`, with the
   status pill, diagnostics banner, and status bar updated (§8).
7. ✅ **Added Reload PCB** — a top-bar action calling `runtime::acquire_board()`
   then setting the board (which re-stitches once and triggers regeneration).

### 10.1 Also fixed: the change-detection was inert

Not in the original spec, but discovered while wiring this: `with_ctx_mut` cloned
the app **after** running the mutation and passed that to `sync_from_app_state`, so
`previous == current` — `board_changed` was always false and `change_set` always
empty. **The regeneration trigger had never fired.** Fixed by snapshotting the app
*before* the mutation (`with_ctx_mut` → `sync_after_mutation(previous_app)`), so the
old→new diff — and therefore board re-stitching and the trigger — is real.

---

## 11. Decisions

**Settled (2026-07-21):**

1. **Cancellation mechanism — a dedicated OS worker thread** with an
   `Arc<AtomicBool>` cancel flag, checked at the §6 checkpoints. Generation is
   CPU-bound, so an async runtime buys nothing.
2. **`GenerationState` — minimal `Idle / Running / Failed`.** No `Queued`: the
   worker is always waiting, so enqueue→run is instantaneous and a request goes
   straight to `Running`; cancellation folds into the superseding `Running`.
3. **On failure — clear everything.** A failed run clears the program and all
   derived outputs (GCode, Rack, operations). k2g is a live tool: an empty result
   is correct, a stale one is a lie.
4. **No debounce.** Every change enqueues immediately; single-flight coalescing (a
   new request cancels the in-flight one) is the only batching.

**Still open:**

5. **Coder lifetime.** Build the Coder per run (simple, deterministic) vs. pool it
   across runs (reuse compiled ASTs; needs care with the single-threaded engine).

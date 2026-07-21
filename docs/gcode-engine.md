# GCode Engine (the Coder)

Status: **design draft** — the contract to build against, companion to
`gcode-template-language.md` (GTL, the surface syntax). This document covers the
*engine*: how a primitive template is turned into GCode, where its variables come
from, how isolation and purity are guaranteed, and how errors surface.

In the architecture (`architecture.md`) this is the **Coder**: a pure function
`PrimitivePlan + CNC RHAI profile → GCode text`. It is a dialect resolver — it
maps machine-independent primitives to a specific controller's GCode — and reads
no board or application data directly.

---

## 1. Role and boundaries

- **Input:** a `PrimitivePlan` (an ordered list of primitive invocations, each
  with its resolved arguments) plus the active CNC profile's `primitives`
  templates.
- **Output:** GCode text, or a typed `CoderError` (never a panic).
- **Purity:** given the same `PrimitivePlan` and profile, the output is
  identical. The Coder holds an *immutable snapshot* of effective values; it
  never touches AppData, the datastore, or the filesystem.
- **Who resolves values:** the generator / `JobProcessor` resolves *effective*
  values (profile default + runtime override) into the plan **before** the Coder
  runs. The Coder never resolves anything from app state — it only formats what
  it was handed. This matches the architecture rule: "effective project values
  are the source of truth for expression evaluation; profile files are never
  mutated by runtime evaluation."

### 1.1 Packaging — the `gtl` crate + the app-side Coder

**Built (2026-07-21) — `crates/gtl`.** The engine is split in two, and the split
is the important part:

- **`gtl` — the generic engine (the crate).** *Generic Template Language*: it
  transpiles backtick-emit templates to Rhai, compiles them, runs them against a
  scope, and captures the emitted **strings**. It depends only on `rhai` and
  `thiserror` — **not** on `units`, `pcb`, `datastore`, or any app type. It has
  never heard of GCode, `FixtureProfile`, or AppData. Its whole surface is
  `Gtl::{new, engine_mut, writer, compile, run}`, `Template`, and `GtlError`
  (Parse/Runtime/Thrown, §6). The only built-ins it registers are `emit(text)`
  (what a backtick line compiles to) and a default `fmt(value)` for plain scalars
  and strings.
- **The Coder — the app-side dialect (the architecture term).** The "Coder" of
  the architecture is the *host layer* that turns the generic engine into a GCode
  resolver. It lives in the k2g binary's `src/gcode/` and, on a `gtl::Gtl`:
  (a) registers the **GCode dialect** — unit-typed `fmt` overloads over
  `units::machine::{number_length, number_feed, number_speed}`, and the modal
  `metric()`/`imperial()` built-ins (each emits `G21`/`G20` via `Gtl::writer` and
  flips a mode flag the `fmt` overloads read); (b) manages the three-layer scope
  and the namespaced job context (§2); (c) builds the `PrimitivePlan`
  (OperationPlanner) and drives `expand` over it. The current
  `src/gcode/template.rs` WIP is superseded.

Why the split: the engine stays a pure, domain-free substrate — snapshot-testable
without a full application context (architecture: "testable independently"),
reusable by its own tests/benches and a future headless CLI without linking the
Dioxus app — while everything GCode-specific is host-injected. That is what makes
"Generic Template Language" honest rather than a relabel.

---

## 2. Scope model — three layers, one immutable snapshot

Every value a script can see is a named variable in its Rhai scope. Variables
come from three layers, resolved to an owned, immutable snapshot and flattened
into a single scope per primitive call:

| Layer         | Set                          | Lifetime            | Examples |
|---------------|------------------------------|---------------------|----------|
| **Program** (job context + run metadata) | once, at `Coder::new` | whole generation | `toolset.size`, `fixture.backing_board_thickness`, `machine.max_feed_rate`, `machine.spindle_rpm_min`, `cnc.z_safe`, `pcb_filename`, `timestamp`, `scaling_x`, `scaling_y` |
| **Operation** | per tool / operation         | one operation's steps | `tool_diameter`, `rpm`, `z_feed`, `xy_feed`, `z_bottom`, `z_retract`, `peck` |
| **Call**      | per `expand()`               | one primitive call  | `x`, `y`, `z`, `s`, `i`, `j`, `arc_cmd`, `slot`, `message`, `text` |

Precedence on a name clash is **call > operation > program**.

### 2.1 The job context — injected once, live for the whole run

The program layer is populated by the **job context**: the batch of resolved job
values the app wants templates to see — fields drawn from the machine, CNC,
fixture, toolset, and machining profiles, plus board/run metadata (filename,
timestamp, scaling). The generator resolves it **once**, to *effective* values
(profile default + any runtime override, per §1), and hands it to `Coder::new`.

From that point the job context is immutable and visible to **every** `expand()`
for the rest of the generation. The engine is built once and reused across the
board's thousands of primitive calls; the job context is re-presented —
identically — to each call's fresh scope (§3.1). "Set once, always in scope,
never mutable across calls" is precisely the program-layer guarantee, and the job
context is its dominant content. So a manual peck loop can read `{machining.peck}`
and a header can read `{fixture.backing_board_thickness}` with no per-call
plumbing: those live in the job context and do not change during a run.

Nothing is *reached* — everything is *pushed to it*. The Coder performs no lookups
of its own; it never touches AppData, the project, or the board.

### 2.2 Naming — namespaced context, flat hot values

Two conventions, split by role:

- **Job context is namespaced.** Each source profile becomes a top-level
  namespace in scope — `machine.*`, `cnc.*`, `fixture.*`, `toolset.*`,
  `machining.*` — with run-level values at the top level (`pcb_filename`,
  `timestamp`, `scaling_x/y`). A template reads `{fixture.backing_board_thickness}`:
  provenance is legible at the call site, and two profiles can both expose a
  `max_feed_rate` without colliding.
- **Hot per-operation / per-call values stay flat and terse.** The values on
  nearly every emitted line — `x`, `y`, `z`, `z_feed`, `tool_diameter` — are
  plain names. They dominate the 90% case (GTL principle #2); a prefix there is
  pure noise.

Mechanism: a namespace is a Rhai object map whose fields are the typed unit
values, so `machine.max_feed_rate` is ordinary Rhai property access and the
`{ … }` interpolation expression already accepts it (GTL §4). A flat alias is
still allowed where a namespaced value is used constantly (the app may inject
`z_safe` as an alias of `cnc.z_safe`).

The crate supplies the *mechanism* — a namespaced, typed scope. The *content* —
which profile field maps to which name — is chosen by the app when it builds the
job context (§7), so the engine never learns the shape of a `FixtureProfile` or
`ToolsetProfile`. The authoritative per-primitive name/type list is the
**variable catalog** (§9), the canonical contract deferred from GTL §12; the
names here are illustrative.

---

## 3. Isolation and purity

### 3.1 Script variables are per-primitive

Each `expand()` builds a **fresh Rhai `Scope`** from the (program + operation +
call) snapshot and evaluates the primitive in it. Any `let` the script declares
lives and dies inside that scope — there is no leakage from one primitive to the
next. A fresh scope per call also means a mutated injected variable in one call
never affects the next.

### 3.2 The only *mutable* cross-call state is program-scoped engine state

The job-context / program snapshot spans the whole run too, but as **immutable**
carry-over: it is re-seeded into every fresh scope unchanged and never written
back (§2.1). The two facts below are the only **mutable** engine state that
evolves as generation proceeds; both live on the engine, **not** in the script
scope:

- **Active unit mode** — set by `metric()` / `imperial()` (GTL §5). It mirrors
  the machine's modal `G21`/`G20` state, so it must survive from `initialise`
  through every later primitive.
- **Line-number counter** — the monotonic `N` word, advanced by the machine's
  `line_numbering_increment`.

Everything a *script* declares is call-local; only these engine facts carry over.

### 3.3 Scripts cannot mutate application data

Guaranteed structurally, not by convention:

- The Coder owns an **immutable snapshot** (an owned copy of effective values),
  never a handle back to AppData.
- No registered function writes application state. The built-in surface (§8) is
  pure: unit math, `min`/`max`/`clamp`/`abs`, formatting, `throw`.
- Injected variables are pushed **by value**. Mutating them (e.g. a countdown,
  `let n = holes; while n > 0 { n -= 1; … }`) changes only the scope copy.

Consequence: a primitive can freely compute and mutate locally, but the worst a
buggy template can do is produce wrong GCode or an error — never corrupt state.

---

## 4. Typed values

Injected values keep their unit type inside the scope (`Length`, `FeedRate`,
`RotationalSpeed`, `Angle` from the `units` crate), registered as Rhai custom
types. This is what makes script logic unit-correct without `.mm` noise:

```rhai
let z = z_retract;              // Length
while z > z_bottom {            // Length vs Length, compared in canonical units
    z = max(z - peck, z_bottom);// Length arithmetic stays Length
    `G1 Z{z} F{z_feedrate}      // formatted to the active unit at emit (GTL §4)
}
```

Minimum registered surface for phase 1:

- `Length ± Length → Length`, `Length * number → Length`,
  `FeedRate * number → FeedRate`
- comparisons `< > <= >= ==` on each unit type (canonical-unit compare)
- `max`, `min`, `abs`, `clamp` over unit types
- read accessors: `.mm .cm .inch .mil` (Length), `.mm_per_min .in_per_min`
  (FeedRate), `.rpm`, `.degrees .radians` — each returns a plain number, the
  escape hatch for forcing a specific unit (GTL §4)

The `units` types are `Copy` newtypes today, so these are small operator impls or
`register_fn` additions — no redesign of the crate.

---

## 5. Evaluation pipeline

Per generation:

1. **Transpile** each primitive template from GTL to Rhai source (backtick lines
   → `emit(...)` calls; interpolations → `fmt(expr)` splices — GTL §7), keeping a
   **line map** (author line ↔ transpiled line) for diagnostics.
2. **Compile** each transpiled source once with `Engine::compile` into a cached
   `AST`.

Per `expand()`:

3. Build the fresh scope (§2), then `eval_ast_with_scope` the primitive's cached
   `AST`. `emit(...)` appends to the call's output buffer; the buffer is the
   primitive's GCode result.

Compiling once and evaluating many amortises Rhai parsing across the thousands of
holes/segments a board produces, and keeps the hot path allocation-light. The
cached `AST` is shareable, which fits the architecture's thread-pooled RHAI note.

---

## 6. Error model

`Coder::expand` returns `Result<String, CoderError>` and never panics:

```rust
enum CoderError {
    /// GTL transpile error or Rhai compile error, mapped back to the author's
    /// source line via the line map — not the transpiled line.
    Parse   { primitive: String, line: usize, col: usize, message: String },
    /// Rhai evaluation error: undefined variable, type/unit mismatch, etc.
    Runtime { primitive: String, line: usize, message: String },
    /// The script called Rhai `throw expr` to assert a precondition.
    Thrown  { primitive: String, value: String },
}
```

- **Parse** is detected up front at compile time (step 2), so a broken template
  fails before generation emits anything.
- **Thrown** lets a primitive validate its inputs and abort with a clear message:

  ```rhai
  if z_bottom > 0 { throw "z_bottom must be below the work surface" }
  ```

  Rhai supports `throw`; a thrown value bubbles up as `Thrown`.

All three are typed diagnostics, per the architecture requirement that "RHAI
expression errors surface as typed diagnostics, not panics." Each carries the
primitive name and (for Parse/Runtime) the author-source location.

---

## 7. The `expand` API

```rust
// The app resolves effective job values and groups them into namespaces. The
// engine receives only this generic namespaced bundle — it never sees
// `FixtureProfile`, `ToolsetProfile`, or AppData.
let job = job_context! {
    machine:   ns!{ max_feed_rate, spindle_rpm_min, spindle_rpm_max },
    cnc:       ns!{ z_safe, line_numbering_increment },
    fixture:   ns!{ backing_board_thickness },
    toolset:   ns!{ size },
    machining: ns!{ peck },
    // run-level values sit at the top of the job context
    pcb_filename, timestamp, scaling_x, scaling_y,
};

// Compiles + caches every primitive AST from the active CNC profile and captures
// the job context (program layer). Built once, reused for the whole generation.
let mut coder = Coder::new(&cnc_primitives, job)?;

// Sets the operation layer — the flat, terse hot values — for the tool about to run.
coder.enter_operation(args!{ tool_diameter, z_feed, xy_feed, rpm });

// One primitive call; args! is the call layer, overlaid on operation + job context.
let gcode = coder.expand("peck_drill", args!{ x, y, z_bottom, z_retract })?;
```

The engine is **long-lived**: `Coder::new` runs once, then `expand` runs per
primitive across the board. Only the operation and call layers change between
calls; the job context, unit mode, and `N` counter carry over (§2.1, §3.2).

- `args!{ x, y }` is field-shorthand sugar producing a `name → typed value` map
  (call layer); `job_context!`/`ns!` are the same sugar for the namespaced job
  context (macro names illustrative).
- In production the `JobProcessor` emits a `PrimitivePlan` of `(name, args)`
  steps and the Coder iterates it, calling `expand` per step; `expand!` is the
  clean single-call unit for tests and direct use.
- `metric()`/`imperial()` mutate the coder's program state (unit mode) and are
  normally invoked from `initialise`; `enter_operation`/`expand` read it.

Snapshot tests drive `expand` with a fixed `PrimitivePlan` per CNC profile and
compare against golden GCode (architecture: "Snapshot tests per CNC profile").

---

## 8. Built-in surface

Pure functions only — no I/O, no app-state access. Two provenances: `emit` and
the default `fmt(scalar)` are **`gtl`-crate built-ins**; everything unit-aware
below — `metric()`/`imperial()`, the unit-typed `fmt` overloads, and the
unit-type math and accessors — is **registered by the Coder** on the engine
(§1.1). `throw` is Rhai core.

- **Unit switching:** `metric()` (emits `G21`, sets metric formatting),
  `imperial()` (emits `G20`, sets imperial).
- **Math:** `min`, `max`, `abs`, `clamp` over numbers and unit types.
- **Formatting:** `fmt(v)` (internal; the emit-time type-driven formatter, GTL
  §4) and the `.mm/.inch/...` accessors. `fmt` is a thin type-dispatch over
  `units::machine::{number_length, number_feed, number_speed}` — the shared
  machine formatter already built (see `unit-display.md` §6). It must not grow
  its own rounding.
- **Control:** `throw expr`.
- **Emit:** `emit(str)` (what a backtick line compiles to; also directly callable
  for programmatic emission).

Custom attributes defined on the CNC profile are added to scope automatically and
documented alongside the primitive's variables.

---

## 9. Catalogs (phase 2)

Two declarative registries, needed by both the generator and the editor:

- **Variable catalog** — per primitive, `[(name, type, unit, description,
  layer)]`. Finalises the canonical variable contract deferred in GTL §12.
  Purposes: (a) the generator knows what to inject; (b) the editor documents each
  variable; (c) the loader can warn when a template references an unknown name.
- **Built-in catalog** — `[(signature, description)]` for the §8 surface, driving
  the editor's function reference.

---

## 10. Expression editor (phase 3)

A pop-up editor for a single primitive, built on the engine + catalogs:

- **Syntax highlighting** — GTL-aware: backtick emit lines, `{…}` interpolation,
  Rhai keywords/strings/comments. Start with a lightweight GTL tokenizer rendering
  coloured spans rather than embedding a heavy editor component in the webview.
- **Variable panel** — from the variable catalog: every variable available to
  this primitive, with type, unit, and meaning; click to insert at the cursor.
- **Function panel** — from the built-in catalog.
- **Continuous verify** — on each edit (debounced), run transpile + `compile`
  (no eval) and show diagnostics inline against the author's lines.
- **Dry run** — run the real engine against a scope of sample values (seeded from
  the variable catalog, editable by the user) and show the emitted GCode live.

---

## 11. Phasing

1. **Engine core** — typed-value scope, transpile+compile+eval, three-layer
   scope, `expand`, error model. Unblocks generation; snapshot-testable.
2. **Catalogs** — variable + built-in registries (finalises the variable
   contract).
3. **Editor pop-up** — highlight, variable/function panels, continuous verify,
   dry run.

---

## 12. Open decisions (before phase 1)

1. **Typed unit values in Rhai (recommended) vs. pre-converted numbers.** Typed
   is what makes `z - peck` and `z > z_bottom` work and is the whole point of the
   GTL model; it costs a few small operator impls on the `units` crate. Strong
   recommendation: typed.
2. **`args!` shape — generic `name → Value` map (recommended for phase 1) vs. a
   typed args struct per primitive.** The map is flexible and quick; a typed
   struct is safer but adds boilerplate and couples the Coder to each primitive's
   signature. Recommendation: generic map now, revisit once the variable catalog
   exists.
3. **Line-numbering ownership.** Confirm the `N`-word counter lives on the engine
   (program state) and is applied at emit, not authored in templates. (Assumed
   yes here.)
4. **Crate name & split — RESOLVED (2026-07-21).** The engine ships as the
   generic `crates/gtl` (*Generic Template Language*); the GCode-specific "Coder"
   is the app-side host layer in `src/gcode/` (see §1.1). The name `coder` was
   dropped — the crate emits plain strings, not GCode.
5. **Namespacing mechanism.** Job context as Rhai object maps (`machine.*`,
   `fixture.*`, …, recommended) vs a fully flat scope (`machine_max_feed_rate`).
   Maps give legible provenance and collision-free names; the cost is that an
   unknown `ns.field` is a silent `()` in Rhai by default — caught instead by the
   load-time variable-catalog check (§9), or by enabling Rhai's
   fail-on-invalid-map-property for a hard error. Hot per-call values stay flat
   regardless (§2.2). (App-side decision; the `gtl` crate is neutral — it just
   runs whatever scope it is handed.)
6. **Where the `units` ↔ Rhai registration lives — RESOLVED (2026-07-21).**
   Host-side, by construction: the `gtl` crate is generic and cannot depend on
   `units`, so the app registers the unit-typed `fmt` overloads (and `metric()`/
   `imperial()`) on the `gtl::Gtl` engine when it builds the Coder (§1.1). A
   feature-gated `units::rhai` module stays an option if a second consumer ever
   needs the same registration.

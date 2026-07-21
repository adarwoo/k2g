# GCode Template Language (GTL)

Status: **design draft** — the surface grammar. The generic engine that runs it
now exists as `crates/gtl` (*Generic Template Language* — it emits plain strings;
see `gcode-engine.md` §1.1). This document describes the **GCode dialect** the app
layers on top: the backtick/emit grammar plus the GCode-specific built-ins
(`metric()`/`imperial()`, unit-typed `fmt`). The old `src/gcode/template.rs` is
the rewrite target, superseded by `gtl` + the app-side Coder.

This document supersedes the "Strings between `{}` are interpreted as RHAI"
model described in `Specification.md` §6.6. That text-first model stays fine for
one-liners but collapses into unreadable string concatenation as soon as a
primitive needs a loop (see the manual peck cycle, below). GTL inverts the
default: a primitive is a **Rhai script**, and lines that emit GCode are marked.

---

## 1. Design principles

1. **Modal units, tracked by the engine, not the author.** A CNC coordinate
   system is modal (`G20`/`G21`). A value interpolated into emitted GCode is
   converted to the *active* unit system automatically. The author never writes
   `.mm` in the common case and never tracks which unit is live.
2. **Simple stays simple.** ~90% of primitives are a line or two of GCode with a
   few substitutions. Those must read as GCode with minimal ceremony.
3. **Complex stays readable.** Loops and conditionals (manual peck, ramping,
   arc-approximated beziers) are ordinary Rhai control flow that *emits* lines —
   never manual `out += "..." + x.to_string()` assembly.
4. **Computation is typed; conversion happens at emit.** Inside script logic,
   values keep their unit type, so `z > z_bottom` and `z - peck` are
   unit-correct. Conversion to a bare number happens only when a value is
   emitted.

---

## 2. The model in one paragraph

A primitive template is a Rhai program. Every physical line is Rhai **except**
lines whose first non-whitespace character is a backtick (`` ` ``); those are
**emit lines**. An emit line is verbatim GCode text in which each `{ expr }` is
evaluated in the current Rhai scope, unit-formatted, and spliced in. The engine
transpiles each emit line into a single `emit(...)` Rhai statement, then runs the
whole thing as one script. Whatever is emitted, in order, is the primitive's
GCode output.

---

## 3. Lexical rules

### 3.1 Line classification

- A line is an **emit line** iff its first non-whitespace character is `` ` ``.
- Indentation *before* the backtick is Rhai source layout and is discarded.
- Everything *after* the backtick (including spaces) is the emit payload, taken
  verbatim except for interpolation and escapes.
- Every other line is passed through to Rhai unchanged.

```
    `G1 Z{z} F{z_feed}      // emit line; leading spaces are code indentation
let z = z_retract;          // Rhai line
while z > z_bottom {        // Rhai line
```

A bare backtick emits a blank line:

```
`                           // => one empty output line
```

### 3.2 Interpolation

- Inside an emit line, `{ expr }` holds any Rhai expression, evaluated in scope.
- Because braces delimit, no spaces are needed between fields: `X{x}Y{y}` is
  valid and emits e.g. `X3.2Y7.0`.
- To emit a literal brace, double it: `{{` → `{`, `}}` → `}`. (GCode never uses
  braces, so this is rarely needed.)
- Interpolation applies **only** on emit lines. On Rhai lines, `{ }` is Rhai.

### 3.3 Comments

- Rhai lines use Rhai comments: `//` and `/* */`.
- GCode comments (`(...)`, `;...`) are just literal text and are only meaningful
  inside emit lines, where they are emitted verbatim.

---

## 4. The type-driven formatter (`fmt`)

Every `{ expr }` is passed through the engine's formatter before splicing. The
formatter dispatches on the value's **type** and emits a bare number (no unit
suffix — the unit is implied by the modal `G20`/`G21` state):

| Value type          | Metric mode         | Imperial mode        |
|---------------------|---------------------|----------------------|
| `Length`            | millimetres         | inches               |
| `FeedRate`          | mm/min              | in/min               |
| `RotationalSpeed`   | rpm                 | rpm (unit-invariant) |
| `Angle`             | degrees             | degrees              |
| integer / float     | as-is               | as-is                |
| string              | verbatim            | verbatim             |

Numbers are tidied to the unit system's display precision and rendered as
integers when whole (`-40`, not `-40.000`). This reuses the rounding already in
`crates/units/src/display.rs`.

**Escape hatch.** To force a specific unit regardless of mode, use an explicit
accessor that returns a plain number: `{ z.mm }`, `{ z.inch }`,
`{ z_feed.mm_per_min }`. Those bypass modal conversion (a plain number formats
as itself).

---

## 5. Modal unit state

- `metric()` sets the formatter to metric **and** emits `G21`.
- `imperial()` sets the formatter to imperial **and** emits `G20`.
- The mode is **program-scoped state**: it is set once (normally in
  `initialise`) and *persists across every primitive* in the generated program,
  exactly mirroring the machine's own modal state. `peck_drill` does not re-set
  units; it inherits whatever `initialise` established.

This is the mechanism behind principle #1: the same call that tells the machine
the unit (`G21`) tells the formatter the unit, so the two can never desync.

---

## 6. Typed values in scope

Variables provided to a primitive are the `units` crate's typed values
(`Length`, `FeedRate`, `RotationalSpeed`, `Angle`) plus plain scalars and
strings. To make script logic natural, the engine registers, at minimum:

- Arithmetic: `Length ± Length → Length`, `Length * number → Length`,
  `FeedRate * number → FeedRate`.
- Ordering: `<  >  <=  >=  ==` on each unit type (compared in canonical units).
- Helpers: `max(a, b)`, `min(a, b)`, `abs(a)`, `clamp(v, lo, hi)` for unit types.
- Accessors: `.mm`, `.cm`, `.inch`, `.mil` on `Length`; `.mm_per_min`,
  `.in_per_min` on `FeedRate`; `.rpm`; `.degrees`, `.radians`.

(The `units` types are `Copy` newtypes today, so these are small `register_fn`
additions or thin operator impls — no redesign of the crate.)

---

## 7. Transpile

The engine rewrites each emit line into one `emit(...)` call and passes the rest
through untouched. `emit(str)` appends the string plus a newline to the output
buffer; `fmt(v)` is the formatter from §4. Both are also callable directly by
authors who want programmatic emission.

Emit line → Rhai:

```
`G1 Z{z} F{z_feed}
```
becomes
```
emit("G1 Z" + fmt(z) + " F" + fmt(z_feed));
```

Literal segments become string literals (`"`, `\`, newline escaped; `{{`/`}}`
unescaped); each `{ expr }` becomes `+ fmt(expr) +`.

---

## 8. Worked examples

### 8.1 `move_slow` — the 90% case

Source:
```
`G0 X{x} Y{y}
```
Transpiled:
```
emit("G0 X" + fmt(x) + " Y" + fmt(y));
```
Output (metric, x = 3.2 mm, y = 7 mm): `G0 X3.2 Y7`

### 8.2 `initialise` — establishes the modal unit

```
`(Created by k2g from '{pcb_filename}' - {now()})
`(Reset all back to safe defaults)
`G17 G54 G40 G49 G80 G90
metric();
`G10 P0
`G0 Z{z_safe}
if has_positioning_pins {
    `G56
} else {
    `G54
}
```

Notes:
- `metric()` emits `G21` at its position and fixes the unit for the whole
  program.
- The `if` is a plain Rhai line — no wrapping braces. (The old text-first model
  needed `{ ... }` to "escape into" Rhai; here Rhai is the default, so control
  flow is written directly.)
- Emit is **line-oriented**: a backtick is recognised only as the first
  non-whitespace character of a line, never mid-expression. So the two branches
  are broken onto their own lines rather than written inline as
  `if ... { `G56 } else { `G54 }` (see §11).

### 8.3 `peck_drill` — the payoff (manual cycle, no `G83`)

```
// Manual peck cycle for controllers without a canned G83.
`G0 X{x} Y{y}
`G0 Z{z_retract}
let z = z_retract;
while z > z_bottom {
    z = max(z - peck, z_bottom);
    `G1 Z{z} F{z_feed}
    `G0 Z{z_retract}
}
```

Transpiled:
```
// Manual peck cycle for controllers without a canned G83.
emit("G0 X" + fmt(x) + " Y" + fmt(y));
emit("G0 Z" + fmt(z_retract));
let z = z_retract;
while z > z_bottom {
    z = max(z - peck, z_bottom);
    emit("G1 Z" + fmt(z) + " F" + fmt(z_feed));
    emit("G0 Z" + fmt(z_retract));
}
```

Compare against today's text-first equivalent in `template.rs` (`SPEC_TEMPLATE2`),
which is a wall of `out += "G1 Z" + next_z.to_string() + ...`. Same behaviour,
readable.

### 8.4 `peck_drill` — machine *with* `G83` (still a one-liner)

```
`G83 X{x} Y{y} Z{z_bottom} R{z_retract} Q{peck} F{z_feed}
```

The grammar does not force loops on machines that have canned cycles; the simple
form is unchanged.

---

## 9. Grammar (EBNF)

```ebnf
template      = { line } ;
line          = emit_line | rhai_line ;

emit_line     = ws , "`" , emit_payload , newline ;
emit_payload  = { emit_text | interp | brace_escape } ;
emit_text     = ? any chars except "{", "}", newline ? ;
brace_escape  = "{{" | "}}" ;
interp        = "{" , rhai_expr , "}" ;

rhai_line     = ? any physical line whose first non-ws char is not "`" ? ;
rhai_expr     = ? a Rhai expression, balanced braces, string-aware ? ;
ws            = { " " | "\t" } ;
```

`interp` scanning is brace-depth aware and string-literal aware (a `}` inside a
Rhai string in the expression does not close the interpolation) — the existing
`parse_segments` scanner in `template.rs` already implements exactly this and is
reused.

---

## 10. Errors and diagnostics

- **Interpolation parse errors** (unbalanced `{`) are reported with the source
  line/column of the emit line.
- **Rhai parse/eval errors** are reported against the *author's* source, not the
  transpiled script. The transpiler therefore maintains a line map (author line
  ↔ transpiled line). This must exist from day one; retrofitting it is painful.
- Per `Specification.md` §6.6, evaluation must be deterministic for identical
  project inputs and must surface as typed diagnostics, never panics.

---

## 11. Line-oriented emit (settled) and its limits

**Decision:** emit is line-oriented. A backtick is recognised only as the first
non-whitespace character of a line; inline emit (a backtick mid-expression) is
deliberately *not* supported. This keeps the transpiler a pure line pre-pass
with no Rhai parsing, at the cost of spreading a branch across a few lines. This
was weighed against an inline form and rejected as not worth the scanner
complexity.

Consequences:

- Emit detection **runs before Rhai parsing**. A continuation line of a Rhai
  multi-line string literal that happens to begin with `` ` `` would be misread
  as an emit line. In practice primitives don't contain such literals; documented
  so it isn't a surprise.
- Control flow that emits must break each emit onto its own line (see §8.2), not
  inline as `if cond { `A } else { `B }`.

---

## 12. Open decisions (need sign-off before coding)

1. **Static preamble: per-line backtick, or a raw block?**
   A large literal header (`initialise`) is ~10–15 backtick lines. Options:
   - **(A, recommended)** Per-line backtick everywhere. One uniform rule, no
     second syntax. Add a raw block later only if it bites.
   - **(B)** Add a triple-backtick fenced block now: a line that is exactly
     ```` ``` ```` opens/closes a raw region emitted verbatim (still honouring
     `{ expr }`), so headers need no per-line prefix.
   This choice shapes the line scanner, so decide first.
2. **Canonical variable set per primitive.** The names used above (`z_safe`,
   `z_retract`, `z_bottom`, `peck`, `z_feed`, `x`, `y`, `rpm`, `slot`, …) are
   illustrative. The authoritative per-primitive variable contract (and which
   are `Length` vs `FeedRate` vs scalar) is owned by the primitive definitions
   and is TBD in a follow-up.
3. **Keep `use_metric()`/`use_imperial()` names, or rename to
   `metric()`/`imperial()`?** This note uses the shorter names; the current
   engine uses `use_*`. Cosmetic, but pick one before writing seed primitives.
```

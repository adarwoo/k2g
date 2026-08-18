# K2G — CNC profile safety & drill cycle rework

## Why

A generated program crashed the machine (MASSO G3, ATC, automatic tool setter). Root cause and
associated defects, from reviewing the emitted GCode:

| # | Defect | Consequence |
|---|---|---|
| 1 | No machine-frame retract after `M6` | Spindle parked over the tool setter after measurement; the drill cycle's R-plane rapid descended into it. **Crash / ESTOP.** |
| 2 | No machine-frame retract at program start | Program start position is unknown; the initial `G0 Z20` is a bet on where the work zero is. |
| 3 | Canned cycle never cancelled | `G81` still modal across `M6`. Any macro motion is interpreted as a hole. |
| 4 | No `G98`/`G99` selected | Inherited `G98` returns to initial level after every hole — 20 mm of pointless travel per hole. |
| 5 | Retract plane hardcoded/oversized (`R5`) | Same. |
| 6 | Full `G81` block emitted per hole | Defeats the canned cycle; large files. |
| 7 | No `G94` in the safety line | If the control is left in `G95`, the F word becomes feed-per-revolution. |
| 8 | Drill depth ignores point length | Breakthrough measured to the tip, not to full diameter. Holes under-drilled. |
| 9 | Feed/speed clamping scaled RPM down | `S24000 F4800` on a 0.30 mm drill = 0.2 mm/rev, ~10× over, and starves surface speed. |
| 10 | `line_format` skips an increment on comment lines | Cosmetic; N70 missing from output. |

This plan covers all shipped CNC profile YAML files, the CNC profile schema, the affected
primitives, and the generator-side feed/depth calculations.

---

## Ground rules

**This plan was written without access to the repository.** Names below marked *(proposed)* are
suggestions, not existing identifiers. Do not invent a name that collides with or duplicates an
existing one — complete Phase 0 first and reconcile.

**Stay dialect-agnostic.** Excellon output is a supported target. No GCode word, and no
GCode-specific concept, may enter a schema field, a profile field name, or a validation rule.
Anything that resolves to `G53`, `G80`, `G94` etc. lives *inside a primitive template*, where the
profile author owns it. This is the same rule already applied to `origin_reference`.

**Primitives own postconditions.** The recurring failure is a primitive that emits text without
guaranteeing machine state on exit. Where this plan adds a contract, add it to the primitive's
schema documentation so profile authors editing a cloned template know what they must preserve.

**Migrate, don't break.** Existing user profiles on disk must load. Extend
`normalize_cnc_value` for every new or retired key.

---

## Phase 0 — Inventory (do this before changing anything)

Report back before proceeding. Specifically:

1. The full CNC profile JSON-Schema — the `machine` block keys and the primitive list with each
   one's `x-kind`, `x-category`, and declared variables in scope.
2. Whether a tool-change primitive exists, and if not, where `T<n> M6` is currently emitted
   (generator-side, a preamble template, or inlined in an operation).
3. The exact signature of `drill`: variable names, unit types, and whether it is invoked once per
   hole or once per hole list.
4. Whether any hook exists between operations, or before/after the tool change, where a
   machine-frame move could be emitted. (Preamble/postamble hooks were discussed in July but were
   not in schema v1.)
5. Every shipped CNC profile YAML under version control, and any test fixtures that assert on
   generated output.
6. Where feed rate is currently sourced — tool stock entry, catalog entry, or computed — and where
   the `max_feed_*` clamp with proportional spindle scaling is implemented.

---

## Phase 1 — Schema additions (`machine` block)

Add, with migration defaults chosen so existing profiles keep working:

| Key *(proposed)* | Type | Purpose | Default on migration |
|---|---|---|---|
| `tool_change_clearance` | measurement (length) | Distance below machine top that is guaranteed clear of setter, rack, dust hood and clamps. Consumed by the safe-move primitive. | `0` (i.e. machine top) |
| `spindle_spinup_seconds` | number | Dwell after spindle start before first cut. | `2.0` |
| `min_surface_speed` | measurement (speed) | Floor used by the feed/speed resolver before it refuses to reduce RPM further. | see Phase 4 |

Notes:

- `tool_change_clearance` is a **machine** property, not a fixture property. The fixture profile's
  existing `Z safe` / `Z retract` are work-frame values describing clearance above the board and
  clamps; they cannot describe the tool setter, which the fixture knows nothing about. Do not reuse
  them here, and do not move them.
- Keep these unit-typed like every other measurement (bare-number comparison must remain an error).

## Phase 2 — Primitive contracts

### 2a. New primitive: `move_safe` *(proposed)* — `x-kind: callable`

**Contract:** on exit, the tool is clear of every fixture, rack and metrology obstacle, and no
motion in any other axis has occurred.

Variables in scope: `tool_change_clearance`. MASSO template emits `G53 G0 Z0` (or
`G53 G0 Z{-tool_change_clearance}`). An Excellon profile may emit nothing or a machine-specific
retract — that is the point of making it a primitive rather than a schema-driven line.

### 2b. Tool change

If a `tool_change` primitive exists, extend its contract. If tool changes are emitted generator-side,
promote them to a primitive.

**Contract — on exit:**

1. No canned/modal cycle is active.
2. Tool is at machine clearance (i.e. the template ends with the equivalent of `move_safe`).
3. Spindle is commanded at the operation's RPM and has been given `spindle_spinup_seconds` to reach it.
4. Position in X/Y is undefined — callers must position explicitly.

Point 2 is the crash fix. `M6` leaves the spindle wherever the controller's macro leaves it, which
on a machine with `tool_length_measurement: auto_setter` is directly above the setter. The
generator must never inherit that position.

Ordering note for the template: issue the spindle start immediately after `M6`, then the retract,
then the dwell — the spin-up overlaps the motion. A 60 000 rpm spindle takes seconds.

### 2c. `drill` — signature change

Currently invoked per hole with x, y, z bottom, z retract, feed. It must become able to emit a
modal cycle correctly. Two options; **prefer (A)**:

- **(A) Pass the whole hole list plus the operation context.** The primitive emits setup, body and
  cancel itself. Simplest contract, and the template author can choose non-modal output (Excellon,
  or a controller without canned cycles) by just looping.
- **(B) Keep per-hole invocation, add `first`, `last`, `index`, `previous`.** Less disruptive but
  pushes modal-state bookkeeping into every template.

**Contract — on exit:** no modal cycle active, tool at the retract plane or above.

Also in scope for the template:

- Explicit retract-mode selection (`G99`) rather than inheriting.
- Retract plane derived from a clearance above the board surface (0.5–1.0 mm), not a constant.
- Subsequent holes emit only changed coordinates.

### 2d. `line_format`

Fix the increment consumed by comment lines (N70 missing). Decide explicitly whether comments are
numbered; either is acceptable for MASSO, but it must be consistent so two generated programs diff
cleanly.

---

## Phase 3 — Template rewrites (all shipped CNC profiles)

### Initialise template

Current MASSO output:

```gcode
N20 G17 G40 G49 G80 G90
N30 G55
N40 G21
N50 G0 Z20
N60 M5
```

Target shape:

```gcode
N20 G90 G94 G17 G40 G80        ; G49 conditional - see below
N30 G21                        ; units before anything numeric
N40 G55                        ; from set_origin
N50 G53 G0 Z0                  ; from move_safe - safe from ANY start position
N60 M5
```

Changes:

- **Add `G94`.** Cheap insurance against an inherited `G95`.
- **Units before the origin select**, and before any motion.
- **Replace the work-frame `G0 Z20` with `move_safe`.**
- **Make `G49` conditional on `tool_length_measurement == manual`.** On an `auto_setter` machine the
  controller manages tool length from its own table; the safety line's job is to cancel modal state
  the program might have inherited, not to fight the control. It is harmless where it currently
  sits (before the first `M6`) but becomes actively dangerous if a user clones the profile and moves
  it.

### Tool change template

```gcode
(load tool T2: unionfab-PSeriesdrill-D0.30)
N70 T2 M6
N80 S60000 M3                  ; start spooling
N90 G53 G0 Z0                  ; move_safe - M6 left us over the setter
N100 G4 P2.0                   ; spindle_spinup_seconds
```

### Drill template

```gcode
N110 G0 X70.967 Y44.967        ; position at machine clearance height
N120 G99 G81 Z-2.69 R0.5 F750  ; cycle setup, once
N130 X72.267                   ; changed coordinates only
N140 Y46.267
...
N200 G80                       ; cancel - postcondition
```

### Program end

Ensure `move_safe`, spindle stop, and `M30` are emitted.

### Invariant to hold across all profiles

> Every X/Y traverse and every tool change occurs at machine clearance. The only descent below it is
> a cycle's own approach to its retract plane, by which point X/Y is already over the work.

---

## Phase 4 — Feeds, speeds and depth (generator-side)

### 4a. Replace rated feed with chipload

The observed `S24000 F4800` on a 0.30 mm drill is 0.2 mm/rev — roughly 10× over, and it dropped
surface speed to ~23 m/min where carbide in FR4 wants 50–100. The clamp rule behaved as designed and
produced a tool-breaking result, which is the design smell already noted in July.

Change the tool stock/catalog schema to store **chipload (length per revolution)** rather than a
rated feed. Then:

```
feed = chipload × rpm
if feed > max_feed_z:
    rpm' = max_feed_z / chipload
    if surface_speed(rpm', diameter) < min_surface_speed:
        warn (or throw, per validation policy) — this tool cannot be run on this machine
    else:
        rpm = rpm', feed = max_feed_z
```

`S` and `F` can then never disagree, and a bad catalog entry surfaces as an implausible chipload
rather than a plausible-looking pair.

Migration: for existing entries, `chipload = rated_feed / rated_rpm`. **Log every derived value** —
this will expose the entries that were already wrong, including the unionfab P-series ones.

### 4b. Drill depth formula

Current: `Z-2.15` on a 1.6 mm board = 0.55 mm breakthrough, short of the 1 mm backboard engagement
in the spec.

```
point_length = (diameter / 2) / tan(point_angle / 2)
z_bottom     = -(board_thickness + point_length + breakthrough)
```

- Add **point angle** to the tool stock schema (default 118° or 130° for PCB drills — confirm the
  supplier spec).
- Define `breakthrough` as measured from where the **full diameter** clears the board's bottom face,
  not where the tip clears. Update the field help text; the distinction is negligible at 0.30 mm
  (~0.09 mm) but is ~0.96 mm on a 3.175 mm pin drill, i.e. the entire engagement allowance.
- Re-check the locating-pin depth rule against this — the "1 mm engagement, warn if unachievable"
  rule should be evaluated on full-diameter depth.

### 4c. Optional: pecking threshold

Straight plunge is correct for PCB drilling at high RPM. If a job ends up RPM-limited, a 0.30 mm
drill at >5:1 aspect ratio will pack chips. Consider a CNC-profile threshold: below some surface
speed, switch to a peck cycle with Q ≈ 2×D. Low priority — do not implement before Phases 1–3.

---

## Phase 5 — Migration

Extend `normalize_cnc_value` for the new `machine` keys with the defaults in Phase 1. Existing
user profiles must load and generate without the user touching them, though they will need to set
`tool_change_clearance` to benefit — surface that in the profile editor rather than silently.

Users who have cloned and edited a shipped profile will **not** receive the template fixes. Decide
and document the policy: warn on load when a cloned profile's template predates the fix, or ship a
note in the release. Do not silently overwrite user templates.

---

## Phase 6 — Tests

1. **Golden-file test per shipped CNC profile** on a representative board, so template regressions
   are visible in review.
2. **Structural assertions on generated output**, dialect-independent where possible:
   - no work-frame Z motion precedes the first tool change
   - every tool change is followed by a machine-frame retract before any X/Y motion
   - every modal cycle is cancelled before the next tool change and before program end
   - line numbers are contiguous
3. **Unit test for the depth formula** across the pin diameter list (2, 2.5, 3, 3.175, 3.2 mm),
   asserting full-diameter breakthrough.
4. **Unit test for the feed/speed resolver**, including the case where the clamp bites and the case
   where `min_surface_speed` refuses.
5. Re-run the `central_control` board and diff against the extract in this document.

---

## Acceptance criteria

- The `central_control` program starts with the spindle parked over the tool setter and completes
  without collision.
- No `G81` block appears without a matching `G80`.
- Retract plane is within 1 mm of the board surface; no per-hole return to initial level.
- A 0.30 mm drill produces a chipload in the 0.01–0.02 mm/rev range at the machine's usable RPM.
- Drill depth achieves the specified breakthrough at full diameter.
- All existing user profiles load without error.

---

## Bring back for decision

1. Option (A) vs (B) for the `drill` signature.
2. Whether `move_safe` should be a distinct primitive or a required tail of the tool-change template.
3. Whether an out-of-range feed/speed should warn or throw. `set_origin` throws; consistency argues
   for throwing, but a throw blocks program output entirely and a conservative clamp may be
   preferable for a merely suboptimal feed.
4. Whether comments are numbered by `line_format`.
5. Whether to add preamble/postamble hooks now — they were deferred from schema v1 and several
   items here would sit naturally in them.

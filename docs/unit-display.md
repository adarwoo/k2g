# Unit Display Unification

Status: **steps 1–6 implemented** (2026-07-21). Rounding, number formatting, and
the machine (GCode) formatter all live in the `units` crate now, so on-screen
values and emitted coordinates are guaranteed to round identically. The GTL
engine's `fmt`, when built, is a thin dispatcher over `units::machine`.

## As-built

- **`round_to_step` unified, then `format_number` extracted.** One
  `pub(crate) round_to_step` and one `pub(crate) format_number(value, decimals)`
  trim core in `units`; both `format_trimmed` (editable-field) and
  `format_with_unit` (compact/summary) delegate to it. Byte-identity preserved by
  keeping `format_with_unit`'s `max_decimals == 0` integer cast
  (round-half-away) distinct from `format_number`'s `{:.0}` (round-half-even) —
  the only edge where the two legitimately differ.
- **Step 6 — machine formatter built.** `units::machine::{number_length,
  number_feed, number_speed}` format a typed quantity as a bare number (no
  suffix) in the active machine system, on the shared core; `Mil` maps to
  `Imperial` (GCode has no mil dialect). Tests pin `-40` / `-25.4` / `-1`
  (imperial) — the same outputs the WIP engine's own formatter produced. The GTL
  engine's emit-time `fmt` is now just type-dispatch over these.
- **`UserUnitSystem` persist mapping is now enum methods** (`as_settings_str` /
  `from_settings_str`), replacing the hand-written matches in `state.rs`.
- **`JobConfig` `xx_mm: f32` fields promoted to typed `Length`** (`tab_width`,
  `tab_width_baseline`, `mouse_bite_pitch`) — the project config joined the typed
  system; the sidebar dropped its `Length::from_mm(x as f64)` display wrappers.
- **UI formatter relocated verbatim as free functions, not new trait methods.**
  Rather than adding `display_string`/`edit_string`/`parse_in_system` methods,
  the entire `ui/unit_format.rs` moved into `units::user_format` unchanged
  (retargeted to `UserUnitSystem`). This guarantees byte-identical UI output —
  the module is the same code — and made the call-site migration a one-line
  import alias per file. Method-style sugar can be added later if wanted.

The rest matches the spec: `UserUnitSystem` gained `Mil`; `app_shell::UnitSystem`
and its dead `user_unit_system()` bridge were deleted; the UI now formats through
the `units` crate only.

---

Original spec follows.

The GCode engine (`gcode-engine.md`) depends on its outcome: the engine's
emit-time formatter (`fmt`, GTL §4) must round identically to the UI, which is
only true if there is one formatter.

---

## 1. Problem

There are two parallel unit-display stacks with duplicated logic:

- **`crates/units/src/display.rs`** — the canonical layer. `UserUnitSystem`
  (`Metric` | `Imperial`), the `UserUnitDisplay` trait
  (`unit_display -> UnitDisplay{user, native}`, `user_value -> f64`), and private
  `round_to_step` / `format_with_unit` / `format_native_*`.
- **`src/ui/unit_format.rs`** — the UI layer. Uses `app_shell::UnitSystem`
  (`Metric` | `Imperial` | **`Mil`**) and **re-implements** `round_to_step`,
  `format_trimmed`, and its own `MM/IN/MIL_PRECISION` tables, plus ~24 formatting
  / parsing / label / step functions.

The bridge between them, `UnitSystem::user_unit_system()`, has **zero callers** —
so the UI never routes through the units crate. Two independent rounding
implementations that can (and eventually will) drift, and a third would appear if
the GCode engine grew its own.

The only thing the UI layer genuinely adds is the **`Mil`** display mode. And
`units::LengthUnit` already has `Mil` and `Thou` variants (with `Length::as_mil`)
— so mil is already a first-class unit; only the *display selector*
(`UserUnitSystem`) lacks a mil option.

---

## 2. Goal

One owner of rounding, precision, labels, steps, and formatting: the **`units`
crate**. The UI and the GCode engine both consume it. Delete
`ui/unit_format.rs`'s duplication and `app_shell::UnitSystem`.

---

## 3. The unified selector

**Extend `units::UserUnitSystem` to three variants:** `Metric | Imperial | Mil`.

Rationale: the shell already presents a 3-way toggle (Metric / Imperial / Mil),
and `Mil` is **length-only** — for feed it means in/min, for angle/rpm it is
irrelevant. So the three trait impls change minimally:

| Quantity        | `Metric`  | `Imperial` | `Mil`               |
|-----------------|-----------|------------|---------------------|
| `Length`        | mm (3dp)  | in (4dp)   | **mil (0.1)**       |
| `FeedRate`      | mm/min    | in/min     | in/min (= Imperial) |
| `Angle`         | deg       | deg        | deg (system-agnostic) |
| `RotationalSpeed` | rpm     | rpm        | rpm (system-agnostic) |

Then `app_shell::UnitSystem` is **deleted** and `units::UserUnitSystem` is
re-exported for the shell/runtime/ui to use as the single type.

Alternative considered (Option B): model a separate "length display unit"
(mm/in/mil) orthogonal to a 2-value system. More correct — mil is a length
presentation, not a whole system — but it threads an extra parameter everywhere
for one length-only case. Deferred; Option A matches the existing 3-way UX with
far less churn. Revisit only if a second length-only presentation appears.

---

## 4. What the units crate owns (the single API)

**Shared primitives** (the one formatter core):

- `round_to_step(value, step)` — already exists; keep as the sole rounder.
- `format_number(value, max_decimals) -> String` — extract the suffix-less core
  of `format_with_unit` (fixed decimals, trailing-zero trim, `-0` → `0`). Both
  `format_with_unit` (UI, adds a suffix) and the engine's `fmt` (no suffix) build
  on this. This is the single source of "how a number prints."

**Per-quantity display** (existing `UserUnitDisplay`, extended with `Mil` arms):

- `unit_display(sys) -> UnitDisplay { user, native }`
- `user_value(sys) -> f64`

**Convenience the UI needs, moved down from `unit_format.rs`:**

- `display_string(sys) -> String` — `user`, plus ` [native]` when native differs
  (replaces `format_length_display` / `format_feed_display` composition).
- `edit_string(sys) -> String` — the editor seed: the native value with its unit
  stripped when the source unit already matches `sys`, kept otherwise (replaces
  `format_*_edit_display`).
- `UserUnitSystem::length_label()` / `feed_label()` → `"mm"` / `"in"` / `"mil"`,
  and `length_step()` / `feed_step()` → the HTML number-input step.
- `Length::parse_in_system(input, sys)` / `FeedRate::parse_in_system(input, sys)`
  — parse a user string with a system-derived default unit for bare numbers
  (replaces `parse_*_with_preference`; wraps the existing `from_string`).

Everything above is pure formatting/parsing over the public accessors — no I/O,
no app state — consistent with the crate's charter.

---

## 5. Delete the UI duplication

`ui/unit_format.rs` collapses; each function maps to the units API:

| `unit_format.rs`                         | Replacement (units crate)                    |
|------------------------------------------|----------------------------------------------|
| `round_to_step`, `format_trimmed`, precision tables | `round_to_step` + `format_number` (single core) |
| `format_length_display` / `format_feed_display` | `Length/FeedRate::display_string(sys)`   |
| `format_*_edit_display`                  | `*::edit_string(sys)`                        |
| `format_angle_display` / `format_rotational_speed_display` | `display_string` (system-agnostic) |
| `length_unit_label` / `feed_unit_label`  | `UserUnitSystem::length_label/feed_label`    |
| `length_input_step` / `feed_input_step`  | `UserUnitSystem::length_step/feed_step`      |
| `parse_length_with_preference` / `parse_feed_with_preference` | `*::parse_in_system(input, sys)` |
| `parse_angle` / `parse_rotational_speed` | existing `Angle/RotationalSpeed::from_string` |
| `*_value_from_mm` / `mm_from_display_*` (few/no callers) | drop, or `user_value` + `from_*` |

Call sites (bindings.rs, screens/stock.rs, screens/job/sidebar.rs, shell.rs) then
call `units` directly. `ui/unit_format.rs` is removed rather than kept as a shim —
the indirection has no value once the logic lives in one place.

`app_shell::UnitSystem` is removed; `runtime` (`AppCtx.unit_system`,
`SetUnitSystem`, `load_persisted_unit_system`) and `ui` switch to
`units::UserUnitSystem`. Persistence keeps its 3-way string mapping
(`"metric"`/`"imperial"`/`"mil"`).

---

## 6. The GCode engine's `fmt` on top

The engine's emit-time formatter (GTL §4, engine §4/§8) becomes a thin consumer
of the same core:

- The engine's active output system is **`Metric` or `Imperial` only** — GCode
  emits mm or inch, never mil. (`Mil` is a UI display choice, not a GCode dialect.)
- `Length` / `FeedRate` → `user_value(sys)` then `format_number(.., n)` → a **bare
  number, no unit suffix** (the suffix is implied by the modal `G21`/`G20`).
- `RotationalSpeed`, integers, floats → `format_number` / pass through; strings
  verbatim.

So the UI's on-screen value and the emitted GCode coordinate are rounded by the
exact same `round_to_step` + `format_number`. That single source of truth is the
whole point of doing this before the engine.

---

## 7. Migration steps

1. Extract `format_number` from `format_with_unit`; keep `round_to_step` as the
   shared rounder. (No behavior change.)
2. Add `Mil` to `UserUnitSystem` and the `Mil` arms to the four impls.
3. Add the convenience methods (`display_string`, `edit_string`, labels, steps,
   `parse_in_system`).
4. Migrate UI call sites to `units`; delete `ui/unit_format.rs`; replace
   `app_shell::UnitSystem` with a re-export of `units::UserUnitSystem`.
5. Prune the now-dead bridge (`user_unit_system()`) and the `#[allow(dead_code)]`
   helpers this obsoletes.
6. (During engine phase 1) implement the engine `fmt` on the shared core.

Steps 1–5 are a self-contained refactor with no behavior change (snapshot the
current UI strings first and assert they are byte-identical after). Step 6 lands
with the engine.

---

## 8. Open decisions

1. **Option A (recommended): `Mil` as a third `UserUnitSystem` variant** vs.
   Option B (separate length display unit). A is far less churn and matches the
   existing 3-way toggle; B is more correct but heavier. Recommend A.
2. **Delete `ui/unit_format.rs` entirely (recommended)** vs. keep a thin shim.
   Recommend delete — call `units` directly.
3. **Keep the name `UserUnitSystem`** for the unified enum (vs. renaming to
   `UnitSystem`). Recommend keeping `UserUnitSystem`; it is the units crate's
   established name and reads correctly ("the user's chosen display system").

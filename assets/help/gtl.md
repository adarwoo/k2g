# GCode template syntax

The program lifecycle and motion fields (`initialise`, `rapid_move`,
`peck_drill`, `change_tool`, …) are written in the **GCode Template Language**.
Each field is a small script: most lines are the GCode you want, and the
engine fills in coordinates, feeds and speeds for you — already converted to the
machine's active unit.

## The one rule

A line that **starts with a backtick** `` ` `` is emitted as GCode. Every other
line is script (loops, conditions, variables).

```
`G0 X{x} Y{y}
```

emits `G0 X3.2 Y7` — the backtick is dropped, and `{x}` / `{y}` are replaced
with the real values.

## Substituting values — `{ ... }`

Put any variable or expression in braces. Because braces mark the boundary, you
don't need spaces between fields:

```
`G0 X{x}Y{y}Z{z_safe}
```

You can compute inside the braces too:

```
`G1 Z{z_bottom - clearance} F{z_feed}
```

## Units are automatic

A CNC coordinate system is *modal* — once the program says `G21` (mm) or `G20`
(inch), every coordinate is in that unit. You never track this yourself:

- Call **`metric()`** once (usually in `initialise`) to work in millimetres, or
  **`imperial()`** for inches. That call emits the matching `G21` / `G20` **and**
  tells the engine how to format every value from then on.
- A length like `{z_safe}` then prints as mm or inches automatically. Feeds
  print as mm/min or in/min. Spindle speeds (rpm) are the same either way.

Need a specific unit regardless of mode? Use an explicit accessor, which gives a
plain number: `{z_safe.mm}`, `{z_safe.inch}`, `{z_feed.mm_per_min}`.

## Loops and conditions

Everything outside a backtick line is an ordinary script. Values keep their unit
type, so comparisons and maths are unit-correct (`z > z_bottom`, `z - peck`).

A manual peck-drill cycle for a controller without a canned `G83`:

```
`G0 X{x} Y{y}
`G0 Z{z_retract}
let z = z_retract;
while z > z_bottom {
    z = max(z - peck, z_bottom);
    `G1 Z{z} F{z_feed}
    `G0 Z{z_retract}
}
```

Branching — note each emitted line is on **its own line** (a backtick is only
recognised at the start of a line, never in the middle):

```
if has_positioning_pins {
    `G56
} else {
    `G54
}
```

## Comments

- Script comments use `//` — they are not emitted.
- Anything on a backtick line is emitted verbatim, so normal GCode comments
  work there: `` `(drill first hole) ``.

## Quick reference

| You write            | You get                                             |
|----------------------|-----------------------------------------------------|
| `` `G0 X{x} Y{y} ``  | a GCode line with values substituted                |
| `{ expr }`           | evaluate `expr`, convert to the active unit, insert |
| `{{` / `}}`          | a literal `{` / `}`                                 |
| `metric()`           | switch to mm, emit `G21`                             |
| `imperial()`         | switch to inch, emit `G20`                           |
| `{ v.mm }`           | force millimetres (plain number, no conversion)     |
| `//`                 | script comment (not emitted)                        |

## Values available

Each field is given the variables relevant to that operation (for a drilling
primitive: `x`, `y`, `z_bottom`, `z_retract`, `peck`, `z_feed`, `rpm`, …), plus
any custom attributes defined on the CNC profile. Lengths, feeds and speeds are
typed, so they format and combine correctly.

# GCode template syntax

The program lifecycle and motion fields (`initialise`, `rapid_move`, `drill`,
`change_tool`, …) are written in the **GCode Template Language**.
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

## Continuing a line — the closing backtick

A backtick at the **end** of the line too means "don't break the line here", so
whatever comes next carries on where this left off. The `line_number` field is
the clearest case — its whole job is to put something in front of a line it does
not otherwise touch:

```
`N{line * 10} `
```

emits `N10 ` and stops. No line break, so the line being numbered follows it and
you get `N10 G0 X3.2 Y7`.

Notice the space before the closing backtick. That is the whole reason the mark
goes at the *end*: the space sits between two backticks, so you can see it and
your editor cannot quietly trim it off the end of the line. Whatever you want
between the pieces — a space, a colon, nothing at all — goes inside.

You can use it anywhere you want to build a line from more than one piece. An
optional prefix is the safe shape, because the line is still finished by an
ordinary emit:

```
if z_bottom < z_retract {
    `(plunging) `
}
`G1 Z{z_bottom} F{z_feedrate}
```

**Take care to finish the line.** A line left open runs straight into whatever is
emitted next, wherever that comes from — so put the closing backtick on the
*optional* pieces, and let an ordinary emit line end the result.

Two details worth knowing:

- A backtick **on its own** is still an empty emitted line, as before — one
  backtick is a start mark with nothing after it, not a start *and* an end.
- Two backticks `` `` `` emit nothing at all.

## Substituting values — `{ ... }`

Put any variable or expression in braces. Because braces mark the boundary, you
don't need spaces between fields:

```
`G0 X{x}Y{y}Z{z_safe}
```

You can compute inside the braces too:

```
`G1 Z{z_bottom + z_retract} F{z_feedrate}
```

## Units are automatic

A CNC coordinate system is *modal* — once the program says `G21` (mm) or `G20`
(inch), every coordinate is in that unit. You never track this yourself:

- Call **`metric()`** once (usually in `initialise`) to work in millimetres, or
  **`imperial()`** for inches. That call emits your machine's word for it **and**
  tells the engine how to format every value from then on — one call, so the two
  can never disagree.
- What it emits is yours to set, in the **`set_unit`** field: usually
  `` `{if metric { "G21" } else { "G20" }} ``, and empty for a machine that has no
  unit command at all.
- A length like `{z_safe}` then prints as mm or inches automatically. Feeds
  print as mm/min or in/min. Spindle speeds (rpm) are the same either way.

Need a specific unit regardless of mode? Use an explicit accessor, which gives a
plain number: `{z_safe.mm}`, `{z_safe.inch}`, `{z_feedrate.mm_per_min}`.

## Comparing values with units

Values keep their unit type, so comparisons and maths are unit-correct: `10mm` and
`1cm` are equal, and `z_retract - z_bottom` is still a length that prints in the
active unit. You also get `max`, `min`, `abs` and `clamp`.

What you **cannot** do is compare a measurement with a bare number:

```
if z > 5          // error — five what?
if z > z_bottom   // fine
if z.mm > 5       // fine — you named the unit yourself
```

That is deliberate. A silent answer here would be worse than an error: a loop like
`while z > z_bottom` whose bound was accidentally a plain number would simply never
run, and the program would come out with no cutting moves in it.

## Loops and conditions

Everything outside a backtick line is an ordinary script.

A manual peck-drill cycle for a controller without a canned `G83`, in three bites:

```
`G0 X{x} Y{y}
`G0 Z{z_retract}
let z = z_retract;
let step = (z_retract - z_bottom) / 3;
while z > z_bottom {
    z = max(z - step, z_bottom);
    `G1 Z{z} F{z_feedrate}
    `G0 Z{z_retract}
}
```

Every name in there is one the `drill` field is given — see **Values available**
below. `max` is what lands the last bite exactly on `z_bottom` instead of past it.

Branching — note each emitted line is on **its own line** (a backtick is only
recognised at the start of a line, never in the middle). An arc, choosing its
direction from the `clockwise` flag the field is given:

```
if clockwise {
    `G2 X{x} Y{y} I{i} J{j} F{xy_feedrate}
} else {
    `G3 X{x} Y{y} I{i} J{j} F{xy_feedrate}
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
| `` `N{line} ` ``     | the same, with **no line break** — the next emit continues it |
| `` ` ``              | one empty line                                      |
| `` `` ``             | nothing                                             |
| `{ expr }`           | evaluate `expr`, convert to the active unit, insert |
| `{{` / `}}`          | a literal `{` / `}`                                 |
| `metric()`           | switch to mm, emit the `set_unit` field              |
| `imperial()`         | switch to inch, emit the `set_unit` field            |
| `{ v.mm }`           | force millimetres (plain number, no conversion)     |
| `{ v.inch }`         | the same in inches; also `.cm`, `.mil`, `.mm_per_min`, `.rpm`, `.degrees` |
| `max(a, b)`          | also `min`, `abs`, `clamp(v, lo, hi)` — on measurements or numbers |
| `//`                 | script comment (not emitted)                        |

## Values available

Each field is given the variables relevant to that operation, and **only** those —
`drill` gets `x`, `y`, `z_bottom`, `z_retract` and `z_feedrate`; `cut_arc` gets a
centre offset and a direction; `line_number` gets the line's number and its text.
A name the field is not given is an error, not an empty value, so a typo is caught
in the preview rather than in the program.

The panel beside the editor lists exactly what the field you are editing has, with
each one's type. Lengths, feeds and speeds are typed, so they format and combine
correctly.

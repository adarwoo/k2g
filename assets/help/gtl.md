# GCode template syntax

The program lifecycle and motion fields (`program_begin`, `move_rapid`, `drill`,
`tool_change`, …) are written in the **GCode Template Language**.
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
whatever comes next carries on where this left off. The `line_format` field is
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

- Call **`metric()`** once (usually in `program_begin`) to work in millimetres, or
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

## Loops are bounded

A template is a script, so a loop that never ends would otherwise hang the application —
and the editor runs your template on **every keystroke**, while it is still half-written.

So a template is stopped after **200,000 operations** and reported as:

```
drill:3: did not finish — stopped after 200000 operations. A loop is most
likely never ending: check that its condition can become false …
```

Almost always the loop body has stopped changing the value the condition tests:

```
let z = z_retract;
while z > z_bottom {
    `G1 Z{z} F{z_feedrate}
    z = max(z - peck, z_bottom);   // <- forget this line and it never ends
}
```

The budget is per render, not per program, so a board of thousands of holes is unaffected.
It is also generous: a 2000-pass loop uses about a tenth of it, and no real cycle comes
close.

## Three kinds of primitive

The editor shows a badge beside each primitive's name. It matters, because two of the
three do nothing on their own:

- **Generator** — the application emits it, at a fixed point in the program.
  `program_begin`, `tool_change`, `drill`, `cut_linear` and the rest of the machining
  primitives are all generators. Fill one in and it appears.
- **Callable** — **nothing emits it.** It appears only where another template calls it
  by name: `set_origin();`, `metric();`, `comment("Outline pass")`. A perfectly good
  `set_origin` template produces no output at all if your `program_begin` never calls
  it.
- **Filter** — `line_format` alone. It runs over every line of the finished program.

The callables you can use from any template:

| Call | Emits |
|---|---|
| `metric()` / `imperial()` | `set_unit` — **and** sets how every value formats |
| `set_origin()` | `set_origin` — the fixture's work offset, validated |
| `comment("…")` | `comment` — an annotation the machine ignores |
| `message("…")` | `message` — text for the operator, no stop |
| `pause("…")` | `pause` — text, then wait for the operator to resume |

The last three take your text as `{text}` inside their own template, so what a comment
*looks* like stays the profile's business:

```
comment("Outline pass");     ->  ( Outline pass )
```

## Formatting every line

`line_format` is handed each line of the finished program and emits the line that
replaces it. It gets `index` (counting non-blank lines from 0) and `text` (the line as
generated):

```
`N{(index + 1) * 10} {text}          ->  N10 G0 X1 Y2
```

Two things follow from it owning the whole line:

- **You must emit `text`,** or the G-code is thrown away and you get a column of bare
  line numbers.
- **Emitting nothing drops the line** — which is how you remove one deliberately.

Because the template decides *whether* as well as *how*, comments can go unnumbered:

```
if text.starts_with("(") {
    `{text}
} else {
    `N{(index + 1) * 10} {text}
}
```

Leave it empty and the program is emitted exactly as the generators built it.

## Selecting the work origin

The fixture says which of the machine's stored zeros it sits in — its **Machine
Origin Reference**, written the way your controller writes it (`G55`, or
`G54.1 P7` on a MASSO). Emit it by calling **`set_origin()`**, usually in
`program_begin`:

```
`G17 G40 G49 G80 G90
set_origin();
`G0 Z{z_safe}
```

What that call emits is the **`set_origin`** field — and unlike the other
primitives, its job is not only to emit but to **check**. Which offsets exist is a
fact about your controller, so your profile is the only thing that can say. The
field gets `origin_reference` (the raw text the operator typed) and is expected to
`throw` when it is not an offset this machine has:

```
let key = origin_reference;
key.trim();
key.make_upper();
key.replace(" ", "");
if key.is_empty() {
    throw "the fixture has no origin reference. Set it to one of: G54 G55 G56 G57 G58 G59.";
}
let valid = [];
for n in 54..=59 { valid.push("G" + n); }
if !valid.contains(key) {
    throw "'" + origin_reference + "' is not a valid origin reference for this machine.";
}
`{key}
```

A `throw` here refuses the whole program — nothing is written. That is deliberate,
and it is worth being strict about: an offset the controller does not have leaves
the job running against **whatever origin happens to be active**, which cuts the
board somewhere you did not intend and reports nothing. Quote
`origin_reference` in the message so the operator sees what they actually typed.

Two things to know:

- `set_origin` is rendered **once, before any G-code exists**, so a rejection is
  reported as "this program cannot be generated" rather than half a file.
- For the same reason, the primitive editor's preview of `program_begin` shows no
  origin line — the preview has no fixture to read. Preview the `set_origin` field
  itself to check it.

`origin_reference` is also in scope directly in `program_begin` and `program_end`, for a
template that needs the raw text. Do not use both it and `set_origin()`, or the
origin is emitted twice.

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
centre offset and a direction; `line_format` gets the line's number and its text.
A name the field is not given is an error, not an empty value, so a typo is caught
in the preview rather than in the program.

The panel beside the editor lists exactly what the field you are editing has, with
each one's type. Lengths, feeds and speeds are typed, so they format and combine
correctly.

## The job's steps, in the header and footer

`program_begin` and `program_end` are also given the whole machining profile: `steps`
is every step in order, and `step_index` says which one this program is — `0` for the
first. So a header can name itself:

```
`(Step {step_index + 1} of {steps.len()}: {steps[step_index].name})
`(Machine: {steps[step_index].cnc_name}, face: {steps[step_index].board_face})
```

Each entry in `steps` is the step exactly as you set it up on the Machining screen, so
its fields are that screen's fields: `name`, `board_face`, `operations`, and the
settings blocks `drill_pth`, `drill_npth`, `route_board` and `mill_board`. Reach into
them with a dot, as deep as you like:

```
`(Finishing pass: {steps[step_index].route_board.finishing})
`(Tabs: {steps[step_index].route_board.outline.retention.count})
```

Measurements inside a step are real measurements, so they convert with `metric()` and
`imperial()` and take `.mm` / `.inch` like any other.

The CNC, fixture and toolset a step uses are stored as identifiers, which are of no
use in a comment — so each one has its name beside it: `cnc_name`, `fixture_name` and
`toolset_name`. An unbound one is empty rather than missing.

Two things to know:

- **A list cannot be printed whole.** `{steps}` is an error; reach a field of one
  entry — `{steps[step_index].name}`. `{steps.len()}` is fine, being a number.
- **Index only what is there.** `steps[step_index + 1]` on the last step is an error.
  A footer naming the next setup should ask first:

```
if step_index + 1 < steps.len() {
    `(Next: {steps[step_index + 1].name})
}
```

You can also walk the whole job with a loop:

```
for step in steps {
    `(- {step.name} on {step.cnc_name})
}
```

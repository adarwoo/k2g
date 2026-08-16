# k2g user manual

k2g turns a PCB open in KiCad into the G-code that drills and cuts it out. This
manual covers the application from the operator's side: what each screen is for, what
every setting means, and the order to do things in.

- Installing, connecting to KiCad, updating and uninstalling: **[install-and-security.md](install-and-security.md)**
- What is stored and what leaves the machine: **[PRIVACY.md](../PRIVACY.md)**
- Writing CNC profile templates: **[GCode template language](../schemas/docs/gcode-template-language.md)**

> [!WARNING]
> **The programs k2g produces drive a spindle.** k2g works from geometry it reads
> from the board and from profiles *you* wrote describing *your* machine, fixture
> and tooling. It cannot see your actual setup and does not know when a profile is
> wrong. Read every program, confirm the work origin, the stock thickness and the
> tool in the spindle, and air-cut anything new. There is no warranty (GPL-3.0
> §§15–16) — the operator owns the outcome.

---

## Contents

1. [How k2g is put together](#1-how-k2g-is-put-together)
2. [The window](#2-the-window)
3. [Quick start — your first board](#3-quick-start--your-first-board)
4. [Stock and catalogs](#4-stock-and-catalogs)
5. [Profiles: the shared rules](#5-profiles-the-shared-rules)
6. [CNC profiles](#6-cnc-profiles)
7. [Fixture profiles](#7-fixture-profiles)
8. [Toolset profiles](#8-toolset-profiles)
9. [Machining profiles](#9-machining-profiles)
10. [The Job screen](#10-the-job-screen)
11. [Generating and saving the program](#11-generating-and-saving-the-program)
12. [Two-sided work](#12-two-sided-work)
13. [Settings](#13-settings)
14. [Logs](#14-logs)
15. [Troubleshooting](#15-troubleshooting)
16. [Reference](#16-reference)

---

## 1. How k2g is put together

Five things, each referring to the one below it:

```
Job  ──▶  Machining profile  ──▶  step 1 ──▶ CNC + Fixture + Toolset + operations
(one,          (ordered steps)     step 2 ──▶ CNC + Fixture + Toolset + operations
 live)                             …
                                            └── tools come from Stock ◀── Catalogs
```

| | What it is | Where it is edited |
|---|---|---|
| **Job** | The one live thing being worked on: the board currently loaded from KiCad, the machining profile it runs, and the board's placement angle. There is exactly one — no job library, nothing to open or save. | Job screen sidebar |
| **Machining profile** | An ordered list of **steps**. Each step is one physical setup: its own CNC, fixture, toolset, board face and set of operations. This is the reusable recipe. | Machining screen |
| **CNC profile** | One machine: its spindle range, feed ceilings, ATC slot count, and the G-code templates that decide every word it emits. | CNC screen |
| **Fixture profile** | The physical setup: which of the machine's stored zeros it sits in, which corner of the bed is X0/Y0, backboard thickness, retract and safe heights. | Fixtures screen |
| **Toolset profile** | What is loaded in the rack: T1…Tn, each fixed to a tool, left spare, or disabled — plus the policy for what happens when the job needs more tools than the rack holds. | Toolset screen |
| **Stock** | The tools you actually own. Copies, not references — deleting a catalog never touches stock. | Stock screen |
| **Catalogs** | Read-only supplier libraries you import tools *from*. | Catalog screen |

Three consequences worth knowing up front:

- **A step is one setup.** A second machine, a second fixture, or the board turned
  over means a second *step*, never an alternative inside one.
- **Everything auto-saves.** There is no Save button for configuration. Edits are
  written as you make them.
- **Generation is automatic.** Any change that could affect the program re-runs it,
  and only one run happens at a time — a new change cancels the run in flight.

---

## 2. The window

### Top bar

| Element | What it does |
|---|---|
| **K2G** logo | Opens the About screen. |
| **Board** | The name of the PCB read from KiCad. **↻** re-reads it — press this after changing the layout in KiCad. |
| **Job** | The machining profile the job runs. Says *No machining profile selected* until you pick one, and warns when the profile's step has no operations ticked. |
| **mm / in / mil** | The display unit for the whole application. Changes only how values are shown and how bare numbers you type are interpreted; nothing stored is converted. |
| **Status pill** | Whether there is a program to save (see below). |
| **Save…** | Writes the program(s) to disk. Disabled when there is nothing current to save. |
| **USB button** | Appears *only* when a removable medium is plugged in: saves and ejects in one action. Its appearance is the signal that the stick is ready. |
| **⚙ cog** | Settings. |

The pill reads:

| Pill | Meaning |
|---|---|
| **Program ready** / **N programs ready** | Generation finished; Save is live. |
| **Generating…** | A run is in flight; what is on screen is the previous result. |
| **Not ready** | The job cannot be machined yet. The **Code** tab lists exactly why. |
| **No program** | Ready, but nothing has been generated yet. |
| **N of M steps failed** | Some steps produced a program and some did not. Hover for which. |
| **Generation failed** | The last run errored; see the diagnostics banner and Logs. |

### Navigation rail

**Job** · — · **Machining** · **CNC** · **Fixtures** · **Toolset** · — · **Stock** ·
**Catalog** · — · **Manual** · **Logs** · **About**

The 📌 beside **Job** docks the Job view *beside* the profile and inventory screens,
so Code, Tooling or Rack stay visible while you edit the things that feed them. The
docked view is live and shares its tab selection with the Job screen.

**Manual** is this document, built into the application — so it is readable at the
machine with no browser and nothing online. Its contents list stays beside the text;
the companion documents at the foot of that list open in your browser.

### Status bar and messages

- The bar at the bottom shows the **KiCad connection**: green when KiCad answered
  with a version, red for not connected or not responding.
- A **diagnostics banner** appears above the work area whenever there are errors
  (red) or warnings (orange). One entry states itself; several summarise. *Show
  details* expands every entry.
- Short-lived **toasts** in the corner report actions: saves, ejects, imports,
  KiCad integration changes.

---

## 3. Quick start — your first board

A fresh install has the tool catalogs and the bundled CNC templates, and nothing
else. Building the first job takes about ten minutes; after that, a new board is
"open it, check the plan, save".

1. **Open the board in KiCad**, with the IPC API enabled. If k2g's top bar says *No
   board loaded*, see [Connecting to KiCad](install-and-security.md#connecting-to-kicad)
   — the cog → *KiCad integration* can enable the API and add a **Create GCode**
   button to KiCad's toolbar for you.
2. **Stock → Add tools from catalog.** Pick the drills and routers you actually own.
   Nothing can be planned without tools. (§4)
3. **CNC → Add CNC.** Start from a bundled template if one matches your machine —
   *Genmitsu 3018-Pro*, *Masso G3 (with ATC / manual tool change)*, *Bantam Tools
   Desktop PCB Milling Machine*. Otherwise start from any of them and edit. Check
   the spindle range, the feed ceilings and the ATC slot count. (§6)
4. **Fixtures → Add Fixture.** Backboard thickness, bed origin corner, machine
   origin reference (`G54`…), safe and retract heights. **Get the backboard
   thickness right — it is what keeps the drill out of your bed.** (§7)
5. **Toolset → Add Toolset.** Set the slot count to your rack, then pin the tools
   that live there permanently and leave the rest *Spare*. On a machine with no ATC,
   a one-slot spare toolset is enough. (§8)
6. **Machining → Add Machining.** Bind the CNC, fixture and toolset to the step,
   tick the operations you want (*Drill plated holes*, *Drill non-plated holes*,
   *Cut board outline* is the usual first set), and set the outline options. (§9)
7. **Job → sidebar → Machining profile**: select the profile you just made. The job
   summary fills in and generation starts by itself.
8. **Check the plan.** *Tooling* — is each hole size getting a sensible tool?
   *Machining* — does the 3D toolpath look like your board? *Board* — are all the
   features there?
9. **Read the program** on the *Code* tab.
10. **Save…**, then air-cut it.

---

## 4. Stock and catalogs

### Stock

Stock is the list of tools you own. Every stock tool is an independent copy — even
one added from a catalog — so deleting a catalog, or reimporting a newer version of
it, never disturbs stock.

**The table.** Type, diameter, name, source catalog, preference, ATC slot (only when
the selected machine has an ATC), and status. Above it:

- a **filter box** matching type, name, source, preference or status;
- a **type filter** (All / Drill / Router / V-bit / Engraving);
- a **sort** (latest first, type, size ascending/descending, status, preference,
  source catalog).

**Working with rows.**

| Action | How |
|---|---|
| Open a tool | Double-click the row |
| Select several | Row checkboxes; the header box takes every visible row |
| Delete | Select, then **Delete Selected** — confirmed, and it warns if the tools are used by the current job or referenced by a profile |
| Change availability | The **Status** dropdown in the row itself |

**Availability and preference** are what the planner reads:

- *In stock* / *Out of stock* — an out-of-stock tool is never chosen. This is the
  quickest way to tell k2g "I broke that drill" without deleting it.
- *Preferred* / *Neutral* / *Not preferred* — a tie-break when several tools fit a
  hole equally well.

**Tool detail** (double-click) shows the catalog metadata read-only and lets you edit
the practical values: custom name, diameter, tip geometry, feed rate, spindle speed,
availability and preference. Any field you change from its catalog original is
flagged, with a revert control beside it; **Revert to catalog** resets the whole
tool. **Clone Tool** copies it — useful for "the same drill, but the one that has
done 4,000 hits".

Feeds and speeds you leave empty are derived by k2g and clamped to the machine's real
spindle and feed range.

### Adding from a catalog

**Add tools from catalog** opens the picker: catalogs, then sections, then tools.

- Click a tool to select it.
- **Shift-click** takes the whole run between it and your last plain click, within
  one section.
- A section's header checkbox takes the whole section.
- **Add Selected (n)** copies every catalog field into new stock entries.

### The Catalog screen

Browse what is inside a catalog without adding anything, and manage the list:

- **Import catalog** reads a catalog YAML file.
- Built-in catalogs (*generic*, *kyocera*, *unionfab*) are marked **Protected** and
  cannot be deleted; imported ones can.
- Selecting a catalog lists its tools read-only, with SKU, point angle, feed and
  speed.

---

## 5. Profiles: the shared rules

CNC, fixture, toolset and machining profiles all behave the same way.

**The toolbar** at the top of each screen: a dropdown of existing profiles, then
**Clone**, **Export**, **Delete**, and — always available — **Add \<type\>** and
**Import**.

- **Add** and **Clone** open a small dialog asking for the name. Enter accepts,
  Escape cancels. The CNC dialog also offers a **Template** to start from.
- **Export / Import** are YAML files, so a profile can be shared or version-
  controlled. Treat an imported profile like a script you were sent: a CNC profile
  contains code that k2g runs to produce the program your spindle follows.
- **Delete** is refused while something still points at the profile — a machining
  profile referencing a CNC, fixture or toolset blocks that profile's deletion, and
  the message names the profiles involved. Clear the reference first. Nothing
  references a machining profile, so it always deletes.
- Every profile has a permanent hidden identity, so renaming one never breaks a
  reference and cloning always produces a genuinely new profile.

**Editing fields.**

- Type into a field and press **Enter** to commit, or simply move focus away —
  leaving a field also commits it. **Escape** reverts to the last committed value.
- Measurements are shown in your chosen unit, with the stored value in brackets when
  it differs (`0.61mm [0.024in]`). Typing a bare number means the current display
  unit; typing `3/64in` or `0.5mm` overrides it. Fractions are accepted.
- A field the profile cannot do without shows as invalid until it is filled in.

---

## 6. CNC profiles

One profile per machine. **Every G-code word k2g emits comes from this profile** —
none is built into the application — so an unusual controller is a profile, not a
patch.

### Machine

| Field | Meaning |
|---|---|
| **Output file extension** | What a saved program for this machine is called (`nc`, `ngc`, `drl`…), without the dot. The templates decide the *format*, so the extension belongs with them. |
| **ATC slot count** | Number of automatic tool-changer pockets. **0 means no ATC** — tool changes become operator prompts, and the Job screen's *Rack* tab disappears for steps on this machine. |
| **Spindle min / max rpm** | The machine's real range. Tool speeds are clamped into it. |
| **Max feed XY / Z** | The machine's real feed ceilings, from its specification — not a preference. A tool rated faster than the machine can feed is run at a proportionally *lower spindle speed*, so its chip load is preserved rather than silently ruined. Z is usually the slower axis and binds every drilling plunge. |
| **Axis scaling X / Y** | Multipliers for a machine with a known dimensional error. 1.0 is no scaling. |
| **Repeatable home** | True if the machine homes to a repeatable machine-coordinate origin. When true the work origin can be taught once and reused; when false the operator re-establishes work zero every job. |
| **Tool length measurement** | `auto_setter` — a probe measures every tool at M06 and the program needs nothing extra. `manual` — the operator re-zeros Z on each change, and the `tool_measure` template is emitted after each tool change. |

### Primitives (the G-code templates)

Below the machine fields, the templates, grouped as **Program**, **Tools**,
**Motion**, **Drilling**, **Operator** and **Formatting**. Click **Edit…** on any of
them to open the template editor, which shows the variables that primitive receives,
validates as you type, and previews the rendered output against sample values. The
**ⓘ Template syntax** button opens the full language reference.

Each primitive carries a badge saying **how it is used**, and this is the thing to
read first:

| Kind | Meaning |
|---|---|
| **generator** | k2g emits it at a defined point in the program. Fill it in and it appears. |
| **callable** | *Nothing* emits it — another template calls it by name (`set_origin();`, `metric();`, `comment("…")`, `pause("…")`). A filled-in callable that nobody calls produces no output at all. |
| **filter** | Applied to every line of the finished program (`line_format`). |

The essentials:

- `program_begin` / `program_end` — header and footer, once per step. The header is
  conventionally where `metric()` and `set_origin()` are called; neither happens
  unless a template calls it.
- `tool_change`, `tool_measure`, `spindle_start`, `spindle_stop`.
- `move_rapid`, `cut_linear` (**must emit a feed word**), `cut_plunge`, `cut_arc`.
  Leaving `cut_arc` blank does not lose the geometry — arcs are cut as short straight
  moves to the profile's curve tolerance instead.
- `drill` — one hole, with `index`/`count` over the block's run of holes so a profile
  can open a modal cycle on the first hole and cancel it on the last.
- `set_origin` — **this is what validates the fixture's machine origin reference.**
  It is expected to refuse (`throw`) when the controller has no such offset, because
  an offset the machine lacks leaves the job running against whatever origin happens
  to be active. The shipped default accepts G54–G59; a MASSO's extended `G54.1 P…`
  offsets need a profile that says so.
- `line_format` — rewrites *every* line, emitting the whole replacement. This is
  where line numbering lives (`` `N{(index + 1) * 10} {text} ``). Empty means the
  program is emitted unchanged.

---

## 7. Fixture profiles

How the board is held, and where zero is.

| Field | Meaning |
|---|---|
| **Board holding method** | Free text — vacuum, clamps, tape. Descriptive. |
| **Bed origin corner** — *X zero edge* (left / right), *Y zero edge* (near / far) | Which corner of the **bed** the board is registered into, and therefore where X0/Y0 lands. These are the bed's own directions as you stand at the machine: `near` is your side. The axes keep their machine directions, so this moves the zero — it does not mirror anything. With `x0: right`, the board sits in negative X, which is exactly right for a right-hand stop. |
| **Machine Origin Reference** | Which of the machine's *stored* zeros this fixture is set up in, in the controller's own words: `G54`, `G55`, `G54.1 P7`. Validated by the CNC profile's `set_origin` — a value that machine does not have refuses generation rather than cutting in the wrong place. Only meaningful on a machine that homes repeatably. |
| **Board flip axis** | Which axis the board turns about for a back-face step. `y` turns it left-to-right like a page (the near edge stays near); `x` tumbles it near-to-far. It follows from where your registration pins physically are. **Getting it wrong mirrors the board.** |
| **Backboard thickness** | The martyr/exit board under the PCB. Bounds how far the tool may travel past the underside. **Keep this accurate or drilling can reach the bed.** |
| **Bed clearance** | Minimum clearance from any tool tip to the bed. |
| **Breakthrough** | How far past the underside of the board a through-feature cuts, to guarantee it is clean. Kept small, and bounded by the backboard. |
| **Z retract** | The height above the board surface the tool retracts to between features (the drilling R plane). |
| **Z safe** | The travel height for rapid moves across the work — high enough to clear clamps, pins and fixture hardware. |

**Z0 is always the top of the PCB.** There is no "what is zero" toggle anywhere in
k2g; what differs between machines is only how that zero is established, and that is
the CNC profile's *repeatable home* capability. Board thickness comes from the KiCad
stackup, not from here.

---

## 8. Toolset profiles

The rack: slots T1…Tn.

- **Slot count** — 1 to 64. It may legitimately exceed the machine's physical ATC
  capacity (a portable superset); that produces a warning, not a refusal.
- Each slot is **Spare** (available for automatic assignment), **Do not use**
  (broken pocket, reserved position — never allocated), or **a specific tool** from
  in-stock stock, pinned there.

**Generation policy** decides what happens when the job needs more tools than the
rack holds:

| Policy | Behaviour | Use for |
|---|---|---|
| **Fixed toolset** | Generation *fails* if the required tools exceed the usable slots. | Unattended runs, repeatable production. |
| **Allow reload** | The program pauses so the operator reloads the rack, then continues. | A small ATC and a big job. |
| **Allow hybrid** | Falls back from automatic to manual tool changes when the ATC is exhausted. | Hybrid and low-cost machines. |

On a machine with no ATC, tools are prompted for manually and a single spare slot is
all a toolset needs.

---

## 9. Machining profiles

The recipe. A machining profile is an ordered list of **steps**, each one physical
setup.

A profile with one step shows no step machinery at all — it reads like a plain
settings page. Add a second and the headings, ordinals, reorder arrows and fold
controls appear. **+ Add step** appends one (and folds the previous card, so the new
one lands where you are looking); ↑ ↓ reorder; ✕ removes.

### Each step

| Control | Notes |
|---|---|
| **Name** | Only shown with more than one step. An unnamed step is displayed by what it does — "PTH + NPTH + Pins" — so a folded card still identifies itself. |
| **CNC / Fixture / Toolset profile** | Exactly one each. A step left without one is not runnable, and the planner says which binding is missing rather than guessing at hardware you may not have. |
| **Operations** | The checkboxes below. An operation another step already claims *on the same board face* is disabled, with a tooltip naming the step that holds it — the board has each feature once, so cutting it twice is a fault, not a division of labour. Locating pins and engraving are exempt. |
| **Board face to machine** | *Front* (component side) or *Back* (solder side). Absent on a step that drills locating pins — pins are drilled before the board is ever turned over, so the face is settled. |

Each ticked operation adds its own foldable configuration section.

### Drill plated holes (PTH) / Drill non-plated holes (NPTH)

Both take the same settings, under **Holes**:

| Setting | Meaning |
|---|---|
| **Route fallback** (on) | A hole with no suitable drill — too big, or one whose drill point would reach the bed — is milled with a router instead of erroring. Every routed hole is listed on the Tooling tab, so this is visible, not silent. |
| **Drill first** (on) | Prefer drilling over routing whenever a drill fits. |
| **Pilot** (off) | For a hole that is being routed for want of a drill, drill a pilot first with the largest drill bigger than the router bit. |
| **Oversize / Undersize allowance** | How far a stock drill may differ from the requested finished size, as a fraction of the hole capped by an absolute maximum (defaults: 8% capped at 0.10 mm oversize, 6% capped at 0.08 mm undersize). Applies to *every* hole, round and oblong. Plated holes account for the plating thickness read from the board. |
| **Oblong hole strategy** | How a slot longer than it is wide is made: `route`, `drill_ends_then_route` (default), `drill_chain`, or `drill_chain_then_route`. Round holes ignore this. |

### Cut board outline

The board's own boundary, however it is made — the **Cut** setting decides.

| Setting | Meaning |
|---|---|
| **Cut** | `route` and `mill` cut right through, so the board needs retaining. They currently produce the identical toolpath: one contour offset by the cutter's radius. `score` and `vgroove` are **not yet planned** — a step set to either produces no outline pass at all, and says so in the step's notes, while still requiring the kerf router. |
| **V-groove depth** | 50–100% (default 80%). Stored, and not yet read by anything — see **Cut**. |
| **Retention → Mode** | `tabs` leaves short bridges so the part cannot move under the cutter; `none` cuts it free in one pass — correct only when something else holds it (tape, vacuum). |
| **Retention → Count** | How many tabs. They are shared out over the outline's straight sides, longest side first, evenly within each side. |
| **Retention → Width** | How much contour each tab leaves uncut. |
| **Retention → Mouse bites** | Perforate each tab with small drills so it snaps cleanly and files to nothing. Drilled while the board is still whole. How many holes is not a setting — it follows from the tab width and the drill, at twice the drill diameter. |
| **Edge routing kerf** | The width of the channel routed around the board — which *is* the diameter of the cutter that routes it. **Matched exactly**: a step with no router this size fails rather than quietly cutting a narrower channel, because a kerf is a dimension of the finished job. Default 2 mm. |
| **Finishing** | Material left on the wall for a final full-depth pass. 0 means the roughing pass is the finished cut. |

Cut direction is not a setting: the toolpaths pick climb from the geometry, which is
what keeps FR4's top copper from lifting.

### Route interior cutouts

The openings inside the board, cut on their own terms — with a cutter chosen to
*fit* each opening rather than the edge kerf, so a 1.5 mm slot is cut by the 1 mm
router in the rack instead of being reported as impossible. This is the only
operation that cuts an interior opening.

| Setting | Meaning |
|---|---|
| **Hold the slug** (on) | Leave one uncut tab holding the piece of board each cutout removes, so the cutter cannot throw it. Only an opening wider than twice the router diameter leaves a slug at all. |
| **Tab width** | As a fraction of the slug's own perimeter (default 4%), floored at 1 mm. A fraction because the same profile has to cut a 4 mm disc and a 40 mm panel. A tab working out wider than 2 mm is perforated automatically. |
| **Relieve sharp corners** (on) | Drill into each corner a round cutter cannot reach, tangent to both edges so it can never cut past what was drawn. Done in the drill phase while the board is whole; skipped with a note where no suitable drill is in the rack. |

### Drill locating pins

Two holes through the board and on into the backboard, on the fixture's flip line, so
the board can be turned over and land back in the same place. The only setting is the
**pin diameter**, from a fixed list of sizes pins are actually sold in: 2, 2.5, 3,
3.175 (= ⅛″) and 3.2 mm. 3.2 mm is the default — it takes a ⅛″ shank with about 25 µm
of play.

This step must be first and must be on the front face; see §12.

### Engrave copper isolation

Isolation routing: cut a channel around every piece of copper so the nets separate.

The only setting is **isolation width** — an electrical decision, not a tooling one.
Wider is better isolated and slower to cut. The V-bit is chosen to suit and the depth
it needs is worked out from it, so depth is deliberately not asked for. Where the
board is too cramped for the requested width, the pass narrows only across that
stretch and says which nets it narrowed, in the step's notes; it never widens and
never cuts into a neighbour. Outer copper only (F.Cu or B.Cu, whichever the step's
board face names).

---

## 10. The Job screen

Left: the tabbed view. Right: the job configuration sidebar.

### The sidebar

- **Machining profile** — the profile this job runs. Selecting one starts generation.
- **Job summary** — the resolved profile, CNC, fixture, toolset, board face,
  operations and board thickness. A missing reference is shown as a broken reference
  here, which is the quickest way to spot a profile pointing at something deleted.
- **Board orientation angle** — −180 to +180 degrees, for fitting the board on the
  bed. This is live job data, not a profile setting: the same board on a different
  bed may need a different angle.

Outline settings — tabs, mouse bites, kerf — are **not** here. They belong to the
machining profile's route step, because they are part of the recipe.

### Step chips

With more than one step, a row of chips under the tabs selects which step every view
shows. A chip whose program failed is marked, and hovering it gives the reason — so a
failure is discoverable without opening Code.

### Board

The PCB as read from KiCad: edge cuts, drilled holes, routed slots and copper.

- **Zoom**: the **+** / **−** buttons, or the wheel. **Reset** returns to fit.
- **Pan**: drag.
- **Open PCB documents**: when KiCad has several boards open, which one a refresh
  reads.
- The counter strip reports drilled holes, routed slots, edges and copper features.

The legend is also the layer control — **click a legend row to bring that layer to
the front**, click again to restore the drawing order. Reading the drawing:

- A round hole is drawn as a **symbol** keyed to its size class, true to size, so a
  0.3 mm via reads smaller than the 0.8 mm pad beside it.
- Anything a **router** makes — an oblong slot, the outline — is drawn as a
  **hatched band**: the hatch is the tool path and its width is the feature's width.
- Features the *selected step* does not make are ghosted: still drawn, because a
  board is unreadable without its own geometry, but plainly not this step's work.

### Machining

The operation plan, and the 3D toolpath render above it.

- **3D**: orbit with the mouse. Z is up, machine convention. Rapids are thin and
  muted; cutting moves are solid and coloured per tool. The **Tools** legend beside
  it switches individual tools off, which is how you read a dense plan.
- Below: the step's summary (operation count, tool blocks, travel), then one table
  per tool block listing the atomic operations in the order the planner chose, with
  coordinates. Long blocks are capped at 40 rows with a "+N more" line.
- Step **notes** appear as warnings at the bottom — this is where "engraving narrowed
  between these two nets" or "a corner was left unrelieved" is reported.
- While isolation contours are being worked out, an overlay says so; the rest of the
  step stays visible and usable.

### Code

The generated program for the selected step, syntax-highlighted, with line numbers.
It is **read-only** — every generation replaces it wholesale, and this view exists to
read and verify it. The strip at the bottom gives line and character counts, the
board thickness, and (in a multi-step job) which CNC this program is for.

When there is no program, this view says *why*: a failed step's own message, or the
list of readiness reasons blocking generation.

### Tooling

The tooling plan for the selected step — the answer to "what makes what".

- **Tool selection**: the resolved rack, slot by slot.
- **Machining requirements**: each requirement, how many of them, the tool assigned,
  its diameter (**Ø**) and how far that is from the requested size (**Δ**). A
  requirement satisfied by milling instead of drilling carries a **routed** badge.
- **Warnings** below — pilot holes dropped for rack capacity, corners skipped, and
  similar.
- A step with no solution shows **No tooling solution** and the reasons: which hole,
  what size, and what the nearest stock tools are.

### Rack

Only present when the selected step's machine has an ATC. Each slot, the tool in it,
and what you must do about it before running the step:

| | |
|---|---|
| **Fixed** 📌 | Pinned by the toolset profile. |
| **Load** | Must be swapped in before this step. |
| **Kept** | Carried over from an earlier step — leave it alone. |
| **Empty** | Nothing in it. |

With a single step there is nothing to carry over, so the view collapses to slot and
tool. With several, a line says how many changes stand between the previous step and
this one.

---

## 11. Generating and saving the program

### When generation runs

Automatically, whenever something that could change the output changes: the board,
the machining profile, a bound CNC/fixture/toolset, stock, or the job's own settings.
One run at a time — a change arriving mid-run cancels it and starts again, so what
you see is always the result of the newest inputs. Typical boards regenerate in a
second or two.

### Why it might not run

The pill says **Not ready** and the Code tab lists the reasons. The common ones:

| Reason | What to do |
|---|---|
| PCB data not loaded | Open a board in KiCad and press ↻. |
| No machining profile selected | Pick one in the Job sidebar. |
| Step *n* has no CNC / fixture / toolset | Bind it on the Machining screen. |
| Referenced CNC / fixture / toolset profile is missing | The profile points at something deleted. Re-bind it. |
| Open contours detected · Floating island detected · Stitching errors | The board's edge-cut layer does not close into a usable outline. Fix it in KiCad. |
| Locating-pin faults | See §12. |
| An operation is claimed by two steps on one face | Untick it in one of them. |
| Blocking runtime errors present | Open the diagnostics banner. |

### Saving

**Save…** in the top bar, from any screen.

- **A single-step job** opens an ordinary save dialog, pre-named after the board
  (`.nc` by default — change the extension in the dialog if your controller wants
  another).
- **A multi-step job** first shows the save plan: one row per step that produced a
  program, with its name, machine and line count. Each row is pre-named after the
  board and the step, with that step's own CNC file extension. Tick which to write,
  adjust the file names, then choose **one folder** for all of them. Names are checked before
  the folder prompt — blank names, path separators and clashes (including
  case-only clashes, which collide on Windows) are refused there rather than halfway
  through writing. Existing files are confirmed once, for the batch.
- **The USB button** does the same thing starting on the removable medium, then
  **ejects it** — but only if the whole batch was written, so a partial write leaves
  the stick mounted to retry. Which drive is ejected is decided by where the files
  actually landed, not by which button you pressed.

Every write is recorded in the security log (file name and byte count; the directory
is redacted, and the program text never goes near the record).

---

## 12. Two-sided work

Machining the back face works, and there are three rules k2g enforces because
breaking any of them scraps the board:

1. **The locating-pins step comes first.** Pins are drilled into a board still in its
   original setup. Drilled after something else has cut the board, they register it
   against holes made after the fact — which registers nothing.
2. **Pins are drilled on the front.** Drilling registration from the back means the
   board was already turned over, before it had anything to be turned over against.
   The editor hides the face control on a pins step for exactly this reason.
3. **A face change needs pins before it.** Two steps on opposite faces with no
   locating-pins step between the start and the change is a board lifted off the
   fixture and put back by eye — and the program that follows is exact to a micron
   and lands wherever you happened to put it.

The editor prevents the first two; the readiness gate catches all three, including in
a hand-edited or imported profile.

Set the fixture's **board flip axis** to match where your pins physically are: `y`
for pins on a left-to-right line (turn it like a page), `x` for pins on a near-to-far
line (tumble it). A back-face program opens by asking the operator to confirm the
board is back-face up.

---

## 13. Settings

The **⚙ cog** in the top bar. Everything here takes effect immediately.

**Appearance** — Light or Dark. k2g deliberately does *not* follow the desktop's own
light/dark schedule: a job runs for hours, and a window that repaints itself at dusk
mid-cut is a surprise rather than a convenience.

**KiCad integration** — one card per detected KiCad version, each stating exactly
which file it will touch before it touches it:

- **Enable the KiCad API** sets `api.enable_server` in that version's
  `kicad_common.json`, writing a `.k2g-backup` copy first. **Refused while KiCad is
  running**, because KiCad rewrites that file on exit and would discard the change.
- **Register with KiCad** installs a small plugin so KiCad shows a **Create GCode**
  button on the PCB editor toolbar; pressing it opens k2g with that board already
  loaded and connected — no discovery, no ambiguity about which KiCad is meant.
  Restart KiCad afterwards. **Unregister** removes it. A badge reads *plugin stale*
  when the registration points at a different k2g build; re-register to fix it.

**Updates** — a once-a-day check against GitHub's releases API, and the only network
request k2g ever makes. It shows when it last checked, and any version you skipped or
reminder you postponed, each with an undo. Nothing downloads or installs without an
explicit click, and every installer is signature-checked before it runs.

**Security recording** — appends a line to `logs/security.jsonl` when something
security-relevant happens: update checks and installs, changes to these switches,
KiCad plugin registration and API edits, rejected configuration files, G-code written
to disk, and resets. Nothing is transmitted; home-directory paths are shortened to
`~`. Switching it off reveals a button to delete what was kept.

**Data and reset** — names the one directory everything lives in, then:

- **Reset settings** — deletes your settings, profiles, stock and job and restores
  the shipped defaults immediately. Catalogs and the security log are kept, and the
  board stays loaded.
- **Delete all data** — removes the whole directory and closes k2g. The next start
  behaves like a fresh install. This cannot be undone.

---

## 14. Logs

Two records that look alike and are not the same thing.

- **Diagnostics** — a live tail of the application's own log output, held in memory
  and gone when k2g exits. Filter by All / Info / Warnings / Errors, **Refresh**, or
  **Clear**. This is for working out what the run in front of you is doing. Start k2g
  with `RUST_LOG=debug` for more of it.
- **Security** — the persisted record described above, surviving across runs, with
  **Export…** to write it out for someone else to read. It says so when recording is
  currently switched off, and still shows what was captured before.

---

## 15. Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| Top bar says *No board loaded* | KiCad's IPC API is off, KiCad is not running, or no board is open | Cog → KiCad integration → *Enable the KiCad API* (with KiCad closed), then restart KiCad and press ↻ |
| Status bar KiCad line is red | Same, or KiCad stopped responding | Press ↻; if k2g had already cached the board, generation carries on regardless |
| Board loaded but *Not ready* | A readiness reason | Open the **Code** tab — it lists them |
| *No tooling solution* | Nothing in stock fits a feature, or everything that does is out of stock | Check the reason's suggested sizes; add the tool, mark it in stock, or widen the over/undersize allowance |
| A step fails with a missing router of the exact kerf | The edge kerf is matched exactly by design | Add that router to stock, or change the step's kerf to a size you own |
| Everything is planned with the smallest router | Older behaviour; the kerf now decides the edge cutter | Set the step's **Edge routing kerf** |
| Generation refuses over the origin reference | The fixture names an offset this controller does not have | Correct the fixture's **Machine Origin Reference**, or teach the CNC profile's `set_origin` about the machine's extended offsets |
| The program has no line numbers | Line numbering is a template, not a setting | Fill in the CNC profile's `line_format` |
| A primitive is filled in but nothing appears | It is a **callable** — nothing emits it on its own | Call it from `program_begin` or wherever it belongs |
| Board outline errors — open contours, floating island | The edge-cut geometry does not close | Fix it in KiCad, then ↻ |
| Multiple KiCads open and the wrong board loads | KiCad serves one fixed API socket and instances are not individually addressable | Launch k2g from the board you want, via KiCad's **Create GCode** button |
| The USB save button is missing | It appears only when a removable medium is mounted | Plug the stick in |
| Deleting a profile is refused | A machining profile still references it | Re-bind or delete that machining profile first — the message names it |

---

## 16. Reference

### Mouse and keyboard

| Where | Gesture |
|---|---|
| Board view | Wheel = zoom · drag = pan · **+** / **−** / **Reset** buttons |
| 3D view | Drag = orbit · wheel = zoom · legend checkboxes hide tools |
| Board legend | Click a row to raise that layer; click again to restore |
| Catalog picker | Click = select · Shift-click = the run since the last plain click · section header box = whole section |
| Stock table | Double-click a row = tool detail · checkboxes = multi-select |
| Any editable field | **Enter** or leaving the field commits · **Escape** reverts |
| Dialogs | **Enter** accepts · **Escape** cancels |

### Where your data lives

| Platform | Directory |
|---|---|
| Windows | `%APPDATA%\k2g` |
| macOS | `~/Library/Application Support/k2g` |
| Linux | `$XDG_CONFIG_HOME/k2g`, else `~/.config/k2g` |

Holding `configs/` (settings, profiles, stock, job), `catalogs/` and `logs/`. Back up
that directory and you have backed up everything k2g knows. The one thing outside it
is the KiCad plugin registration, which lives in KiCad's own folders and is removed
from the KiCad integration card.

### Glossary

| Term | Meaning |
|---|---|
| **Breakthrough** | How far a tool passes below the board's underside to guarantee a clean through-cut. |
| **Kerf** | The width of the channel a cutter removes — for the board edge, the cutter's own diameter. |
| **Mouse bite** | A tab perforated with small drills so it snaps cleanly and files to nothing. |
| **NPTH / PTH** | Non-plated / plated through-hole. Plated holes are drilled oversize to account for the plating. |
| **Rack** | The set of tool positions T1…Tn a step loads. |
| **Retention** | What holds a through-cut part in place until it is broken out: tabs, or nothing. |
| **Slug** | The piece of board an interior cutout removes. |
| **Step** | One physical machining setup: one CNC, one fixture, one toolset, one board face. |
| **Tab** | A short bridge of uncut material holding a part to the surrounding board. |

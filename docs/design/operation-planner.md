# Operation Planner — decomposition & ordering

Status: **design draft** — the algorithm to build next. Where `gcode-generation.md`
covers *when/how* generation runs and `gcode-engine.md` covers the Coder (dialect
rendering), this document covers the **middle step**: turning a resolved job into an
**ordered list of atomic machining operations** ready to render.

```
Assigner (§8.7)          OperationPlanner (this doc)        Coder (gcode-engine.md)
tools + rack        →    decompose + order                 →   GTL → GCode text
```

Pure function, per machining step:

```
stitched Board  +  JobInstance (this step)  +  ToolAssignment  →  PrimitivePlan
```

- **No I/O, no machine dialect** (that is the Coder), **no tool *selection*** (that is
  the Assigner). The planner *consumes* the tools the Assigner already chose and the
  rack it built, *decomposes* the step's demand into atomic operations that use those
  tools, and *orders* them.
- Runs **once per step** (each step is one setup — its own CNC, fixture, toolset,
  rack). The `PrimitivePlan` is therefore per-step (see §8 for multi-step).

---

## 1. The atomic operation — the unit of the plan

The whole design turns on one abstraction: the **atomic op**. It is what the ordering
stage sorts and what the rendering stage walks.

```
AtomicOp {
    tool:   RackSlot,      // which loaded tool performs it
    entry:  Point,         // where the tool must arrive to begin
    exit:   Point,         // where it leaves  (== entry for a point drill)
    recipe: PrimitiveCall, // GTL primitive + args (how to render it)
    z:      ZProfile,      // z_bottom, z_retract, feed
    // …spindle rpm etc. carried for rendering
}
```

Two consumers, two views:

| Consumer | Needs |
|---|---|
| **Ordering** (§4) | `tool`, `entry`, `exit` — nothing else |
| **Rendering** (§6) | the full op — `recipe`, `z`, feeds, rpm |

> **Invariant — atomicity absorbs iteration.** Anything internally repetitive stays
> *inside a single op's expansion* and is invisible to the op list: a peck cycle, a
> multi-pass route, a contour's tabs / mouse-bites, a route's lead-in/out. The op list
> is **flat**; one op has exactly one `entry`/`exit`. If multi-pass or pecking leaked
> into the list, the TSP would balloon and per-feature precedence (§5) would break.

For a **point drill** `entry == exit`. For a **route** (open path) `entry != exit`;
for a **closed contour** it returns to its start, so `entry == exit` again but the op
still owns a whole path internally.

---

## 2. Inputs the planner correlates

- **Stitched board** — `pcb::StitchResult` (contours + hole list with positions,
  drill_x/drill_y, kind PadPth/PadNpth/Via, plated flag). Cached from acquisition; the
  planner never re-stitches.
- **JobInstance for this step** — the effective (defaults + overrides) step config:
  which `operations` are enabled, `drill_pth`/`drill_npth` `holes` settings (incl. the
  `oblong` strategy and `pilot`), `route_board` edge/finishing, `routing`
  `board_face`, board orientation.
- **ToolAssignment** (`src/gcode/assigner.rs`) — per-hole `tool_id` (+ optional
  `pilot_tool_id`, `strategy`, `z_bottom`/`z_retract`), and the `rack` (slot → tool).
  Feeds/speeds/rpm and geometry (point angle, flute) come from the tool's stock/catalog
  record.

> **Oblong tools are already reserved.** [`runtime/tooling.rs`](../../src/runtime/tooling.rs)
> computes `needs_router = has_route || (has_oblongs && oblong_routes)` from the step's
> `oblong` strategy and reserves a router in the rack next to the drill the Assigner
> picks for the oblong's **minor axis**. So both tools an oblong needs are held; the
> planner's job is only to *use* them — it does not re-derive tooling.

---

## 3. Decomposition — demand → atomic ops

For the step's enabled operations, map each feature to op(s). Every round hole drills
with a single `drill` op (G81 — no peck, decision 4).

| Feature | Ops emitted |
|---|---|
| **Round hole / via** (`drill_pth`/`drill_npth`) | one **drill** op at `(x,y)` (`drill`, G81) |
| **Oblong — `route`** | one **route** op (mill the slot, router) |
| **Oblong — `drill_ends_then_route`** | two **drill** ops (the end centres) **+** one **route** op (mill the web) |
| **Oblong — `drill_chain`** | N overlapping **drill** ops along the major axis |
| **Oblong — `drill_chain_then_route`** | N **drill** ops **+** a cleanup **route** op |
| **Cut board outline** (`route_board`) | **contour** ops for the outline (offset path, tabs/mouse-bites *inside* the op) + one per interior **cutout** |
| **Corner relief** | smallest-drill ops at concave corners the router radius can't reach |
| **Pilot** (routed hole, `pilot` on) | a **drill** op preceding the hole's helical route |

The oblong `major` axis + hole centre come from the board hole (`drill_x`/`drill_y`);
the strategy from the step config; the drill tool from the assignment; the slot router
from the rack's mandatory router.

### 3.1 One face, one claim

Every operation above except `drill_locating_pins` may be claimed by **at most one step
per board face**. They each remove material the board has *once*, so a second step
claiming the same one does not add work — it repeats it, driving a tool back through
holes that are already there, into a board the first step may have released from its
tabs.

The rule is **per face, not per profile**: a face is its own setup with its own
geometry, so milling the front and then the back is two distinct jobs that happen to
share a key.

`drill_locating_pins` is exempt because it registers the board against a *fixture*
rather than cutting a feature of the board: a job that re-fixtures genuinely drills a
second set. Engraving will be exempt for the same reason when it lands — several passes
at different depths, or over different regions, are all legitimately engraving.

Enforced in two places, because profiles arrive by more than one route: the machining
editor disables an operation another step has claimed (naming that step), and
`tooling::duplicate_operations_reason` makes it a readiness no-go for a profile that was
hand-edited or imported. The table of which operations are constrained is
`data::model::operations`, mirroring the `operation_key` enum.

`drill_locating_pins` carries rules of its own — where in the sequence it may appear, and
which face it machines — because it is what makes a two-sided job possible at all. See
§6.1.

> **Routed paths keep KiCad's move types — they are not flattened to G1.** KiCad gives
> an *unordered* set of edge moves (line, arc, bezier). Stitching's job is to **re-order
> them into continuous closed loops** and snap segment endpoints for perfect continuity;
> tessellation is used **only internally** to resolve connectivity and nesting
> (point-in-polygon), never as the output. A contour is therefore an ordered list of
> **typed segments** (line / arc / bezier), and a route op expands to the matching
> primitives — `cut_linear` (G1), `cut_arc` (G2/G3) — so curves stay curves.
> Rationale: one CNC arc is far more accurate and faster than the *n* × G1 chords a
> tessellation emits. **The stitcher now carries this** (§9.6): `pcb::Contour` keeps
> both `points` (tessellation, for topology/containment) and an ordered `segments`
> loop (`Segment::{Line,Arc,Bezier}`, endpoints snapped for continuity). The remaining
> piece is the **segment-wise offset** that turns a contour into a toolpath.

---

## 4. Ordering — the hierarchy (the core of this phase)

**A flat TSP over every op is the wrong model.** A tool change (manual: stop, unload,
load, re-zero; ATC: still seconds of dwell) costs 10²–10³× a rapid move — so
travel-optimising *across* tools optimises the wrong variable. Ordering is
**hierarchical**, which is also what `architecture.md` means by "grouped by project
type, TSP within each":

1. **Phase by operation type, rigidity-decreasing:**
   `(engrave — future) → drill → route`.
   This is a **hard constraint, not a preference.** Routing releases the part (tabs cut,
   perimeter breached), so *all* drilling must finish while the board is fully attached
   and flat.
2. **Within a phase, group by tool** — each tool used in **one contiguous block**. This
   is what actually minimises tool changes. The Assigner already shrinks to a minimal
   tool set, so blocks are few.
3. **TSP inside each tool block** — nearest-neighbour + 2-opt (Or-opt) is ample for PCB
   hole counts and, crucially, easy to make **deterministic** (fixed start point, fixed
   tie-breaks). Prefer a small hand-rolled pass over a black-box crate *precisely* for
   determinism (§7).
4. **Order the tool blocks:**
   - **Drilling — smallest → largest diameter.** Small bits are the most fragile and
     want the most rigid (least-drilled) board, and the size progression is
     operator-friendly for manual changes. Treat it as a **policy, not a law** — it is a
     *weak* lever (block count is already minimal; for ATC it barely moves the clock).
   - **Routing — interior before perimeter.** See below.

> **Routing is not "just TSP."** It carries two ordering laws over travel:
> - **Interior cutouts/slots before the outer perimeter** — once the perimeter is
>   breached (even tabbed) the part shifts and interior cuts lose accuracy.
> - **The perimeter is tabbed and cut last**, tabs being uncut bridges that keep the
>   part in the panel.

---

## 5. Precedence falls out of the phase structure

The elegant consequence of phasing by type/tool: **feature-level precedence is
satisfied for free**, with no per-feature sequencing.

- An oblong's **end-drills land in the drill phase** and its **slot-route in the route
  phase** — so "drill the ends before milling the web" holds automatically.
- A routed hole's **pilot is a drill** (drill phase) and its **helical route** is in the
  route phase — so "pilot before route" holds automatically.

So the planner does **not** keep a feature's ops adjacent (which would force a
drill→router tool change *per oblong* — pathological). It scatters them into the right
phases, and the phase order guarantees the dependency. The only precedence needing
explicit care is *within a single tool block* (e.g. `drill_chain` order), which the TSP
handles by position.

---

### 5.1 Feeds and speeds

A tool has **two** rated feeds, not one: `table_feed` is what it cuts at laterally and
`z_feed` what it plunges at. They differ because the moves differ — a straight plunge
engages the tool's weak end-cutting geometry over its full diameter at once, where a
lateral pass engages its flutes. Catalogues state both, and `gcode::feeds` keeps them
apart all the way to the `F` word:

| move | `F` |
|---|---|
| drill cycle (`G81`) — entirely plunge | `z_feed` |
| routing lead-in / plunge | `z_feed` |
| routing lateral cut, arc | `table_feed` |

Both are rated **at the tool's rated spindle speed**, so a spindle clamp scales both by the
same ratio. Each axis ceiling then caps its own feed and nothing else: `max_feed_xy` binds
the lateral rate, `max_feed_z` the plunge. There is no `Motion` discriminant deciding which
ceiling applies to a single feed — each feed has an axis.

When a catalogue states only one feed, the fallback depends on what the tool *is*
(`RatedFeeds::for_tool`): a router's single quoted feed is its lateral one, so the plunge
falls back to a third of it (`PLUNGE_FEED_FRACTION`, the conventional derating for a
straight plunge); a drill does nothing but plunge, so its single quoted feed **is** the
plunge feed and is used unchanged.

### 5.2 The edge kerf chooses the cutter

The board outline and its interior cutouts are cut by **one** router — a step pays one tool
change for the whole outline — and the machining profile's `route_board.kerf` (2 mm by
default) is what chooses it.

**The kerf is the cutter.** A single-pass route removes exactly one cutter width of
material, so a 2 mm kerf is a 2 mm router and it is matched **exactly** (to 1 µm, the
assigner's own precision). A step with no such router in stock **fails**, naming the size,
rather than quietly cutting a narrower channel — the same rule locating pins follow (§6.1)
and for the same reason: a kerf is a dimension of the finished job, and a board cut with a
1.6 mm channel where 2 mm was specified is a different board.

That makes the kerf cutter a **required tool of the step**. It is chosen outside the
assigner, so it joins the routers in `RouterPlan::mandatory_ids` and is reserved a rack slot
like any other tool the step must load.

Being pinned in the toolset breaks a tie between two cutters of the same diameter — it
costs no rack slot — but never overrides the requested size. Before this, the rule was "the
smallest router in stock, or any router pinned in the toolset", and both halves were wrong
for an edge cut: the smallest cutter is the right default for reaching tight *internal*
corners and the slowest possible way to cut an outline, and a pinned 0.8 mm won over
everything.

Slots (oblong holes) are chosen separately and by their own geometry, `pick_slot_router`
taking the widest cutter that fits the slot. A cutout narrower than the kerf is reported as
vanishing under it rather than cut with something else.

### 5.3 Roughing and finishing the wall

`route_board.finishing` (0.1 mm by default) leaves material on the wall for a second pass.
With cutter radius `R` and allowance `f`, one wall becomes two passes:

| pass | cutter centre | sweeps | direction |
|---|---|---|---|
| rough | `R + f` | `f .. 2R+f` | conventional |
| finish | `R` | `0 .. 2R` | climb |

So the finished edge is made by a light cut that never meets full engagement, while the
pass that does meet it — with a fully loaded cutter and the whole depth of the board —
runs conventional, which is the more forgiving of the two there. `f = 0` is one pass
straight to size, and is what most steps do.

**The channel is `kerf + f` wide, not `kerf`.** The board still comes out at its drawn
size — the finishing pass puts the final wall exactly where a single pass would have — and
the extra width is all on the waste side. The two passes overlap only while `f < kerf`; at
or beyond it a ring of material survives between them and the piece never comes free, so
the planner refuses such an allowance and cuts in one pass instead.

**Direction is read off the placed geometry, not configured.** Climb is "material to the
right of travel" (§`gcode::routing`), so it is *clockwise* round the board's boundary —
where the material is inside the loop — and *counter-clockwise* round a cutout wall, where
it is outside. The sign is taken in **machine** space, after the placement has applied any
back-face mirror; taken in board space, a mirrored step would come out conventional on both
passes from the same profile, with nothing on screen to say so.

**Passes are ordered, not travel-optimised against each other.** `plan_outline` takes an
ordered list of passes and TSPs *within* each, so no stretch is ever finished before it is
roughed. Interior cutouts join the roughing pass, which is also what keeps "interior before
perimeter" (§4) true. Each pass tours from where the previous one left the cutter, so the
seam costs no more travel than it must.

**It needs something to hold the piece.** A finishing pass cuts a wall, and by the time it
runs the roughing pass has cut everything except the tabs. With `retention: none` on the
boundary, or `retain_island: false` on a cutout that leaves a slug, there is no wall left to
finish — so the allowance is dropped, the cut is made to size in one pass, and the step says
so in its notes. A cutout too tight to stand the cutter one allowance further in is dropped
the same way: the opening is still cut to its drawn size, only the second pass is missing.

---

## 6. Coordinate placement

Every op's `entry`/`exit`/`z` are **machine coordinates**, but the geometry arrives in
board (design) space. A single **Placement** object owns that mapping, so the transform
lives in one place instead of scattering offset/scaling/rotation math through the
planner and Coder. It is built **once per step** from the JobInstance (fixture +
machining + CNC + board bounds) and is a pure function of them.

**XY** — a composed affine `board → machine`:
- **orientation** — the step's board rotation (`board_orientation`).
- **fixture origin** — where the board sits and which corner is X0/Y0 (the fixture
  `origin` x0/y0 = Left/Right/Near/Far, in the **bed's** directions — `near` is the
  operator's side. Deliberately not the board's `front`/`back`, which name the PCB's two
  faces.)
- **pin margin** — extra room the origin makes for work that is not the board. Today only
  the locating pins (§6.1). Zero for a job without them, which is what keeps such a job's
  output identical to one generated before margins existed. Deliberately **not** the routed
  channel: the cutter centre runs one radius outside the edge, so a routed job has always
  cut into negative coordinates, and folding that in here would move every existing routed
  program.
- **CNC scaling** — per-axis calibration (`machine.scaling.x/y`).
- **board flip** — for a step whose `board_face` is `back`, a final mirror about the
  board's own centre line, on the axis the fixture's `board_flip_axis` names (§6.1).
- *(the machine origin — G54/G55, or a MASSO's G54.1 P7 — is selected in `program_begin` by
  `set_origin()`, from the fixture's `origin_reference`; the Placement produces coordinates
  **relative to** it. The CNC's `set_origin` primitive also validates that reference, and
  refuses to generate when the machine has no such offset.)*

**Z** — the datum is fixed: **Z0 is always the top of the PCB.** There is no
`z0_reference` toggle; what varies by machine is only *how* that zero is established
(a per-job surface touch-off, or a computed/persisted bed reference), which is a
**CNC-profile capability** (`machine.has_repeatable_home`, `machine.tool_length_measurement`),
not a fixture setting.
- **depths** — through-hole `z_bottom = −(board_thickness + breakthrough)` below the
  surface; `board_thickness` comes from the KiCad stackup, `breakthrough` from the fixture.
- **safety** — the plunge is bounded so the tip stays above the bed: the fixture's
  `backboard_thickness` and `bed_clearance` gate it (cut into the martyr board, never the bed).
- **heights** — `z_retract` (R-plane) and `z_safe` (travel) from the fixture, measured
  from the board top.

The Placement exposes the primitives the planner needs — `xy(board_pt) → (mx,my)`,
`z_bottom(through)`, `z_retract()`, `z_safe()` — so the planner emits ready-to-render
coordinates and the Coder only **formats** them (`fmt`), never computes geometry.
Because ops are placed in machine space, the §4 TSP minimises **physical** travel.

> Build this **before** routing gets real: CNC offsets, per-axis scaling, fixture
> stack-up and board rotation compound quickly, and centralising them here keeps that
> complexity out of every op and out of the templates.

### 6.1 Locating pins and the two-sided frame

A board machined on both sides has to be lifted out of the fixture, turned over and put
back **exactly** where it was. Two registration pins are what make that possible: holes
drilled through the board and on into the backboard while it is still in its original
setup, so the turned-over board drops back onto the same two points.

**Where they go** is a fixed rule with one setting — the pin diameter. They sit **on the
fixture's flip mirror line**, centred on the board's bounding box, one pin each side, one
diameter clear of it. A page turn (`board_flip_axis: y`, mirroring X) puts them above and
below; a tumble (`x`) puts them left and right. That placement is the whole scheme: because
each pin lies *on* the mirror line, the flip maps it onto itself, so the holes drilled in
setup 1 are the holes the pins occupy in setup 2. `gcode::pins` owns the geometry and is a
pure function of the placed board rectangle.

**The frame is the job's, not the step's.** The margin (1.5 × diameter on the two pin
sides) and the flip axis are derived once, from the profile's locating-pins step, and given
to every step and to the 3D workpiece. Per step, two programs of one job could be written
against different zeros — and the operator sets up against one.

**Depth** is through the board plus the whole usable space below it
(`backboard_thickness − bed_clearance`). This deliberately bypasses the §2½ Z-feasibility
check, which exists to keep a tool off the bed by rationing exactly that space: a pin that
engages only the board is not registration, because the board pivots on it. Engagement
under 1 mm is a note, not a refusal.

**Tooling** never substitutes. Elsewhere a drill inside the step's oversize/undersize
allowance is a fine stand-in; a registration hole 0.1 mm over lets the board return to
anywhere within 0.1 mm. So: an exact-diameter drill, else a router spiralling the hole to
exact size, else the step fails.

**Ordering rules** (enforced by `tooling::locating_pin_faults` as readiness no-gos, since
they constrain steps against each other and no per-step schema can express them):

1. a locating-pins step is the **first** step — registration drilled after something else
   has cut the board registers nothing;
2. it machines the **front** — pins are what lets the board be turned over, so they
   precede the turn;
3. steps on **different faces** require a pins step before the one that changes face.

**And a prompt.** Every back-face program opens with `pause("Board back face up?")`.
Two symmetric pins of one diameter accept the board unflipped, and turned 180°, exactly as
readily as the right way up; no geometry k2g has can tell those apart, so the question is
the entire guard. A controller with no `pause` primitive emits nothing for it, and the
planner says so.

---

## 7. Handoff to the Coder

The Coder (`gcode-engine.md`) walks the ordered plan. The program-scope job context
(`machine.*`, `cnc.*`, …) and modal unit state are injected once; then:

- **At each tool-block boundary** — emit `tool_change` (slot, rpm, manual message) then
  `spindle_start`.
- **Per op** — `move_rapid` to `entry` at safe Z → the op's primitive
  (`drill`/`peck_drill`, or `cut_linear`/`cut_arc` for routes) → retract.
- **Between ops** — the rapid at safe Z *is* the transit whose XY length the TSP
  minimised (`exit`ᵢ → `entry`ᵢ₊₁).
- `program_begin` / `program_end` bookend the program (already implemented).

The plan slots into the empty **body** section of the current `[header, <body>, footer]`
assembly in `run_generation` — **one program per step** (§9.2).

---

## 8. Determinism

Same `JobInstance` + board + assignment → **identical** `PrimitivePlan` → identical
GCode. This mirrors the Assigner's ethos and is what makes snapshot tests meaningful.
Every heuristic (TSP start, neighbour tie-breaks, block order) must be a total,
deterministic rule — no clock, no hash-map iteration order, no RNG.

---

## 9. Decisions

**Settled (2026-07-24):**

1. **Route orientation — ignored for v1.** `entry`/`exit` stay *distinct* on the op so
   per-route orientation can be optimised later, but the planner does not choose it now.
   Instead, each tool block's **TSP start node is the spindle position after the tool
   change** (the CNC's tool-change / park position) — the virtual origin the first move
   travels from.
2. **Multi-step — one program per step.** Each step renders a **complete, standalone
   program** (its own `program_begin` + body + `program_end`); steps may target different
   CNCs/fixtures. A profile with K steps therefore produces **K programs — exactly one
   per step**. The output model (Code view + export) carries a program per step;
   `PrimitivePlan` is per-step.
3. **Assigner before Planner — kept.** The rack / tool selection (Specification §8.7
   Assigner) runs **first**; the Planner consumes its `ToolAssignment`. The
   "decompose-informs-rack" alternative is **rejected** — it front-loads a large
   decomposition compute for a marginal gain (a slot router differing from the outline
   router), which one-router-for-all-routing already covers.
4. **No peck.** PCBs are ≤ ~4 mm, so the planner **always emits `drill` (G81)** — the
   drill-vs-peck decision is dropped. The `peck_drill` primitive has been **removed
   entirely** from the schema, the CNC templates, and the profile crosswalk; reintroduce
   it only if thick-stock support is ever needed.
5. **Engraving phases first (future).** When added: `engrave → drill → route`, cutting
   copper while the board is intact — the §4 phase list is built to prepend it.
6. **Routing keeps typed segments (arc-preserving), not G1 polylines** (§3). The stitched
   contour is an ordered list of KiCad's own line/arc/bezier moves (endpoints snapped for
   continuity), and routing emits G1/G2/G3 accordingly — one CNC arc beats *n* chords on
   both accuracy and speed. Two pieces of work this implies, flagged so we go in
   eyes-open:
   - **Stitcher output model** — *done.* `pcb::Contour` now carries an ordered
     `Vec<Segment>` (Line / Arc / Bezier, endpoints snapped) alongside the tessellated
     `points`; tessellation stays *internal* to the connectivity + nesting tests. The
     chainer reverses a flipped fragment's segments (and swaps arc/bezier control points).
   - **Segment-wise offset** — *pending.* The toolpath is the contour offset by the tool radius, so
     the offset must be computed **per segment** (line → parallel line, arc → concentric
     arc) with join handling (fillet arc at convex vertices, trim at concave), **not** via
     clipper2's point-polygon offset (which reflattens to G1). This is standard 2D cutter
     compensation — well-understood, but real geometry work, and the main cost of this
     decision.

**Still open:**

7. ~~**Bezier offset.**~~ Settled 2026-07-31. A bezier's offset isn't a bezier — nor
   rational at all, bar the Pythagorean-hodograph family — so there is nothing to fall
   back *to*, and the `cut_bezier` primitive was retired. Every curve, bezier or arc,
   is offset as a polyline by Clipper and then re-fitted with arcs to
   `machine.curve_tolerance` (`src/gcode/arcfit.rs`).
8. **Drill-vs-peck threshold** — moot under (4); revisit only if `peck_drill` is
   reintroduced into the schema.

---

## 10. Testing

Mirror `architecture.md` §OperationPlanner, on constructed fixtures (pure function, no
app context):

- Drilling plan for a known hole set — correct ops, small→large block order.
- Oblong decomposition — one case per `oblong` strategy (op counts + tool usage).
- Contour plan — tabs, mouse-bites, V-groove; interior-before-perimeter ordering.
- TSP reduces total travel vs. naïve input order (and is stable across runs).
- Determinism — identical inputs yield byte-identical plans.
- Precedence — end-drills precede slot-routes; pilots precede helical routes (via the
  phase split, not adjacency).

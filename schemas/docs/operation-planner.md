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
  `cut_depth_strategy` + `multi_pass_max_depth`, `side_to_machine`, board orientation.
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

For the step's enabled operations, map each feature to op(s). The drill-vs-peck choice
is the planner's (a hole whose depth ÷ diameter exceeds a threshold pecks to clear
chips — `drill` = G81 vs `peck_drill` = G83).

| Feature | Ops emitted |
|---|---|
| **Round hole / via** (`drill_pth`/`drill_npth`) | one **drill** op at `(x,y)`; `drill` or `peck_drill` by aspect ratio |
| **Oblong — `route`** | one **route** op (mill the slot, router) |
| **Oblong — `drill_ends_then_route`** | two **drill** ops (the end centres) **+** one **route** op (mill the web) |
| **Oblong — `drill_chain`** | N overlapping **drill** ops along the major axis |
| **Oblong — `drill_chain_then_route`** | N **drill** ops **+** a cleanup **route** op |
| **Route board** (`route_board`/`mill_board`) | **contour** ops for the outline (offset path, tabs/mouse-bites *inside* the op) + one per interior **cutout** |
| **Corner relief** | smallest-drill ops at concave corners the router radius can't reach |
| **Pilot** (routed hole, `pilot` on) | a **drill** op preceding the hole's helical route |

The oblong `major` axis + hole centre come from the board hole (`drill_x`/`drill_y`);
the strategy from the step config; the drill tool from the assignment; the slot router
from the rack's mandatory router.

> **Routed paths keep KiCad's move types — they are not flattened to G1.** KiCad gives
> an *unordered* set of edge moves (line, arc, bezier). Stitching's job is to **re-order
> them into continuous closed loops** and snap segment endpoints for perfect continuity;
> tessellation is used **only internally** to resolve connectivity and nesting
> (point-in-polygon), never as the output. A contour is therefore an ordered list of
> **typed segments** (line / arc / bezier), and a route op expands to the matching
> primitives — `linear_cut` (G1), `cut_arc` (G2/G3), `cut_bezier` — so arcs stay arcs.
> Rationale: one CNC arc is far more accurate and faster than the *n* × G1 chords a
> tessellation emits. **This reworks today's stitcher** (§9.6), whose
> `pcb::Contour.points: Vec<(i64,i64)>` currently discards the segment types.

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

## 6. Coordinate placement

Every op's `entry`/`exit`/`z` are **machine coordinates**, but the geometry arrives in
board (design) space. A single **Placement** object owns that mapping, so the transform
lives in one place instead of scattering offset/scaling/rotation math through the
planner and Coder. It is built **once per step** from the JobInstance (fixture +
machining + CNC + board bounds) and is a pure function of them.

**XY** — a composed affine `board → machine`:
- **orientation** — the step's board rotation (`board_orientation`).
- **fixture origin** — where the board sits and which corner is X0/Y0
  (`work_origin_reference` x0/y0 = Left/Right/Front/Back).
- **CNC scaling** — per-axis calibration (`machine.scaling.x/y`).
- *(the work-coordinate-system origin — G54/G55 — is set in `initialise`; the Placement
  produces coordinates **relative to** that WCS.)*

**Z** — reference plane and depth math:
- **reference** — `fixture.z0_reference` (machine bed / spoilboard top / board top); Z0
  is the bed for k2g, so heights are bed-relative.
- **surfaces** — board-top = f(fixture backboard thickness, board thickness).
- **depths** — through-hole `z_bottom = surface − (thickness + breakthrough)`, bounded by
  `bed_clearance`; `z_retract` and `z_safe` from the fixture.

The Placement exposes the primitives the planner needs — `xy(board_pt) → (mx,my)`,
`z_bottom(through)`, `z_retract()`, `z_safe()` — so the planner emits ready-to-render
coordinates and the Coder only **formats** them (`fmt`), never computes geometry.
Because ops are placed in machine space, the §4 TSP minimises **physical** travel.

> Build this **before** routing gets real: CNC offsets, per-axis scaling, fixture
> stack-up and board rotation compound quickly, and centralising them here keeps that
> complexity out of every op and out of the templates.

---

## 7. Handoff to the Coder

The Coder (`gcode-engine.md`) walks the ordered plan. The program-scope job context
(`machine.*`, `cnc.*`, …) and modal unit state are injected once; then:

- **At each tool-block boundary** — emit `change_tool` (slot, rpm, manual message) then
  `start_spindle`.
- **Per op** — `rapid_move` to `entry` at safe Z → the op's primitive
  (`drill`/`peck_drill`, or `linear_cut`/`cut_arc` for routes) → retract.
- **Between ops** — the rapid at safe Z *is* the transit whose XY length the TSP
  minimised (`exit`ᵢ → `entry`ᵢ₊₁).
- `initialise` / `conclude` bookend the program (already implemented).

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
   program** (its own `initialise` + body + `conclude`); steps may target different
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
   - **Stitcher output model** — `pcb::Contour` changes from `points: Vec<(i64,i64)>` to
     an ordered `Vec<Segment>` (Line / Arc / Bezier); tessellation stays *internal* to the
     connectivity + nesting tests, not the result.
   - **Segment-wise offset** — the toolpath is the contour offset by the tool radius, so
     the offset must be computed **per segment** (line → parallel line, arc → concentric
     arc) with join handling (fillet arc at convex vertices, trim at concave), **not** via
     clipper2's point-polygon offset (which reflattens to G1). This is standard 2D cutter
     compensation — well-understood, but real geometry work, and the main cost of this
     decision.

**Still open:**

7. **Bezier offset.** A bezier's offset isn't a bezier; approximate by biarc fit, or fall
   back to the `cut_bezier` primitive. Rare in Edge.Cuts — decide when one actually
   appears. (Lines and arcs, the 99 % case, offset exactly.)
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

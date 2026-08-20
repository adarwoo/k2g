# Validation plan

For one operator, on the hardware this project actually has: a **Genmitsu 3018-Pro**
and a **MASSO G3** (with and without its ATC), on **Windows and Linux**. macOS is built
and signed by CI and has never been run — it is validated by launching the artifact, and
nothing more is claimed for it.

Ordered by consequence, not by feature list. The failure that matters here is not a
crash: it is **a program that looks right and cuts the wrong thing**. So every case says
what to *observe*, and every bench case says what to *measure*.

## How to use this

Work down the areas in order. §A before §B before §C — a rig whose depths are wrong will
fail later cases in ways that look like other bugs.

Each case is marked with where it is proved:

| | |
|---|---|
| **auto** | a test in `cargo test`; CI reports a regression |
| **read** | inspect the emitted program or the plan views; no machine needed |
| **bench** | only a machine can prove it |

### The escalation ladder — for every bench case

1. **Read the program.** Job → Code. The origin in the header, the tool changes, the Z
   values. A depth with the wrong sign is visible here and nowhere else cheaply.
2. **Look at it.** Job → Machining, in 3D. Wrong geometry is obvious here; wrong *depth*
   is not, which is why reading comes first.
3. **Air cut.** Work Z origin about 10 mm above the real surface, and run the whole
   program. Watch for rapids that dive, tool changes that do not stop, travel that leaves
   the table.
4. **Sacrificial material.** FR4 or MDF at the true Z. Depth, breakthrough and tab
   thickness first become real here.
5. **The board.**

Stop and log a defect the moment a step surprises you. Do not carry on to see.

---

## A. Depth, origin and the bed — first, always

Everything that can break a cutter, a board, or the bed. Nothing below §A is trustworthy
until these pass.

| # | Case | How | Expect |
|---|---|---|---|
| A1 | Z0 is the board top | Any drilling step, read the program | Every cutting Z negative; retract and safe heights positive **read** |
| A2 | The backboard bounds the plunge | Backboard 2.5 mm, breakthrough 0.5 mm | Deepest Z about -(board + 0.5), never near -(board + backboard) **read**, then **bench** on scrap: bed unmarked |
| A3 | Bed clearance refuses a tool | Stock a drill whose point would reach the bed | Refused, naming the tool **read** |
| A4 | Flute length reaches | Cutter with flute shorter than board + breakthrough | Refused or warned, never silently short **read** |
| A5 | The origin reference is validated | Fixture origin `G54.1 P7` on the Genmitsu | **Refused** — that controller has G54–G59 only **read** |
| A6 | The origin reference is honoured | `G55` on the MASSO | `G55` in the header, and the machine G55 is where the fixture sits **bench** |
| A7 | Bed origin corner | Fixture `x0: right`, `y0: far` | Board occupies negative X; the cut lands at the stop **bench** on scrap |
| A8 | Axis scaling | `scaling.x = 1.001` | X ordinates scale, Y unchanged **read** |
| A9 | Safe height clears clamps | A tall clamp beside the board | Rapids pass over it **bench**, air cut |

> **A5 is the one that silently ruins a board.** An offset the controller does not have
> leaves the job running against whatever origin happens to be active.

## B. Two-sided registration

The other way to scrap a board irrecoverably. Use a **coupon**: a rectangle with an
asymmetric hole pattern, so a mirror error is visible rather than plausible.

| # | Case | How | Expect |
|---|---|---|---|
| B1 | Pins come first | Pins on step 2 | Refused, naming the step **auto/read** |
| B2 | Pins are front-face | Pins step set to back | Refused **auto/read** |
| B3 | A face change needs pins | Two steps, opposite faces, no pins | Refused **auto/read** |
| B4 | Flip axis Y (page turn) | `board_flip_axis: y`, coupon | Back-face holes land on the pins; the pattern is not mirrored **bench** |
| B5 | Flip axis X (tumble) | The same with `x` | As B4 — and confirm B4's setting would have been wrong here |
| B6 | The prompt appears | Any back-face step | The program opens by asking that the board is back-face up **read** |
| B7 | Pin diameter | 3.2 mm against 3.175 mm | The hole takes the pin with the play the manual claims **bench** |

## C. Tool selection and the rack

| # | Case | How | Expect |
|---|---|---|---|
| C1 | The kerf is matched exactly | Kerf 2 mm, no 2 mm router in stock | The step fails naming the size — never a narrower channel **auto/read** |
| C2 | Oversize and undersize allowance | Hole 0.85 mm, only a 0.8 mm drill | Chosen, with the difference shown on Tooling **read** |
| C3 | Out of stock is never chosen | Mark the only fitting drill out of stock | Falls back or fails; never selects it **read** |
| C4 | Preference breaks a tie | Two equal drills, one Preferred | The preferred one **read** |
| C5 | Route fallback | A hole larger than any drill | Milled, badged as routed on Tooling **read**, then **bench** — measure it |
| C6 | Pilot hole | Enable pilot on C5 | A pilot drilled before the mill **read** |
| C7 | Oblong strategies | One slot pad, each of the four values | Drill chain, route, or both as named; orientation correct **bench** on scrap |
| C8 | ATC rack schedule | MASSO with ATC, three tools | Rack view fixed/load/kept correct; the tool-change lines agree **bench** |
| C9 | Fixed and do-not-use slots | Pin a tool to T1, disable T3 | T1 used as pinned; T3 never allocated **read** |
| C10 | Rack too small | Fixed toolset, more tools than slots | Hard failure naming the shortfall **read** |
| C11 | Reload and hybrid policies | The same job under each | Pauses and manual prompts where claimed **bench** |
| C12 | Manual tool change | Genmitsu, two tools | Spindle stops, prompt shown, resumes cleanly **bench** |
| C13 | Racks belong to machines | Two steps, different machines | Each step shows its own machine rack **read** |

## D. Copper isolation

The newest capability, and the one with a depth *tolerance* rather than a depth.

| # | Case | How | Expect |
|---|---|---|---|
| D1 | A width the board can take | Dense board, 0.25 mm | Nets separate; narrowed pairs listed in the step notes **bench**, then continuity-test two adjacent nets |
| D2 | A width it cannot | 0.6 mm on fine pitch | Refused, or narrowed **and said so** — never silently narrowed **read** |
| D3 | No suitable V-bit | Remove the fine V-bits from stock | Refused, naming the width **read** |
| D4 | Both faces at once | Engrave on a front step and a back step | One generation settles; no repeating `isolation_ready` in the Logs; both faces engraved **read** — regression for `37351c8` |
| D5 | Depth on real copper | D1 on scrap clad | Copper cleared, substrate barely marked; measure the channel against the request **bench** |
| D6 | Warp sensitivity | D5 on a board deliberately not flat | **Expected to fail** — there is no height map. Record how badly. This is the evidence for building one |
| D7 | The width set to the board's own clearance | 0.2 mm on an 8 mil board, 0.1 mm tip | Every trace keeps a channel on **both** sides. The regression for the fault where stretches went missing and nothing was reported **read** + **bench**, continuity-test the fan-out |
| D8 | Silence means success | Sweep the width 0.1 → 0.4 in 0.05 steps on a dense board | At every step, either the copper is fully separated or a note or banner says what could not be cut. A quiet pass must never be a joined board **read** |
| D9 | Joined copper is raised even when the outlines look perfect | A board with two pads closer than the tip | The "cannot be separated" banner fires although every contour is a tidy closed loop — `intact_fraction` cannot see this and must not be the only check **read** |

## E. Outline, cutouts and retention

| # | Case | How | Expect |
|---|---|---|---|
| E1 | Tab count and width | Four tabs, 2 mm | Four bridges, longest side favoured, about 2 mm each **bench**, measured |
| E2 | Mouse bites | Enable on E1 | Perforated tabs that snap cleanly and file to nothing **bench** |
| E3 | No retention | Mode none, taped stock | One pass, part free **bench**, only with hold-down |
| E4 | A cutout holds its slug | A 20 mm opening | The slug is held by one tab, not thrown **bench** |
| E5 | Corner relief | A cutout with square internal corners | Corners drilled tangent, never cutting past the drawing **read** + **bench** |
| E6 | A small opening | 1.5 mm slot against a 2 mm edge kerf | Cut by a smaller cutter chosen to fit, not reported impossible **read** |
| E7 | A curved outline | Board with an arc edge | Arc words emitted, not hundreds of chords **read** |
| E8 | No arc word | A CNC profile with `cut_arc` emptied | Arcs become short straight moves; geometry intact **read** |
| E9 | score and vgroove | Set either | **No outline pass at all**, and the step says so. Declared, not implemented — do not chase **read** |
| E10 | Finishing leaves a wall | 0.3 mm on E1's board | Twice the outline ops, every `.rough` before every `.finish`; 3D shows two loops 0.3 mm apart with the tab gaps aligned **read** |
| E11 | Finishing cuts to size | Cut E10, and the same board at 0 | Both measure to the drawing; the 0.3 mm one has the better wall and a channel 0.3 mm wider on the waste side **bench**, measured |
| E12 | Finishing is climb | E10 | The `.finish` spans run the opposite way round the boundary from the `.rough` ones — clockwise, seen from above **read** |
| E13 | …after a flip | E10 on a `board_face: back` step | Still clockwise. The direction is taken after the mirror, so a mirrored step must not silently go conventional **read** |
| E14 | Nothing to hold it | E10 with retention `none` | One pass, and a note saying the finishing pass was dropped. Same for a cutout with `retain_island: false` **read** |
| E15 | An allowance wider than the kerf | Finishing 2 mm against a 2 mm kerf | One pass, and a note about the passes not overlapping **read** |

## F. Drilling

| # | Case | How | Expect |
|---|---|---|---|
| F1 | Plating allowance | A 0.8 mm finished plated hole | Drilled oversize by the plating; measure **bench** |
| F2 | Non-plated holes are untouched by it | A mixed board | At nominal **read** |
| F3 | The drill map matches KiCad | Any board | Counts and positions match the KiCad drill table **read** |
| F4 | Buried vias are refused | A board with one | An explicit error, not silence **read** |
| F5 | Modal cycle | One cycle per hole, then a modal template | Both forms drill identically **bench** |

## G. Program output and dialects

| # | Case | How | Expect |
|---|---|---|---|
| G1 | Both bundled dialects | One job through the Genmitsu and the MASSO profile | Each runs on its own machine **bench** |
| G2 | Units | A metric profile and an imperial one | The unit word and the ordinates agree; never a mixed-unit program **read** |
| G3 | Line numbering | `line_format` with an increment | Numbered as written; comments treated as the template says **read** |
| G4 | File extension | A CNC set to `ngc` | The saved file takes it **read** |
| G5 | Multi-step save | A three-step job | One folder prompt, three named files, no clash **read** |
| G6 | USB save and eject | A stick plugged in | Saves, and ejects only on a complete batch **bench** |
| G7 | A folder per stick | Two sticks, different folders | Each reopens in its own; one returning on another letter still does **bench** — regression for `7a322a3` |
| G8 | The program matches the job | Change a setting after generating | The pill leaves ready; the stale program cannot be saved **read** |

## H. Data, migration and lifecycle

| # | Case | How | Expect |
|---|---|---|---|
| H1 | Old profiles open | Keep copies of pre-0.12 machining profiles | They load with no validation errors, and `mill_board` is folded onto the outline operation as a mill cut **auto/read** |
| H2 | Everything auto-saves | Edit, then kill the process | The edit survives **bench** |
| H3 | Reopen where left off | Restart | Screen, step and 3D view as they were **read** |
| H4 | Import and export profiles | Round-trip each kind | Identical after re-import **read** |
| H5 | Catalog import | A third-party catalog file | Imported, and its tools can be added **read** |
| H6 | Duplicate stock naming | Add the same catalog tool twice | The second is suffixed, and the two are distinguishable in the rack picker **read** |
| H7 | Factory reset | Settings, Reset | Profiles, stock and job gone; catalogs and the security log kept **read** |
| H8 | Delete all data | Settings, Delete | The directory is gone; the next start is a fresh install **read** |

## I. KiCad integration

| # | Case | How | Expect |
|---|---|---|---|
| I1 | Enable the API | With KiCad closed | The setting is written and a backup copy made **read** |
| I2 | Refused while running | With KiCad open | Blocked, with the reason **read** |
| I3 | Register the plugin | Then restart KiCad | A Create GCode button, which opens k2g with that board **bench** |
| I4 | A stale registration | Move the k2g install | The badge says stale; re-registering fixes it **read** |
| I5 | Refresh after an edit | Move a footprint, press refresh | New geometry, and regeneration follows **read** |
| I6 | Two KiCad instances | Two boards open | k2g takes whichever owns the socket. **Known gap** — the picker is not wired **read** |
| I7 | Disconnect mid-job | Close KiCad after the board is loaded | The cached board still generates; no half-read state **read** |

## J. Updater and packaging

| # | Case | How | Expect |
|---|---|---|---|
| J1 | A signature is accepted | Install 0.11.0, then offer 0.12.0 | Banner, verification, install **bench** |
| J2 | A bad signature is refused | Corrupt a downloaded installer | Deleted and reported; never run **read** |
| J3 | Per-user install | The setup executable | Installs per-user with no elevation **bench** |
| J4 | Installer scope | The MSI | **Per-machine, and elevates.** Known mismatch: the updater prefers the MSI. Settle this before J1 becomes routine **bench** |
| J5 | Skip and postpone | Both | Honoured, and undoable from Settings **read** |
| J6 | Linux artifacts | deb, AppImage, tarball | Launch and generate on the Linux box **bench** |
| J7 | macOS artifacts | dmg and app zip, both architectures | Launch only — never yet run **bench, if a Mac appears** |

## K. Board fixtures to keep

Build these once in KiCad and reuse them; most cases above name one.

1. **Coupon** — 40x30 mm, asymmetric hole pattern, two locating pins. §B, §F.
2. **Curved** — an arc or rounded corners. E7, E8.
3. **Cutouts** — a 20 mm opening, a 1.5 mm slot, a square internal corner. E4–E6.
4. **Slots** — oblong pads at three angles. C7.
5. **Dense** — fine-pitch part, a ground pour, some 0.2 mm gaps. §D.
6. **Tiny** and **large** — the smallest sensible board, and one near the bed limit. The
   large one also probes the absent envelope check (§L).
7. **Rejects** — one with a buried via (F4); one whose edge cuts do not close.

## L. Known-absent — do not raise these as defects

Confirmed by reading the code, not assumed. Chasing them wastes bench time.

- **No surface probing or height map.** Engraving cuts one fixed Z per span (D6).
- **No machine envelope.** Neither the CNC nor the fixture carries travel or bed size, so
  whether a board fits cannot be checked, and rotation to fit is manual.
- **No job save or open.** The board lives in memory, so a job is not reproducible once
  KiCad moves on. No Gerber import either — the KiCad API is the only way in.
- **No program editing** in the Code view, and no send-to-machine.
- **No panelisation, tool-life counting, multi-pass depth, marking engraving, or undo.**
- **score and vgroove** are accepted and do nothing (E9).
- **No per-pass feeds.** The finishing pass cuts at the same feed as the roughing one
  (E10–E15); a tool has one lateral rate and nothing scales it per pass.
- **Tab nudging** — the planner honours a stored offset; nothing in the UI sets one.

## M. Standing gates

Run before every tag, not per case.

- `cargo test --workspace` **and** `cargo test --workspace --release`. Both, always: a
  real fault shipped that appeared only in an optimised build (`e274d17`).
- The CI platform matrix. A path bug was green on Windows and failed Linux and macOS
  (`7a322a3`); one platform is not coverage.
- `cargo clippy --workspace --all-targets` — reporting, not gating.
- After a 3D-view change, the headless WebGL harness; after a stylesheet change, the
  headless CSS harness. Both render the shipping source, so neither can drift from it.

## N. Recording a defect

Log against the case number, with the profile, the board, the emitted program, and the
Logs diagnostics tail. Severity:

| | |
|---|---|
| **1 — wrong metal** | Emits a program that cuts something other than what was asked, without saying so. Everything in §A and §B is a candidate. Stop testing and fix it. |
| **2 — refuses wrongly** | Blocks a job that is genuinely machinable. |
| **3 — visible fault** | A crash, a hang, a wrong number on screen. Noticed at once, so it cannot reach the metal. |
| **4 — cosmetic** | Wording, layout, a stale label. |

## O. Done when

- Every §A and §B case passes on both machines, twice, on different days.
- §C to §G pass on at least one machine each, with the Genmitsu and the MASSO having each
  cut a complete board end to end.
- One board is drilled, isolated and cut out in a single job, on both faces, and fits its
  enclosure — the whole product in one artefact.
- Every severity-1 and -2 defect is fixed and carries a regression test named after the
  behaviour it protects.

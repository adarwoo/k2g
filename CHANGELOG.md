# Changelog

Notable changes per release. Format follows [Keep a Changelog](https://keepachangelog.com);
k2g is pre-1.0, so minor versions still carry breaking changes to stored data — migrations
are automatic, and [schema-versioning.md](docs/design/schema-versioning.md) describes how an
old file reaches a new build.

Releases are tagged with a name as well as a number (`v0.14.0-first-board`), because the
name is usually the shorter answer to "what changed".

## [Unreleased]

### Changed
- **Setting explanations are tooltips.** The paragraph under each setting has moved into
  a tooltip: hover the setting, or the ⓘ beside its name, to read it. Forms are much
  shorter as a result.
- **A step's operations are shaded alternately**, so it is clear where one operation's
  settings end and the next begin.
- **The Board view shows only the face the selected step machines.** A front step shows
  the front copper, a back step the back copper; the other face is face-down on the bed
  and is no longer drawn over it. An isolation pass on the other face is left out with it.
- **A back-face step shows the board turned over**, about the axis set by that step's
  fixture (*Board flip axis*), so what is on screen is the board as it lies on the bed.
  The strip above the board says which face is up.
- The Board view's legend names the copper layers **Front copper (F.Cu)** and **Back
  copper (B.Cu)**, matching the step's *Board face to machine* setting.
- **One problem is one error.** A problem that blocks generation, such as copper that
  cannot be separated, used to bring a second error with it, "Blocking runtime errors
  present", which said nothing of its own. It is gone; the Code tab still lists the real
  reason.
- **Each copper face is drawn as one flat colour.** Where a track ran onto a pad, or a
  thermal spoke met its pour, the overlap used to show darker or lighter, which is
  KiCad's business and not the machine's. The copper left standing between nets is the
  same colour too: it is copper, and the channels are what outline the nets.
- **Board outline errors say what is wrong and where**, instead of "Open contours
  detected".

### Added
- **3D mouse support (Windows).** A 3Dconnexion SpaceMouse now steers the Machining 3D
  view: slide to pan, push or pull to zoom, twist and tilt to orbit, with the left button
  resetting the view and the right one switching between flat and 3D. It works alongside
  the 3DxWare driver and only while k2g is the window in front.
- **Preview the setup sheet.** The eye button beside the setup sheet in the Export dialog
  opens it in your PDF viewer without exporting it, so you can check it, or print it, first.
- **Operations are added, removed and ordered within a step.** The operation checkboxes
  are replaced by **+ Add operation**, which lists what the step does not run yet (one
  another step already has is greyed, with the step named). Each operation's section has
  **↑ ↓ ✕** to reorder or remove it. The step runs them in the order shown, so engraving
  can now come **after** the holes, which protects small pads from being torn by the
  drill. Moves that would make the step unrunnable (holes after the outline, say) are
  greyed, with the reason in the tooltip. Existing profiles keep the order they had.
- **Router max depth per pass.** The board outline, interior cutouts and milled holes
  and slots are now roughed out in several shallower passes instead of one full-depth
  pass, because the sideways force of a full-depth cut can shift the board on the
  fixture. One setting per step, next to *Board face* (default 1 mm). The depth is split
  into equal passes, so a 1.6 mm board with 0.5 mm breakthrough is cut in three passes of
  0.7 mm. Finishing passes still cut full depth in one go. Set it to 0 to go back to one
  pass. **Existing profiles get the 1 mm default, so routing takes longer than before.**
- **+ Add step puts the new step where it can run.** A step added to a profile that
  already cuts the board outline now goes in before that step instead of after it, where
  it would have machined a board that had already been cut free.
- **Setup sheet.** Exporting now also writes a PDF beside the programs (untick it in the
  Export dialog to leave it out). It covers the board (size, holes, engraving) and gives
  each step its own page: side, machine and kind of work, operations, setup (fixture,
  work zero, how the board turns over), the rack with every manual tool change, and the
  tools in run order with the feeds and speeds the program uses.
- **Show on board.** An error with a place on the board, such as copper that cannot be
  separated, a gap in the outline or holes no drill in stock can make, has a *Show on
  board* button in the diagnostics. It opens the Board view zoomed to the problem, turning
  to a step that machines that face if needed. An error naming several places steps
  through them with ‹ ›.
- **The Board view marks every problem** in red, listed under *Problems* at the top of its
  legend.
- Errors that are fixed elsewhere have an *Open* button that goes there: the profile
  screen, or the Tooling or Code tab for that step.

### Fixed
- **Milled round holes come out to size.** Locating pins milled for 3.2 mm with a 2 mm
  router measured 3.0 mm: the finishing lap cuts at full depth with real material in
  front of it, so the cutter bends away from the wall. After that lap, every milled round
  hole now gets one more lap on the same wall at a quarter of the feed, then spirals back
  in to the centre and lifts from there, so the cutter never stops or lifts on the finished
  wall. All of it is climb milling. Drilling pins with an exact-size drill, when one is
  stocked, is unchanged.
- **Exported program names carry the step number**: `panel_step1_Engrave.nc`,
  `panel_step3_Route outline.nc`. They used to carry the step name alone, so the order to
  run them in could only be found in the setup sheet. Two steps with the same name no
  longer have to fall back to bare numbers.
- **Copper could show darker or hollow where a track ran onto a pad** on a face with no
  isolation pass, such as an outline step. A track and a pad wound in opposite directions
  cancelled where they overlapped. They are now always drawn as one.
- **The Export dialog warns when the work zero has moved** since the board was last
  exported. Adding a step after others were cut (an outline route, say) moves the board in
  every program, which used to happen without a word, leaving the last step offset from a
  board already half made.
- **Reloading the board after moving tracks kept the old isolation paths.** A reload
  only counted as a new board when the outline or holes had changed, so after a
  copper-only edit in KiCad the Board view drew the new copper with the old channels round
  it, and the program was not regenerated, until k2g was restarted. Every reload now
  recomputes the isolation and regenerates the program.
- **Copper clearing could cut a straight line through copper it was meant to leave.** When
  a piece of free copper wrapped round copper that stays (a U shape, say), its inner
  passes split into separate arms. The program still went from one arm to the next
  without lifting, cutting straight across whatever lay between them. A pass now only
  continues to the next ring without lifting when the move stays on copper being cleared.
  Otherwise it lifts and plunges again.

## [0.16.0] — 2026-09-17 — *clearing*

The copper that isolation leaves standing is now taken out, not only cut around.

- The Board view draws the isolation pass as the copper it leaves behind.
- Stranded copper islands are removed by the V-bit already cutting the channel.
- Free copper up to a chosen width can be cleared with a milling bit.
- The *Engraver* tool family is now *Milling*, and the generic catalog carries milling bits.
- The in-app updater can install on Windows.

### Added
- **The Board view now shows the isolation pass, as the copper it leaves behind.** Not a
  centre line — the channel is drawn at the width the bit actually cuts, taken out of the
  copper it removes, so what is on screen is the board that comes off the machine. A face
  about to be isolated is drawn as it really is before the cut: entirely copper, because
  milling takes only the channel and the field between the nets stays standing. A V-bit
  shows the flat at the bottom of its groove as a darker band inside the channel; a
  flat-ended cutter shows one band, which is what it makes. Stretches the pass had to
  narrow to fit between close nets are marked where they happen, rather than only counted
  in the step's notes, and the tool's own line runs down each channel in the colour the 3D
  view draws it — so two lines in one channel is two passes that overlapped. The legend row
  switches the whole thing off and puts the copper back whole. The 3D view is unchanged.
- **The isolation pass now takes out the copper it leaves stranded.** Cutting a channel
  round each net leaves everything else standing, so where two nets sit further apart than
  twice the channel the copper between the two channels survives as an *island*: on no net,
  on no layer of the design, and solder will bridge it. The commonest one is the ring a
  ground pour's clearance leaves round every trace in it. **Remove islands** is on by
  default in `engrave_copper`, bounded to islands no wider than three channels, and cut by
  the V-bit already in the spindle at the depth it is already running — no tool change and
  no rack slot. Wider copper is left alone and its width reported in the step's notes,
  which is what says whether a router would be worth loading. Copper on a net is never
  touched, and anything the bit could not reach without touching one is counted in the
  notes rather than dropped in silence.
- **Narrow copper can be cleared with a milling bit.** Island removal reuses the V-bit and
  stops at three channels wide; `engrave_copper` now also offers **Clear narrow copper with
  a mill**, which loads a flat milling bit and clears free copper up to **Clear copper
  narrower than** (2 mm by default), at the isolation depth, and never closer to a net than
  **Clearing safety margin** (0.2 mm). The bit is the largest milling bit in stock no wider
  than the threshold. It costs a tool change and a rack slot, so it is off by default. A step
  with no suitable bit skips the pass and says so in its notes rather than refusing to
  generate, and copper too narrow for the chosen bit to enter is reported, not left in
  silence.
- **Each piece of copper is cleared in one continuous run.** An island's rings, and a
  clearing pass's, are cut outside-in with a short hop from one ring to the next, instead of
  a retract and a re-plunge between every ring.
- **The generic catalog has a *Milling bits* section**: eight flat end mills from 0.5 mm to
  2.5 mm, for the clearing pass. Like everything else in `generic.yaml`, the feeds and speeds
  are placeholders to calibrate on your own machine before production use.
- `K2G_FORCE_UPDATE_CHECK` runs the update check immediately against the latest release,
  bypassing the daily interval, postponements, skipped versions and the "is it newer"
  comparison. Signature verification still applies. For diagnosing the updater.

### Changed
- **The *Engraver* tool family is now called *Milling*** in stock, catalogs and their
  filters. Only the label changed: files still store `engraver`, so existing stock and
  catalogs load as they are.
- **The bundled catalogs follow the installed build.** `generic.yaml`, `kyocera.yaml` and
  `unionfab.yaml` in the catalogs folder are rewritten whenever the build's own copy differs,
  so tools added in a release reach installs that were seeded by an older one. This replaces
  hand edits to those three files as well. Catalogs under any other name are never touched.
- Dependencies updated: jsonschema 0.50 (with network `$ref` fetching now compiled out, not
  only refused), image 0.25, rfd 0.17, pulldown-cmark 0.13 and base64 0.23. The lockfile is
  31 packages smaller.

### Fixed
- **The in-app updater can install on Windows.** It started the downloaded `.msi` as if it
  were a program, which Windows refuses, so every update failed. The installer is now opened
  the way double-clicking it would. Once it has started, the update banner says so instead of
  staying on "Downloading and checking the signature…", and the download is marked as coming
  from the internet so SmartScreen evaluates it. **0.15.0 still carries the old updater, so
  an existing install has to be updated to 0.16.0 by hand, once.**
- **The TLS library behind the update check is patched** (rustls 0.23.45, RUSTSEC-2026-0285).
  It accepted handshake messages sent unencrypted that should have been encrypted. That
  could not alter or complete a connection, and a downloaded installer is signature-checked
  whatever the connection, but it is the one network path k2g has.
- **When the requested isolation width is below what the finest bit can cut, the step says
  so.** Lowering the width further changed nothing, and nothing on screen explained why; the
  step's notes now name the floor the finest bit in stock or rack sets.

## [0.15.0] — 2026-09-08 — *templates*

A library to start from, instead of whatever had been written first.

- Every **Add** dialog opens on *User defined*; the bundled seeds are listed under it.
- Seven machining templates and three fixture benches, up from two and one.
- A machining template may describe more than one setup — a flip, or a trip to the
  plating bath — and binds on every step rather than only the first.
- The user manual is searchable from inside the application.
- The security log records its own rotation, so a truncated history says it is one.

### Added
- **Every Add dialog now starts from *User defined*, and the bundled template lists are
  worth reading.** Machining offers seven — the two single-sided profiles, isolation on its
  own, a double-sided job, a chemically-plated one that drills before the bath and finishes
  after it, drill-only and cut-edges-only — and Fixtures offers three benches (clamped,
  taped, and pinned for double-sided work) rather than one. None of them saves more than a
  minute of ticking boxes; what they carry is the *order*, which is what a first job gets
  wrong: isolation before drilling, locating pins in the step above a flip, plating between
  two setups.
- **A machining template may describe more than one setup.** Templates now bind their
  CNC, fixture and toolset on every step instead of only the first, so a two-step profile
  arrives ready to generate rather than half-wired.
- **The user manual is searchable from inside the application.** Typing in the Manual
  screen's search field marks every occurrence, narrows the contents rail to the sections
  that contain one, and steps between matches with Enter / shift+Enter. Matching runs while
  the page is rendered rather than in the WebView, so what is typed is never interpolated
  into a script.
- The security log **records its own rotation**. Discarding the oldest generation was the
  one action that destroyed part of the record without mentioning it, so a reader could not
  tell a complete history from a truncated one. A rotation now writes a `log.rotated` entry
  giving what was rolled and what was discarded.
- Issue forms, `CONTRIBUTING.md` and this changelog.

### Fixed
- `src/data/schema_export.rs` and `src/gcode/testcut.rs` were referenced by committed code
  but had never been committed themselves, so `main` did not compile for anyone who had not
  built it locally.
- The weekly security audit could not report what it found: on a schedule the advisory
  action files an issue rather than a check run, and the job lacked permission to. It had
  been failing on the report, not the audit.
- `chacha20` had been yanked from crates.io; updated to a non-yanked patch release.

### Changed
- CI actions moved off the deprecated Node 20 runtime.

## [0.14.0] — 2026-08-23 — *first board*

The release the project was for: a real board, drilled, isolated and cut out.

- Isolation machines the board and says so, instead of the toolpath silently vanishing.
- Each isolation channel is cut once, rather than once from each side.
- Machining order is one order — the list, the blocks and the program agree.
- Copper is no longer read while KiCad is still pouring it.
- The build that is running is reported, not just the release it came from.
- Stock's *Usage* column reports two independent facts rather than a rack count.
- Fixed a release-only compile error in the stock sort header.

## [0.13.0] — 2026-08-20 — *retention*

- V-bit width and the finishing pass corrected.
- Inverted colours in the 3D view fixed.
- Manual validation plan added.

## [0.12.0] — 2026-08-19 — *signed*

Every release artifact now carries a detached minisign signature, verified against the key
compiled into the application.

- Programs export to removable media, remembering a folder per drive.
- One k2g per user; a second launch hands over to the running window.
- Contours held for both faces rather than only the last one.
- The 3D view stays where the operator left it.
- Design documents moved to `docs/design/`, with link checking.

## [0.11.0] — 2026-08-15 — *manual*

The user manual, shipped inside the application as well as in the repository.

## [0.10.0] — 2026-08-13 — *linux tested*

Linux built, tested and published alongside Windows and macOS.

## [0.9.0] — 2026-07-29 — *typed values*

Typed values in templates, and the machine language moved out of the application.

## [0.8.0] — 2026-07-26 — *tabs and templates*

One profile per binding, G-code generation out of the application, computed tab placement.

## [0.7.0] — 2026-07-25 — *oblong*

Oblong slots: routed-feature rendering, width-aware slot routers, drill-chain geometry.

## [0.6.0] — 2026-07-24 — *gcode generation*

G-code generation begins — the Coder and header rendering, a syntax-highlighted code view, a
schema-documented primitive editor with validate and preview, and the Logs and About screens.

## [0.5.0] — 2026-07-22 — *machining steps*

Machining profiles gain ordered steps; the Job becomes a live singleton referencing one
machining profile.

## [0.4.0] — 2026-07-20 — *crate extraction*

`units`, `datastore` and `pcb` extracted into their own crates.

## [0.3.0] — 2026-07-17 — *job profile summary*

Job and profile ownership migration, and summary updates.

## [0.2.0] — 2026-07-16 — *processing sync*

Processing schema, UI and persistence brought into step.

[Unreleased]: https://github.com/adarwoo/k2g/compare/v0.16.0-clearing...HEAD
[0.16.0]: https://github.com/adarwoo/k2g/compare/v0.15.0-templates...v0.16.0-clearing
[0.15.0]: https://github.com/adarwoo/k2g/compare/v0.14.0-first-board...v0.15.0-templates
[0.14.0]: https://github.com/adarwoo/k2g/compare/v0.13.0-retention...v0.14.0-first-board
[0.13.0]: https://github.com/adarwoo/k2g/compare/v0.12.0-signed...v0.13.0-retention
[0.12.0]: https://github.com/adarwoo/k2g/compare/v0.11.0-manual...v0.12.0-signed
[0.11.0]: https://github.com/adarwoo/k2g/compare/v0.10.0-linux-tested...v0.11.0-manual
[0.10.0]: https://github.com/adarwoo/k2g/compare/v0.9.0-typed-values...v0.10.0-linux-tested
[0.9.0]: https://github.com/adarwoo/k2g/compare/v0.8.0-tabs-and-templates...v0.9.0-typed-values
[0.8.0]: https://github.com/adarwoo/k2g/compare/v0.7.0-oblong...v0.8.0-tabs-and-templates
[0.7.0]: https://github.com/adarwoo/k2g/compare/v0.6.0-gcode-generation...v0.7.0-oblong
[0.6.0]: https://github.com/adarwoo/k2g/compare/v0.5.0-machining-steps...v0.6.0-gcode-generation
[0.5.0]: https://github.com/adarwoo/k2g/compare/v0.4.0-crate-extraction...v0.5.0-machining-steps
[0.4.0]: https://github.com/adarwoo/k2g/compare/v0.3.0-job-profile-summary...v0.4.0-crate-extraction
[0.3.0]: https://github.com/adarwoo/k2g/compare/v0.2.0-processing-sync...v0.3.0-job-profile-summary
[0.2.0]: https://github.com/adarwoo/k2g/releases/tag/v0.2.0-processing-sync

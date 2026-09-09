# Changelog

Notable changes per release. Format follows [Keep a Changelog](https://keepachangelog.com);
k2g is pre-1.0, so minor versions still carry breaking changes to stored data — migrations
are automatic, and [schema-versioning.md](docs/design/schema-versioning.md) describes how an
old file reaches a new build.

Releases are tagged with a name as well as a number (`v0.14.0-first-board`), because the
name is usually the shorter answer to "what changed".

## [Unreleased]

### Added
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

[Unreleased]: https://github.com/adarwoo/k2g/compare/v0.15.0-templates...HEAD
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

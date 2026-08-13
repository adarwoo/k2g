# k2g — KiCad to GCode

**CAM for making PCBs on a hobby CNC.** k2g reads the board open in KiCad and writes the
GCode that drills and cuts it out: holes, oblong slots, board outline with breakaway
tabs.

It is not a generic CAM package. It knows what a PCB is — plated and non-plated holes,
pad stacks, board outline, cutouts — and what your machine is, so the questions it asks
are about *machining a board*, not about polygons and offsets.

Written in Rust. Runs as a desktop application beside KiCad and talks to it live over
KiCad's IPC API, so there are no Gerber or Excellon files to export and re-import: edit
the board, hit refresh, get a new program.

![k2g in use — the board in KiCad, then the generated program, tooling, rack schedule, drill map and 3D toolpath]
(https://raw.githubusercontent.com/adarwoo/k2g/main/assets/media/k2g-tour.gif)

<sub>A board in KiCad, then k2g: the generated program, the tooling and rack schedule,
the drill map, the toolpath in 3D, and the profiles that drive it.</sub>

> [!WARNING]
> **Pre-release.** k2g generates code that drives a spindle. Read every program before
> you run it, and air-cut anything new. See [Status](#status) for what is unfinished.

---

## What it does

**Drilling**

- Plated and non-plated holes, grouped by finished size, with plating allowance
- Oblong / slotted holes, either as an overlapping drill chain or routed as a slot,
  carrying the pad's true orientation
- Picks real tools from your stock, and says so when nothing in stock can make a feature
- Schedules the tool rack — ATC slots where the machine has them, prompted manual
  changes where it does not
- Feeds and speeds derived per tool, clamped to the spindle's real range

**Routing**

- Board outline with breakaway tabs, distributed over the outline's sides (longest side
  gets the most), with optional mouse bites
- Internal cutouts
- Optional finishing pass

**The machine is yours to describe**

- **CNC profiles** hold the dialect. Every GCode word k2g emits comes from a template you
  can edit — see the [GCode template language](schemas/docs/gcode-template-language.md).
  No GCode is hardcoded in the application, so an unusual controller is a profile, not a
  patch.
- **Fixture profiles** hold the physical setup: which of the machine's stored zeros the
  fixture sits in, which board corner is X0/Y0, backing board, retract and safe heights.
- **Toolset profiles** hold what is loaded in the rack.
- **Machining profiles** are an ordered list of steps, each binding one CNC, one fixture
  and one toolset.

**Seeing it before you cut**

- **Board** — the stitched outline, holes and slots
- **3D** — the actual toolpath, coloured per tool, over a solid board
- **Code** — the generated program
- **Tooling** and **Rack** — which tool makes which feature, and where it lives

## Status

k2g does its main job — drill a board and cut it out — and is being tested on real
hardware. What is **not** done:

| Area | State |
| --- | --- |
| **Bottom-side machining** | Selectable per step, but **refused**: no geometry is mirrored yet, so k2g blocks generation for a bottom-side step rather than emit a top-side program for it. |
| Copper isolation milling | Not started |
| Engraving | Not started; planned for from the outset |
| Tab nudging | Tabs land where the distribution algorithm puts them. The job stores per-tab offsets, but nothing sets them yet. |
| Arc-preserving offsets | Outline offsets tessellate arcs |

## Requirements

- **Windows** — the UI renders in WebView2. Linux runs (the UI renders in
  WebKitGTK) but is not covered by the release builds; see
  [Linux](#linux) for the GPU caveat.
- **KiCad 9 or later**, running, with a board open and the IPC API enabled.
  k2g can enable it for you — *settings cog → KiCad integration*.
- **Rust** (stable) to build from source.

## Install

Take the latest [release](https://github.com/adarwoo/k2g/releases): an `.msi` or
`-setup.exe` (per-user, no admin rights) or a portable `.zip`. Every artifact is
signed with [minisign](https://jedisct1.github.io/minisign/) and can be verified
against `assets/release-signing.pub`.

Full instructions — connecting to KiCad, registering the toolbar button, updates,
and how to remove everything — are in
**[docs/install-and-security.md](docs/install-and-security.md)**.

### As a KiCad plugin

*Settings cog → KiCad integration → Register with KiCad* adds a **Create GCode** button to
the PCB editor toolbar. Pressing it opens k2g with that board already loaded, since
KiCad hands the plugin its API socket directly. Reversible from the same screen.

### From source

```sh
git clone https://github.com/adarwoo/k2g
cd k2g
cargo run --release
```

CMake is required — `nng-sys` builds the bundled nng C library with it.

#### Linux

The desktop shell is GTK + WebKitGTK, so the build needs their development
packages. On Debian/Ubuntu (the same list CI installs):

```sh
sudo apt-get install -y build-essential cmake pkg-config libglib2.0-dev \
  libgtk-3-dev libwebkit2gtk-4.1-dev libsoup-3.0-dev \
  libjavascriptcoregtk-4.1-dev libssl-dev libxdo-dev
```

`libssl-dev` is needed even though k2g itself speaks TLS through rustls:
`dioxus-desktop` pins `tungstenite` to `native-tls` on every non-Android target
for its hot-reload socket, with no feature to turn it off.

k2g disables WebKit's DMABUF renderer on Linux (see `main.rs`), because on a
driver that can't share buffers the web process dies and the window comes up
blank. If your GPU stack is healthy, `WEBKIT_DISABLE_DMABUF_RENDERER=0` restores
hardware compositing — worth doing for the 3D view. The open-source `nouveau`
driver is known not to be one of the healthy stacks on recent NVIDIA cards: it
was seen rejecting WebKit's GPU command submissions outright (`nouveau: kernel
rejected pushbuf`), which is the failure this workaround exists for.

| Environment variable | Effect |
| --- | --- |
| `RUST_LOG=debug` | Verbose logging, also visible in the in-app **Logs** screen |
| `KICAD_API_SOCKET` | Use an explicit KiCad IPC socket instead of discovering one. Set automatically when KiCad launches k2g as a plugin. |
| `KICAD_API_TOKEN` | API token, likewise set by KiCad when it launches the plugin |
| `K2G_KICAD_SINGLE_INSTANCE=1` | Skip the scan for sibling KiCad instances |

## Privacy and updates in one paragraph

k2g makes exactly one network request — a once-a-day check of the GitHub releases
API — and it can be switched off from the settings cog, after which k2g touches nothing but the
local KiCad socket. There is no telemetry and no analytics. Updates are never
installed without an explicit click, and every installer is signature-checked before
it runs. Details in [PRIVACY.md](PRIVACY.md).

**On multiple KiCad instances:** KiCad serves one fixed API socket, and running instances
are not individually addressable over it. With several KiCads open, k2g talks to whichever
one owns the socket.

## Documentation

| Document | What it covers |
| --- | --- |
| [Install and security](docs/install-and-security.md) | Installing, connecting to KiCad, updating, uninstalling — and the safety warning |
| [Privacy](PRIVACY.md) | What is stored, what leaves the machine (almost nothing) |
| [Security policy](SECURITY.md) | Reporting a vulnerability; what is supported |
| [Specification](schemas/docs/Specification.md) | What the application is meant to do |
| [Architecture](schemas/docs/architecture.md) | How it is put together |
| [GCode template language](schemas/docs/gcode-template-language.md) | Writing CNC profile templates |
| [GCode engine](schemas/docs/gcode-engine.md) | How a program is assembled |
| [Operation planner](schemas/docs/operation-planner.md) | Tool selection, ordering, placement |

## Contributing

Issues and pull requests are welcome. The schemas under `schemas/` are the source of
truth for persisted data and for much of the UI — read
[architecture.md](schemas/docs/architecture.md) before changing one.

## Licence

Copyright © 2026 Bill Arreckx.

k2g is free software: you can redistribute it and/or modify it under the terms of the
**GNU General Public License, version 3**, as published by the Free Software Foundation.

It is distributed in the hope that it will be useful, but **WITHOUT ANY WARRANTY**;
without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.
See [LICENSE](LICENSE), or <https://www.gnu.org/licenses/gpl-3.0.html>, for the full
terms.

In short: use it, change it, share it. If you distribute a modified version, it must also
be GPLv3 and its source must be available.

Bundled third-party components keep their own licences —
[kicad-ipc-rs](third_party/kicad-ipc-rs) (MIT, vendored fork) and
[three.js](assets/vendor) (MIT).

# K2G — KiCad to GCode

**You design in KiCad, you have a CNC, you're 3 clicks away from making the board!** 
K2G reads the board open in KiCad and writes the
GCode that drills and cuts it out: holes, oblong slots, board outline with breakaway
tabs.

<img width="1887" height="1176" alt="image" src="https://github.com/user-attachments/assets/9390f33c-4419-41e0-822c-5f4310a6f827" />

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
- Internal cutouts, each cut with a cutter chosen to fit it, its slug held and its sharp
  corners relieved
- Optional finishing pass
- Curved edges come out as `G2`/`G3`: the offset runs on a polyline, and arcs are fitted
  back to it within the profile's curve tolerance

**Copper isolation**

- Isolation routing at the channel width you ask for; the V-bit is chosen to suit and the
  depth it needs derived from it, so depth is never asked for
- Where two nets are closer together than that, the pass narrows across just that stretch
  and names the nets it narrowed — it never widens, and never cuts into a neighbour
- Contours are worked out on a thread of their own, so the views stay live while a dense
  board is read

**Two-sided work**

- Locating pins drilled through the board and on into the backboard, on the fixture's
  flip line
- Back-face steps mirrored about that line, and the program opens by asking the operator
  to confirm the board is turned over
- The order is enforced rather than assumed: pins first, on the front, and no face change
  without registration drilled before it

**The machine is yours to describe**

- **CNC profiles** hold the dialect. Every GCode word k2g emits comes from a template you
  can edit — see the [GCode template language](docs/design/gcode-template-language.md).
  No GCode is hardcoded in the application, so an unusual controller is a profile, not a
  patch.
- **Fixture profiles** hold the physical setup: which of the machine's stored zeros the
  fixture sits in, which board corner is X0/Y0, backing board, retract and safe heights.
- **Toolset profiles** hold what is loaded in the rack.
- **Machining profiles** are an ordered list of steps, each binding one CNC, one fixture
  and one toolset.

**Seeing it before you cut**

- **Board** — the stitched outline, holes, slots and copper, with the legend doubling as
  the layer control
- **3D** — the actual toolpath, coloured per tool and switchable tool by tool, over a
  solid board
- **Code** — the generated program
- **Tooling** and **Rack** — which tool makes which feature, and where it lives

## Status

k2g does its main job — drill a board, cut it out, and isolate the copper, on either
face — and is being tested on real hardware. What is **not** done:

| Area | State |
| --- | --- |
| Engraving | Not started. Copper *isolation* is done; cutting lettering, fiducials or any other mark is not. |
| Tab nudging | Tabs land where the distribution algorithm puts them. The planner honours a stored per-tab offset, but nothing in the UI sets one yet. |
| Arc-preserving offsets | The outline offset is a polygon operation, so it tessellates. Arcs are fitted back before emission, which is exact to the CNC profile's curve tolerance rather than to the original curve. |

## Requirements

- **KiCad 9 or later**, running, with a board open and the IPC API enabled.
  k2g can enable it for you — *settings cog → KiCad integration*.
- **A desktop platform with a webview**, one of:

| | State | The UI renders in |
| --- | --- | --- |
| **Windows** | Packaged — the release builds are Windows | WebView2 |
| **Linux** | Packaged — `.deb`, `.AppImage` and a portable tarball for x86_64 with each release, built on Ubuntu. Verified against the running application on Debian 13 (GTK 3 + WebKitGTK 2.52, KiCad 10 over IPC). Older distributions and other architectures [build from source](#from-source). | WebKitGTK |
| **macOS** | Packaged — every change is built and tested on both Apple Silicon and Intel, and each release carries a build for each. Nobody has yet driven the UI on a Mac, so reports welcome. | WKWebView |

- **Rust** (stable) to build from source, which is the route on an older distribution
  or an architecture no release covers.

Two features vary by platform, and both degrade rather than break. Exporting straight to
a removable medium and ejecting it needs the Win32 volume API, so on Linux and macOS
the Export button behaves as though nothing is plugged in. And k2g can tell whether
KiCad is running on Windows and Linux but not on macOS — no `/proc` there, and
shelling out to `ps` to answer a question you can answer by looking at your dock is a
poor trade — so on a Mac the KiCad integration card warns before editing KiCad's
settings rather than refusing.

## Install

On Linux (x86_64), take the `.deb`, the `.AppImage` or the portable `.tar.gz` from the
latest [release](https://github.com/adarwoo/k2g/releases). They are built on Ubuntu, so
an older distribution — or another architecture — [builds from source](#from-source).

On macOS, take the build for your Mac from the latest
[release](https://github.com/adarwoo/k2g/releases) — `arm64` for Apple Silicon,
`x86_64` for Intel. They are unsigned, so clear the quarantine flag once with
`xattr -dr com.apple.quarantine` on the app or the extracted binary; see
[install-and-security.md](docs/install-and-security.md#macos).

On Windows, take the latest [release](https://github.com/adarwoo/k2g/releases):

| File | What it is |
| --- | --- |
| `K2G_<version>_x64-setup.exe` | Installs for you alone, into `%LOCALAPPDATA%`, with no administrator rights. The one to take. |
| `K2G_<version>_x64.msi` | The same application, installed for every user on the machine. Needs administrator rights. |
| `k2g-<version>-portable-windows-x64.zip` | Unzip and run — the executable embeds every schema, catalog, stylesheet and script it needs. Keep `k2g-kicad-launcher.exe` beside `k2g.exe` for the KiCad toolbar button. |
| `k2g-<version>.cdx.json` | The CycloneDX software bill of materials for that build. |

> [!NOTE]
> **Every artifact is signed** with [minisign](https://jedisct1.github.io/minisign/) and
> carries its `.minisig` beside it. k2g's updater checks that signature before it will
> run an installer and refuses one it cannot verify, so there is nothing you need to do
> — see [Signing key](#signing-key) to check by hand.

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

#### macOS

Nothing to install but the Xcode command-line tools and CMake — the webview is
WKWebView, which is part of the system, so there is no equivalent of the package
list above. k2g resolves the Mac locations for everything it touches: KiCad's
configuration in `~/Library/Preferences/kicad`, its plugin directory under
`~/Documents/KiCad`, and k2g's own data in `~/Library/Application Support/k2g`.

Built and tested on both architectures by CI, so it compiles and its tests pass — but
CI has no screen, and nobody has yet opened a window on a Mac. That is the part still
worth a report: if you run it, say how it went.

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
it runs — which is exactly why the unsigned releases above have to be installed by
hand. Details in [PRIVACY.md](PRIVACY.md).

**On multiple KiCad instances:** KiCad serves one fixed API socket, and running instances
are not individually addressable over it. With several KiCads open, k2g talks to whichever
one owns the socket.

## Documentation

| Document | What it covers |
| --- | --- |
| [User manual](docs/user-manual.md) | Every screen and setting, from first run to exporting a program |
| [Install and security](docs/install-and-security.md) | Installing, connecting to KiCad, updating, uninstalling — and the safety warning |
| [Privacy](PRIVACY.md) | What is stored, what leaves the machine (almost nothing) |
| [Security policy](SECURITY.md) | Reporting a vulnerability; what is supported |
| [Specification](docs/design/Specification.md) | What the application is meant to do |
| [Architecture](docs/design/architecture.md) | How it is put together |
| [GCode template language](docs/design/gcode-template-language.md) | Writing CNC profile templates |
| [GCode engine](docs/design/gcode-engine.md) | How a program is assembled |
| [Operation planner](docs/design/operation-planner.md) | Tool selection, ordering, placement |
| [Schema versioning](docs/design/schema-versioning.md) | How a file written by an older release reaches a newer one |

## Contributing

Issues and pull requests are welcome. The schemas under `schemas/` are the source of
truth for persisted data and for much of the UI — read
[architecture.md](docs/design/architecture.md) before changing one.

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

## Signing key

Release artifacts are signed with minisign. This is the public half — the same key that
is compiled into every k2g build as the one thing its updater trusts, and the reason the
updater does not have to trust TLS, GitHub's account security or this page:

```
untrusted comment: minisign public key 06B8B6495DD89857
RWRXmNhdSba4BtwDZDRNbvFIpLLW4dHBanW4oe0v8oc+M/z5qX7mcay4
```

To check a download by hand:

```
minisign -Vm k2g-<version>.msi -P RWRXmNhdSba4BtwDZDRNbvFIpLLW4dHBanW4oe0v8oc+M/z5qX7mcay4
```

A failed check means the file is not what was published — do not run it. The same key is
in the repository at [assets/release-signing.pub](assets/release-signing.pub), which is
the copy the application compiles in; the release workflow verifies each signature it
makes against that file, so a build signed with any other key fails rather than
publishing installers no updater would accept.

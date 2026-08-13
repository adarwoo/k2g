# Installing, running and retiring k2g

Practical instructions for a user rather than a developer: how to get k2g onto a
machine, how it connects to KiCad, how updates work, and how to remove it. It also
serves as the user information Annex II of the EU Cyber Resilience Act asks a product
to supply.

- Vulnerability reporting and the support commitment: [SECURITY.md](../SECURITY.md)
- What is stored and what leaves the machine: [PRIVACY.md](../PRIVACY.md)

## What this is, and the risk that comes with it

k2g reads a PCB from a running KiCad and generates CNC machine code — G-code — for
drilling, routing and profiling it. It is for people who machine their own boards on a
desktop CNC.

> **The programs k2g produces drive a physical machine.**
>
> A spindle turning at 20,000 rpm with a 0.8 mm carbide drill in it will break tools,
> ruin work, and throw fragments if the program is wrong about where the stock is or
> how thick it is. k2g works from geometry it infers from the board and from profiles
> **you** wrote describing **your** machine, fixture and tooling. It cannot see your
> actual setup and does not know when a profile is wrong.
>
> Before running any program: check the toolpath and the depths, confirm the work
> origin and stock thickness against the machine, confirm the tool in the spindle is
> the tool the program expects, and dry-run above the work if you can. Keep your hand
> near the feed hold.
>
> k2g is supplied with no warranty of any kind (GPL-3.0, §§15–16). The operator owns
> the outcome.

Known limitations that matter for safety are listed under **Status** in the
[README](../README.md) — most importantly, bottom-side machining is refused rather
than mis-generated.

## Manufacturer

| | |
|---|---|
| Author | Bill Arreckx |
| Contact | software@arreckx.com |
| Source | https://github.com/adarwoo/k2g |
| Licence | GPL-3.0-only |
| Product | k2g — KiCad to G-code |
| Version | shown on the **About** screen, and in the header of every generated program |

k2g is free software published by an individual, not a commercial product. It is not
CE-marked and has no EU Declaration of Conformity — see the regulatory note in
[SECURITY.md](../SECURITY.md).

## Installing

### Windows

Take the latest [release](https://github.com/adarwoo/k2g/releases) and either:

- **`k2g-<version>.msi`** or **`k2g-<version>-setup.exe`** — installs per-user into
  `%LOCALAPPDATA%`, no administrator rights needed, and appears in Apps & features.
  This is the one to pick if you want the in-app updater to work smoothly.
- **`k2g-<version>-portable-windows-x64.zip`** — unzip and run. The executable embeds
  everything it needs, so there is nothing else to place. Keep
  `k2g-kicad-launcher.exe` beside `k2g.exe` if you want the KiCad integration.

### Verifying what you downloaded

Every artifact has a detached `.minisig` signature. You do not have to check it by
hand — k2g's own updater always does — but for a first install from a fresh download:

```
minisign -Vm k2g-<version>.msi -P RWQ...    # the key in assets/release-signing.pub
```

A failed check means the file is not what was published. Do not run it.

### Other platforms

Linux and macOS build from source (`cargo build --release`) but are not currently
tested or packaged. The Windows-only parts are removable-media handling and the KiCad
process check; everything else is portable.

## Connecting to KiCad

k2g talks to KiCad over KiCad's IPC API. That API is **switched off** in a stock KiCad
and has to be turned on once.

### The easy way

Open k2g → the **settings cog** in the top bar → **KiCad integration**. It lists every KiCad version it finds
and offers two buttons per version:

- **Enable the KiCad API** — sets `api.enable_server` in that version's
  `kicad_common.json`. It writes a `.k2g-backup` copy first, changes nothing else, and
  **refuses while KiCad is running**, because KiCad rewrites that file when it exits
  and would discard the change. Close KiCad, press the button, start KiCad again.
- **Register with KiCad** — installs a small plugin directory so KiCad shows a
  **Create GCode** button on the PCB editor toolbar. Pressing it opens k2g with that
  board already loaded and connected. Restart KiCad after registering.

Both are reversible from the same screen, and both are recorded in the security log.

### By hand

*Preferences → Plugins → Enable KiCad API*, then restart KiCad. Then start k2g with a
board open.

### What the registration puts on disk

```
Documents/KiCad/<version>/plugins/k2g/
    plugin.json                the manifest KiCad reads
    k2g-kicad-launcher.exe     a ~350 KB shim
    k2g-target.txt             the path of your installed k2g
    icon.png                   the toolbar icon
```

The shim exists because KiCad only launches programs from inside the plugin directory
— it rejects an absolute path in the manifest. Copying the whole 50 MB application
there instead would leave a second, separately-ageing copy of k2g that keeps working
after the real one is patched, which is exactly the situation a security update is
supposed to end. The shim reads `k2g-target.txt` and starts the installed build, and
k2g rewrites that file at startup whenever it notices it has moved — so updating k2g
keeps the toolbar button working with nothing to redo.

## Updates

k2g checks GitHub once a day for a newer release. This is on by default and is the
only network request it ever makes.

When a release is found, a banner offers four choices: **Install**, **Remind me
later** (7 days), **Skip this version**, or **Turn off update checks**. Nothing
downloads or installs without the explicit click.

**Install** downloads the installer *and its signature*, verifies the signature
against a public key compiled into the running build, and only then runs it. A
download that fails verification is deleted and reported; k2g will not run an
unverified installer, and a release published without a signature is treated as
broken rather than as permission to skip the check.

Close k2g when the installer starts — it cannot replace a running executable.

To turn checking off: the **settings cog → Updates**. k2g then makes no network requests at
all and you update manually from the releases page. Doing so does not disable
anything else.

## Operating securely

- **Treat profiles and catalogs like code.** Machining profiles carry
  [Rhai](https://rhai.rs) templates that k2g executes to emit G-code. A profile from
  someone else runs their code on your machine and produces the program your spindle
  follows. Read one before you use it, exactly as you would a shell script.
- **Keep the API off when you are not using it.** The IPC API lets any local program
  drive KiCad. Turning it off between sessions is reasonable if that matters to you;
  k2g can turn it back on in one click.
- **Take updates.** Fixes reach you no other way — there is no backport branch.
- **Check the security log** if something surprises you: Logs → *Security*. It records
  what k2g changed outside its own directory, and when.

## Where your data lives

| Platform | Directory |
|---|---|
| Windows | `%APPDATA%\k2g` |
| macOS | `~/Library/Application Support/k2g` |
| Linux | `$XDG_CONFIG_HOME/k2g`, else `~/.config/k2g` |

Holding `configs/` (settings, profiles, stock, job), `catalogs/` (tool catalogs) and
`logs/` (the security record). Back this directory up and you have backed up
everything k2g knows.

## Uninstalling

1. **Unregister the KiCad plugin**, if you registered it: settings cog → KiCad integration
   → *Unregister*, for each version. This is the only thing k2g puts outside its own
   directory, and the uninstaller does not touch it.
2. **Turn the KiCad API back off** if you only enabled it for k2g — same card.
3. **Delete your data**: settings cog → Data and reset → *Delete all data*. This removes
   the directory above in full and closes k2g. Skip it if you plan to reinstall and
   want your profiles back.
4. **Remove the application**: Windows Settings → Apps → k2g → Uninstall, or delete
   the portable folder.

On deletion being "secure": these steps delete files rather than overwriting storage,
and on an SSD overwriting would not reliably destroy the data anyway — see the
explanation in [PRIVACY.md](../PRIVACY.md). k2g retains nothing afterwards; guaranteed
physical erasure is a whole-disk concern.

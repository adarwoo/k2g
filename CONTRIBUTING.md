# Contributing to k2g

Thanks for wanting to help.

k2g emits programs that drive a physical machine. That shapes everything below: the bar
for "works" is not that it compiles, it is that someone ran it on real hardware and the
board came out right.

## The most useful contributions are not code

**CNC profiles.** A profile is a YAML document describing one machine's limits and its
G-code dialect — no Rust involved. Every machine k2g does not yet ship a profile for is
someone unable to use it out of the box, and you are far better placed to write one for
your machine than anybody who does not own it. Start from the closest bundled profile in
[`assets/cnc_templates/`](assets/cnc_templates/), read the
[GCode template language reference](docs/design/gcode-template-language.md), and open a
**Machine profile request** issue if you would rather describe the machine than write the
document.

**Tool catalogs.** Likewise data: [`assets/catalogs/`](assets/catalogs/). One rule, and it
is absolute — **every parameter must come from a vendor datasheet or from your own measured
results.** A plausible-looking guess in a catalog is a broken tool in somebody's spindle,
and it is worse than a missing entry because it looks authoritative. Say in the pull
request where the numbers came from.

Both are testable only by running them. A profile or catalog nobody has cut with is not
ready to bundle, and saying "untested" in the pull request is a normal and welcome thing
to do.

## Code

```
cargo build --workspace
cargo test --workspace
cargo check --workspace --all-targets --release
```

That last one is not redundant. Dioxus's `rsx!` expands differently with
`debug_assertions` off, so markup that compiles in debug can fail in release — the release
check is what catches it before a tag does.

Linux needs GTK3, WebKitGTK 4.1 and CMake; the exact package list is in
[install-and-security.md](docs/install-and-security.md#linux).

### What the codebase expects of a change

- **Comments say *why*, not *what*.** The existing ones explain the reasoning behind a
  decision, the failure that motivated it, or the trap that made the obvious approach
  wrong. Read a few before writing new ones — matching that is the main thing a reviewer
  will look for. Private functions get doc comments too.
- **A test that would have caught the bug.** Not coverage for its own sake: the test that
  fails before the fix and passes after.
- `cargo fmt` and `clippy` run in CI but do **not** gate. The workspace carries pre-existing
  warnings, several in the vendored `third_party/kicad-ipc-rs` fork that is deliberately
  kept close to upstream. Don't reformat files you aren't otherwise touching — it buries
  the change under diff noise.

## Reporting things

Bugs and machine-profile requests have [issue forms](.github/ISSUE_TEMPLATE/) that ask for
the specific artifacts needed to reproduce — exported profiles above all, since almost
every generation bug is only reproducible with the profiles that produced it.

**Security vulnerabilities go privately**, as a draft advisory — see
[SECURITY.md](SECURITY.md), which also explains why a program that damages a workpiece is a
bug rather than a vulnerability.

## Licence

k2g is GPL-3.0-only. Contributions are accepted under the same licence.

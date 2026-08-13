# Security policy

## Reporting a vulnerability

Please report privately, not as a public issue.

- **GitHub Security Advisories** (preferred) — [open a draft advisory](https://github.com/adarwoo/k2g/security/advisories/new). This keeps the report private until a fix is published and gives you credit on the advisory.
- **Email** — software@arreckx.com, with `k2g security` in the subject.

Useful to include: the version (About screen, or the `k2g_version` line in any generated G-code), the platform, what an attacker gains, and the smallest reproduction you have. A KiCad board file or a profile YAML that triggers it is worth a great deal.

### What to expect

| | Target |
|---|---|
| Acknowledgement | 5 working days |
| Initial assessment | 10 working days |
| Fix released | 90 days from the report, sooner where practical |
| Public disclosure | when the fix ships, or at 90 days by agreement |

k2g is maintained by one person, not a team with a rota. If a deadline is going to slip you will be told rather than left waiting.

Fixed vulnerabilities are published as GitHub Security Advisories against this repository and named in the release notes. That is the disclosure route; there is no separate mailing list.

### Scope

In scope: anything in this repository, including the vendored `third_party/kicad-ipc-rs` fork.

Particularly interesting, because these are where untrusted input meets the application:

- **Profiles, catalogs and stock files.** YAML from elsewhere, parsed and schema-validated on load, and carrying [Rhai](https://rhai.rs) templates that the G-code generator executes. A profile that escapes the sandbox, or reads or writes files, is a serious finding.
- **The update channel.** k2g downloads an installer and runs it. Anything that gets an unsigned or substituted artifact past the minisign check in `runtime::update` is the most severe class of bug this project can have.
- **The KiCad integration.** `runtime::kicad_integration` writes into another application's directories and edits its configuration file.
- **Board data over IPC.** Geometry arrives from KiCad and is stitched and offset.

Out of scope:

- **G-code that damages a machine or workpiece.** k2g emits programs for a CNC machine; verifying a program against the real fixture, stock and tooling is the operator's job and always will be. A generator bug producing a wrong toolpath is a bug — report it as one — but it is not a vulnerability.
- Anything requiring an attacker who already has your user account.
- Findings from automated scanners with no demonstrated impact.

## Supported versions

Only the latest release. k2g has no long-term-support branch: fixes go into the next release, and the in-app update check (on by default) is how they reach you.

| Version | Supported |
|---|---|
| Latest release | Yes |
| Anything older | No — update |

## Support period

k2g is free software maintained as a personal project, offered with no warranty (see the GPL-3.0 licence, §§15–16). There is no contractual support commitment and no guaranteed end-of-support date.

The intent, stated so you can plan around it rather than as a promise: security fixes for as long as the project is maintained, and **at least five years** from a release. If the project is retired, that will be announced in the repository and in the release notes, with the date support ends.

Because k2g is GPL-3.0, the practical floor is stronger than a promise from one maintainer: the source is public and forkable, so the ability to fix it does not depend on this repository continuing.

## How k2g protects you

- **Signed updates.** Every release artifact carries a detached [minisign](https://jedisct1.github.io/minisign/) signature. k2g verifies it against a public key compiled into the executable *before* running an installer; a download that fails verification is deleted, not offered. The root of trust is that key, not TLS and not GitHub. Verify by hand with `minisign -Vm <file> -P <key from assets/release-signing.pub>`.
- **Software bill of materials.** Every release ships `k2g-<version>.cdx.json`, a CycloneDX SBOM of the full dependency graph.
- **Dependency monitoring.** `cargo audit` against the RUSTSEC database and `cargo deny` run on every pull request and weekly on a schedule; Dependabot proposes updates.
- **Almost no network surface.** The update check is the only outbound request k2g makes, and it can be switched off from the settings cog. Schema validation is pinned to bundled schemas and refuses to fetch a remote `$ref`.
- **A local security record.** Update checks and installs, KiCad integration changes, rejected configuration and G-code writes are recorded to `logs/security.jsonl`. It never leaves the machine and holds no personal data. See [PRIVACY.md](PRIVACY.md).

## Regulatory note

k2g is developed to the technical measures in Annex I of the EU Cyber Resilience Act (Regulation (EU) 2024/2847) as a matter of engineering practice.

It is **not** CE-marked and carries no EU Declaration of Conformity. As free and open-source software supplied outside the course of a commercial activity, k2g falls outside the CRA's scope (recitals 18–19); the measures are implemented because they are worth having, not because they are owed. Anyone distributing k2g commercially takes on the conformity obligations themselves.

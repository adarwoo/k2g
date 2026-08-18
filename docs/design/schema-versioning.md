# Schema versioning — how an old file reaches a new build

Status: design proposal, 2026-08-18. No code yet. Companion to
[data-api.md](data-api.md) and [architecture.md](architecture.md).

## Why now

Everything k2g persists is a document validated against a JSON Schema in
[schemas/](../../schemas). Today those schemas may change freely, because every k2g in
existence is a pre-release and the only files in the world are on machines whose owner is
also the person changing the schema.

1.0 ends that. From then on, a schema change has to answer a question it has never had to
answer: *what happens to the file that is already on the operator's disk?* This document
proposes the answer, and the rules that decide when the question even arises.

## Where it stands

### The gate

`parse_document` refuses a document whose `schema_version` is not exactly the schema's
`x-schema-version` — in **either** direction
([parse.rs:75](../../crates/datastore/src/parse.rs#L75)):

| the file says | what happens | the message |
|---|---|---|
| nothing | dropped | `missing schema_version (schema is version N)` |
| newer than the schema | dropped | `file schema_version M is newer than supported N; rejected` |
| older than the schema | dropped | `file schema_version M is older than N; upgrade not yet supported` |

"Dropped" means `parse_document` returns `None`: the document is not in the store, the
application runs without it, and the file on disk is untouched. A rejected machining
profile is a profile that has vanished from the screen.

The crate says as much about itself — *"a newer file is rejected, and (for now) an older
file is rejected too — an in-place upgrade path is planned"*
([lib.rs:85](../../crates/datastore/src/lib.rs#L85)).

### What stands in for an upgrade path

The application compensates in `src/data/mod.rs`, before the gate ever sees the data. Each
document kind gets a *normaliser* that rewrites an old **shape** and stamps the current
version:

| normaliser | what it repairs |
|---|---|
| `inject_schema_version` | a file with no `schema_version` at all |
| `normalize_machining_value` | the pre-v3 flat setup, wrapped into `steps[0]`, and the version stamped to 3 |
| `normalize_step_value` | retired `enabled` flags, empty-string refs, the old `route_board` edge-only shape |
| `migrate_stock_ref` | the nested `ref: { catalog: … }` object flattened to `source_catalog` |
| `materialize_stock_overrides` | a stock tool whose `overrides` lack their `base` fields |
| `normalize_cnc_value`, `normalize_fixture_value` | the same, per profile kind |

They work, they are idempotent, and they are the reason a 0.x file still opens. What they
are not is a *versioned* upgrade path:

- **They are keyed on shape, not on version.** Each detects "the old way" by a missing key
  and collapses every historical form directly to current. There is one rung, however many
  releases have passed.
- **Nothing is kept.** The upgraded document is written back over the original at the next
  edit. A migration that gets it wrong has destroyed the evidence.
- **Nothing is tested per version.** There is no file at version 1 in the repository that
  CI opens; the only proof a v1 file still loads is that nobody has reported otherwise.
- **They are invisible.** An upgraded document produces no record. The operator is not
  told that the file they have been keeping since March was rewritten.

### Two discrepancies to fix first

1. **`catalog.yaml` declares `schema_version: 1`, not `x-schema-version: 1`**
   ([catalog.yaml:3](../../schemas/catalog.yaml#L3)). The gate looks up `x-schema-version`,
   so for catalogs it never engages at all — a catalog's version is enforced only by the
   ordinary `const: 1` validation, which reports a different error kind and does not drop
   the file. Meanwhile
   [catalog_normalizer.rs:59](../../src/catalog_io/catalog_normalizer.rs#L59) states in a
   comment that catalog.yaml declares `x-schema-version: 1`. The code's belief and the file
   disagree, and the belief is the one that reads correctly.
2. **`id.yaml` and `units.yaml` declare `x-schema-version: 1`** but are `$defs`-only
   support files, never parsed as root documents. Harmless, and worth deleting so the
   keyword means one thing.

## The proposal

### 1. A version is a promise about documents, not about schemas

`x-schema-version` goes up when **a document written by the previous release would be
wrong under the new schema**. That is a narrower rule than "the schema changed", and the
distinction is the whole point:

| change | bump? | why |
|---|---|---|
| add an optional property with a `default` | **no** | the old document is still valid; the loader materialises the default |
| add a value to an `enum` | **no** | no existing document holds the new value |
| widen a `pattern`, raise a `maximum` | **no** | everything valid before is valid now |
| add `x-enum-labels`, `title`, `description` | **no** | annotation; nothing about a document changes |
| rename a property | **yes** | the old key is now unknown |
| remove a property | **yes** | unless it was optional and ignoring it is acceptable |
| change a property's type or unit | **yes** | the stored value no longer decodes |
| tighten a constraint | **yes** | a document that was valid may not be |
| add to `required` | **yes** | every existing document lacks it |

Two recent changes are worked examples of the top half. `export_directories` was added to
`settings.yaml` as an optional array with `default: []` and deliberately kept out of
`required` — no bump. `x-enum-labels` added a keyword to eight schemas and changed no
document — no bump.

### 2. Migrations are a chain, one rung per version

Replace "detect the old shape, collapse to current" with an explicit ladder per document
kind:

```rust
// One entry per version boundary. `from` is the version the document declares.
const MACHINING_MIGRATIONS: &[Migration] = &[
    Migration { from: 1, to: 2, apply: v1_to_v2 },
    Migration { from: 2, to: 3, apply: v2_to_v3 },  // the flat setup → steps[0]
];
```

Applied in order from the file's declared version to the schema's, on the raw
`serde_json::Value`, **before** validation — so the gate only ever sees a current
document, and each rung is a small function with one job. The normalisers above become the
rungs they always implicitly were.

The chain's shape is what buys the long tail: a file from 1.0 reaching a 1.7 build climbs
six rungs it has never seen, and each of them was tested when it was written.

### 3. The original is kept, once

Before the first write of an upgraded document, copy the file to `<name>.v<n>.bak`. Once —
not per write, not per session.

This is the cheapest possible insurance against the failure that matters: a migration that
is *wrong* rather than absent, discovered a week later. It also makes downgrading possible,
which the version gate otherwise forbids outright (a newer file is rejected, so an operator
who tries an older build finds an empty application).

### 4. A refusal is visible

Newer-than-supported stays a refusal — a build cannot guess what a later release meant.
But it must be *said*: today a dropped document is a `warn!` line and an empty screen, and
the two do not obviously belong to each other.

The security log already separates the two outcomes, since
`config.rejected` / `config.problem` were split apart: rejected means the document is not
in use, a problem means it loaded with a complaint. The remaining half is on screen — a
document dropped at load should raise the diagnostics banner naming the file and the
reason, in the same place every other blocking fault appears.

### 5. Fixtures, and a test per rung

For each document kind, keep one file per historical version under
`crates/datastore/tests/fixtures/versions/` (or the app's own test tree, for app-level
shapes). CI asserts:

- every declared past version has a fixture;
- each fixture migrates to current and then **validates**;
- a fixture at version N migrated to current equals the same document written natively at
  current (the property that keeps a rung honest as later rungs are added);
- a document one version *newer* than the schema is still refused.

A migration without a fixture is a migration nobody has run since the day it was written.

## What this does not cover

- **Catalogs** are supplied by third parties and are read-only. They need the same version
  gate (see the discrepancy above) but not the same upgrade chain: an unreadable catalog is
  re-imported, not migrated.
- **Cross-document migrations** — a change that moves data from one file to another — are
  out of scope. The writer is per-file and atomic per file, so a migration spanning two
  documents can half-happen. If one is ever needed, it needs its own design.
- **Downgrade** beyond the `.bak` copy. The gate refuses a newer file by design and this
  does not change that; the backup exists so an operator is not *stuck*, not so that
  versions become interchangeable.

## Order of work

1. Fix the two declaration discrepancies (`catalog.yaml`, the two support files) — small,
   and independent of everything else.
2. Add the migration registry and chain runner to `datastore`, with the existing
   normalisers moved in as the first rungs.
3. Add the fixtures and the CI assertions.
4. Surface a dropped document in the diagnostics banner.
5. Write the bump rules into `CONTRIBUTING`/`claude.md` so the table above is consulted
   before a schema is edited, not after.

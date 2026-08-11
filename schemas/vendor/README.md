# Vendored third-party schemas

Schemas owned by other projects, copied here so tests can check k2g's output against
the real thing instead of against a hand-written restatement of it. Nothing in this
directory is loaded at runtime, and nothing here is edited by hand — refresh a file
from its source URL and let the tests say whether anything broke.

## `kicad-api.v1.schema.json`

KiCad's IPC API plugin manifest schema — what `plugin.json` is validated against.

- **Source:** <https://go.kicad.org/api/schemas/v1>, which redirects to
  <https://gitlab.com/kicad/code/kicad/-/raw/master/api/schemas/api.v1.schema.json>
- **Retrieved:** 2026-08-11
- **Used by:** `runtime::kicad_integration` tests

Why a copy rather than a fetch: the test must run offline and in CI, and pinning the
schema means an upstream change shows up as a deliberate refresh in a diff rather
than as a build that mysteriously starts failing.

Two constraints this schema does **not** express, both enforced in KiCad's
`common/api/api_plugin.cpp` and both silent when violated — the action is simply
dropped and no button appears:

1. `actions[].entrypoint` must be **relative**. An absolute path is rejected outright
   (`"action contains abs path %s; skipping"`), then normalised against the directory
   holding `plugin.json`.
2. For `runtime.type == "exec"`, the resolved entrypoint must pass
   `wxFileName::IsFileExecutable()`.

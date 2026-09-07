//! Publishing the bundled schemas into the user's data directory.
//!
//! Every schema k2g validates against is compiled into the binary (see [`SCHEMAS`]),
//! which is what makes validation independent of what happens to be on disk. That
//! leaves nothing for a user to read, and reading them is the point: a tool catalog is
//! a file a vendor or an operator writes by hand and drops into `catalogs/`, and
//! `catalog.yaml` is the only complete statement of what such a file may contain.
//! `schemas/` — created but never filled until now — is where they go.
//!
//! # These are copies, not user data
//!
//! A file whose content differs from the embedded one is **overwritten**. Nothing here
//! is ever read back — the validator compiles the embedded text
//! ([`crate::catalog_io::SchemaValidator`]) — so an edit made here was never going to
//! take effect, and a copy left to drift describes a version of k2g that is no longer
//! installed. Overwriting costs a user nothing real and keeps the folder honest about
//! the build that is running. The README written alongside says so in the folder
//! itself, where someone about to edit one will actually see it.
//!
//! Content is compared before writing, so an unchanged set is not rewritten on every
//! launch: the timestamps stay meaningful, and a backup or sync tool watching the data
//! directory has nothing to do.

use std::fs;
use std::path::Path;

use log::{info, warn};

use super::SCHEMAS;

/// Written beside the schemas, explaining what they are to whoever opens the folder.
const README_NAME: &str = "README.md";

/// Deliberately short. The one thing it must convey is that these files are outputs —
/// the natural reading of a schema sitting in a writable folder is that editing it
/// changes what the application accepts, and it does not.
const README: &str = "\
# k2g schemas

k2g writes this folder on every launch from the schemas compiled into the running
build. **Edits here are overwritten on the next start** — the files are here to be
read, not changed. k2g validates against its own built-in copies and never loads
anything from this folder.

Together they describe every YAML file k2g reads or writes. The one to start from is
`catalog.yaml`: a tool catalog is a file you can write by hand and drop into the
`catalogs` folder next to this one. `id.yaml` and `units.yaml` hold the shared
definitions the others reference (identifiers, and the `\"3 mm\"` / `\"1/8 in\"` /
`\"1200 mm/min\"` value forms), so keep them beside whichever schema you point an
editor at.

## Writing a catalog

Every catalog file must name the schema it follows, on a top-level key — a file
without it is skipped:

```yaml
$schema: catalog.yaml
name: My tools
sections:
  - name: Drills
    tools: []
```

For completion and live validation while you type, editors that use the YAML
language server (VS Code's YAML extension, Neovim, Helix, and others) take a comment
on the first line. From a file in `catalogs/`, the schemas are one folder over:

```yaml
# yaml-language-server: $schema=../schemas/catalog.yaml
$schema: catalog.yaml
```
";

/// Writes the bundled schema set into `dir`, creating it if needed.
///
/// Returns how many files were written — zero on the ordinary launch, where the copies
/// already match the build. Every failure is logged and skipped rather than propagated:
/// a data directory that cannot be written is worth a line in the log, but these files
/// are a convenience for authoring and nothing in the application reads them, so
/// failing to place one is no reason to hold up the start.
pub fn ensure_schema_files(dir: &Path) -> usize {
    if let Err(error) = fs::create_dir_all(dir) {
        warn!("Cannot write schemas to '{}': {error}", dir.display());
        return 0;
    }

    let mut written = 0usize;
    for (name, text) in SCHEMAS {
        if write_if_changed(&dir.join(name), text) {
            written += 1;
        }
    }
    if write_if_changed(&dir.join(README_NAME), README) {
        written += 1;
    }

    if written > 0 {
        info!("Published {written} schema file(s) to {}", dir.display());
    }
    written
}

/// Writes `content` to `path` unless it is already exactly that, reporting whether it
/// wrote. An unreadable or absent file counts as different, so the write is attempted.
fn write_if_changed(path: &Path, content: &str) -> bool {
    if fs::read_to_string(path).is_ok_and(|existing| existing == content) {
        return false;
    }

    match fs::write(path, content) {
        Ok(()) => true,
        Err(error) => {
            warn!("Could not write schema '{}': {error}", path.display());
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// The whole set arrives, including the two shared-definition files a hand-written
    /// catalog's `$ref`s point at — a `catalog.yaml` published without `units.yaml`
    /// beside it is one an editor cannot resolve.
    #[test]
    fn every_embedded_schema_is_published() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("schemas");

        let written = ensure_schema_files(&root);
        assert_eq!(written, SCHEMAS.len() + 1, "every schema, plus the README");

        for (name, text) in SCHEMAS {
            let path = root.join(name);
            assert_eq!(
                fs::read_to_string(&path).unwrap_or_default(),
                *text,
                "{name} should be published verbatim"
            );
        }
        for shared in ["units.yaml", "id.yaml", "catalog.yaml"] {
            assert!(root.join(shared).exists(), "{shared} is what a catalog author needs");
        }
        assert!(root.join(README_NAME).exists());
    }

    /// The second launch writes nothing. Rewriting an identical set would churn the
    /// modification times of a folder the user may well have under backup.
    #[test]
    fn an_unchanged_set_is_not_rewritten() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("schemas");

        assert!(ensure_schema_files(&root) > 0, "first launch publishes");
        assert_eq!(ensure_schema_files(&root), 0, "second launch has nothing to do");
    }

    /// A copy that has drifted — an edit, or a leftover from a previous version — is
    /// replaced. It is a copy of what this build validates against, and a stale one
    /// describes a k2g that is no longer installed.
    #[test]
    fn a_stale_copy_is_refreshed() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("schemas");
        ensure_schema_files(&root);

        let catalog = root.join("catalog.yaml");
        fs::write(&catalog, "# from an older k2g\n").unwrap();

        assert_eq!(ensure_schema_files(&root), 1, "only the drifted file is rewritten");
        assert_eq!(
            fs::read_to_string(&catalog).unwrap(),
            *SCHEMAS
                .iter()
                .find(|(name, _)| *name == "catalog.yaml")
                .map(|(_, text)| text)
                .unwrap()
        );
    }

    /// The set published is the set the repository holds: a schema added to `schemas/`
    /// but left out of the embedded list would be neither validated against nor
    /// published, and nothing else would say so.
    #[test]
    fn the_embedded_list_covers_the_repository_schemas() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("schemas");
        let embedded: Vec<&str> = SCHEMAS.iter().map(|(name, _)| *name).collect();

        for entry in fs::read_dir(&repo).expect("schemas/ is readable").flatten() {
            let path = entry.path();
            // `vendor/` holds third-party schemas (KiCad's API); they are not part of
            // the data model and have no business in the user's folder.
            if path.is_dir() || path.extension().is_none_or(|ext| ext != "yaml") {
                continue;
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            assert!(
                embedded.contains(&name),
                "schemas/{name} is not in the embedded SCHEMAS list, so it is neither \
                 compiled into the validator nor published to the user's folder"
            );
        }
    }
}

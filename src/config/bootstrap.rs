//! Config bootstrap helpers for embedded schema references.
//!
//! Schema files are reference-only and always refreshed in the user data
//! directory so runtime validation uses the application-matching versions.

use log::{info, warn};
use std::path::Path;

use crate::user_path::{ensure_app_dirs, UserPathError};

macro_rules! embedded_schema {
    ($name:literal, $path:literal) => {
        ($name, include_str!($path))
    };
}

macro_rules! embedded_schemas {
    ($($name:literal => $path:literal),+ $(,)?) => {
        &[
            $(embedded_schema!($name, $path),)+
        ]
    };
}

const SCHEMAS: &[(&str, &str)] = embedded_schemas!(
    "catalog" => "../../resources/schemas/catalog.yaml",
    "settings" => "../../resources/schemas/settings.yaml",
    "cnc" => "../../resources/schemas/cnc.yaml",
    "fixture" => "../../resources/schemas/fixture.yaml",
    "processing" => "../../resources/schemas/machining.yaml",
    "toolset" => "../../resources/schemas/toolset.yaml",
    "stock" => "../../resources/schemas/stock.yaml",
    "id" => "../../resources/schemas/id.yaml",
    "units" => "../../resources/schemas/units.yaml",
);

pub fn write_embedded_schemas() -> Result<(), UserPathError> {
    let dirs = ensure_app_dirs()?;

    for (name, content) in SCHEMAS {
        let dest = dirs.schemas.join(format!("{}.yaml", name));
        match std::fs::write(&dest, content) {
            Ok(_) => info!("Wrote schema reference: {}", dest.display()),
            Err(e) => warn!("Could not write schema '{}': {e}", dest.display()),
        }
    }

    Ok(())
}

/// Seed default files into a directory if they are missing.
///
/// Existing files are preserved. The `after_present` callback runs for each
/// file path that exists after seeding (whether pre-existing or newly created).
pub fn ensure_default_files(
    dir: &Path,
    files: &[(&str, &str)],
    kind_label: &str,
    mut after_present: impl FnMut(&Path),
) {
    for (name, content) in files {
        let dest = dir.join(name);
        if !dest.exists() {
            match std::fs::write(&dest, content) {
                Ok(_) => info!("Created default {}: {}", kind_label, dest.display()),
                Err(e) => {
                    warn!("Could not write {} '{}': {e}", kind_label, dest.display());
                    continue;
                }
            }
        }

        after_present(&dest);
    }
}
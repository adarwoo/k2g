//! Seeding of bundled default files (currently the tool catalogs) into the user
//! data directory. Missing files are created from embedded content; existing
//! files are preserved.

use log::{info, warn};
use std::path::Path;

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

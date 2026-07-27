//! Removable-media stub for the platforms this facility does not implement.
//!
//! It exists so `src/ui` carries no `cfg`: the picker renders an empty list and offers no
//! eject, which is exactly what a Windows machine with nothing plugged in does — so there
//! is no second code path to keep working.
//!
//! The Linux and macOS equivalents are real work rather than missing lines: enumerating
//! mounts from `/proc/mounts` or `/run/media` and testing `/sys/block/<dev>/removable`,
//! then unmounting through `udisksctl` or `diskutil` (neither of which may be installed).
//! An implementation drops in here against the same two functions and nothing else in the
//! app changes.

use super::{EjectError, EjectOutcome, RemovableMedium};

/// Nothing to prepare — the Windows build sets a thread-local error mode here.
pub(super) fn prepare_thread() {}

/// No detection off Windows.
///
/// Deliberately not `unimplemented!()`: the watcher thread calls this on every poll.
pub(super) fn scan() -> Vec<RemovableMedium> {
    Vec::new()
}

/// Always unsupported.
///
/// Unreachable in practice — [`scan`] yields nothing that could be ejected — but the
/// function must exist for the parent module to compile.
pub(super) fn eject(_medium: &RemovableMedium) -> Result<EjectOutcome, EjectError> {
    Err(EjectError::Unsupported)
}

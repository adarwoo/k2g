//! The crate's single public error type.
//!
//! `kicad-ipc-rs` errors are deliberately not re-exported: the whole point of
//! this crate is that the application depends on *it*, not on the IPC client.
//! Every failure that crosses the crate boundary is rendered into a [`PcbError`].

/// A failure acquiring or processing PCB data.
#[derive(Debug, Clone, thiserror::Error)]
pub enum PcbError {
    /// Could not connect to KiCad or a query against it failed.
    #[error("KiCad connection error: {0}")]
    Connection(String),

    /// A board snapshot could not be collected from an open PCB.
    #[error("board snapshot collection failed: {0}")]
    Collect(String),
}

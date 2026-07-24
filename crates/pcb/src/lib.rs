//! PCB acquisition and geometry for k2g.
//!
//! This crate is the one place that knows about KiCad. It connects to KiCad
//! over its IPC API, discovers every running instance, enumerates the PCBs they
//! have open, and collects the subset of each board's geometry that k2g needs
//! into a plain [`BoardSnapshot`] — the **PCB record**. It also owns the
//! edge-cut *stitching* that turns raw outline primitives into closed, nested
//! contours ready for tool-path generation.
//!
//! Everything KiCad-specific (`kicad-ipc-rs`) and every geometry backend
//! (`clipper2-rust`) is contained here: consumers depend on this crate, not on
//! those. The only error type that crosses the boundary is [`PcbError`].
//!
//! # Layers
//!
//! 1. [`KiCad`] — **acquisition.** Connect, enumerate ([`KiCad::enumerate_pcbs`]
//!    → [`PcbInfo`]), and collect ([`KiCad::collect_snapshot`]). Handles the
//!    several-instances case internally.
//! 2. [`BoardSnapshot`] and friends — **the record.** Typed geometry
//!    ([`units::Length`] everywhere, never raw nanometres) for the UI to draw
//!    and the generator to iterate.
//! 3. [`stitch_edge_shapes`] / [`routing_offset`] — **geometry.** Chain the
//!    snapshot's edge shapes into validated [`Contour`]s and offset them by a
//!    tool radius.
//!
//! # Typical flow
//!
//! ```no_run
//! use pcb::KiCad;
//!
//! let kicad = KiCad::connect()?;
//! for info in kicad.enumerate_pcbs()? {
//!     println!("open PCB: {}", info.display_name());
//! }
//! if let Some(board) = kicad.collect_first_snapshot()? {
//!     let stitched = pcb::stitch_edge_shapes(&board.edge_shapes);
//!     assert!(stitched.errors.is_empty(), "board outline is not closed");
//! }
//! # Ok::<(), pcb::PcbError>(())
//! ```

mod error;
mod kicad;
mod snapshot;
mod stitching;

pub use error::PcbError;
pub use kicad::{KiCad, PcbInfo};
pub use snapshot::{
    BoardBoundingBox, BoardEdgeShape, BoardHole, BoardPoint, BoardSnapshot, HoleKind,
};
pub use stitching::{routing_offset, stitch_edge_shapes, Contour, Segment, StitchResult};

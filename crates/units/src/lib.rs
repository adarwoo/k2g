//! Strongly-typed CNC unit primitives.
//!
//! This crate provides the quantity types the application uses to represent
//! physical values entered by the user or read from KiCad and configuration
//! files: [`Length`], [`FeedRate`], [`Angle`], and [`RotationalSpeed`]. Each
//! value preserves the exact form it was authored in — integer, decimal, or
//! fraction, together with its source unit — so that, for example, `1 1/8"`
//! round-trips back to an imperial fraction rather than a bare decimal.
//!
//! # Layering
//!
//! The crate is deliberately split into three modules with a one-way dependency
//! flow, so each concern can evolve independently:
//!
//! 1. `types` — **internal data-type management.** The quantities, their unit
//!    enums, conversions, and the shared string parser. This is the foundation
//!    the other two layers build on; it depends on nothing but `std`.
//! 2. `display` — **UI rendering.** Turns quantities into user-facing strings
//!    and rounded numbers for a chosen [`UserUnitSystem`].
//! 3. `persistence` — **load and save.** serde implementations that read and
//!    write the quantities as they appear in configuration files, mirroring
//!    `schemas/units.yaml`.
//!
//! `display` and `persistence` depend on `types`, never on each other.
//!
//! # No scripting dependency
//!
//! This crate has no dependency on the Rhai scripting engine. Converting a
//! quantity into a script-visible map is the concern of the template renderer,
//! which builds it from the public accessors ([`Length::as_mm`], and so on).
//!
//! # Example
//!
//! ```
//! use units::{Length, UserUnitDisplay, UserUnitSystem};
//!
//! // Parse an imperial fraction; it keeps its source form.
//! let hole = Length::from_string("1/8\"", None).unwrap();
//! assert_eq!(hole.to_string(), "1/8in");
//!
//! // Render it for a metric operator, with the native value annotated.
//! let shown = hole.unit_display(UserUnitSystem::Metric);
//! assert_eq!(shown.user, "3.175mm");
//! assert_eq!(shown.native.as_deref(), Some("0.125in"));
//! ```

mod display;
mod persistence;
mod types;

pub use display::{UnitDisplay, UserUnitDisplay, UserUnitSystem};
pub use types::{
    Angle, AngleUnit, FeedRate, FeedRateUnit, Length, LengthUnit, RotationalSpeed,
    RotationalSpeedUnit, ScalarValue, UnitParseError,
};

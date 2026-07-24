//! GCode generation.
//!
//! Houses the work-in-progress expression/template engine ([`template`]) that
//! renders CNC templates against a job context, and the tool-selection
//! [`assigner`] (Specification.md §8.7). Neither is wired into the live
//! generation path yet; see the notes in each module.

pub mod assigner;
pub mod coder;
pub mod placement;
pub mod plan;
pub mod planner;
pub mod primitive_vars;
pub mod template;

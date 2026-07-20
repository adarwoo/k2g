//! GCode generation.
//!
//! Currently houses the work-in-progress expression/template engine
//! ([`template`]) that renders CNC templates against a job context. It is not
//! yet wired into the live generation path; see the note in `template.rs`.

pub mod template;

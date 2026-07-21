//! The engine's typed error surface. Every failure — a transpile/compile fault, a
//! runtime fault, or a scripted `throw` — is a `GtlError`, never a panic, and (for
//! Parse/Runtime) carries the author-source location so a host can point at the
//! offending line.

use thiserror::Error;

/// A template failure. Mirrors the three cases in `docs/gcode-engine.md` §6:
/// a parse-time fault (GTL transpile *or* Rhai compile), a runtime fault, or a
/// deliberate `throw` precondition.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum GtlError {
    /// A GTL transpile error (e.g. unbalanced `{`) or a Rhai compile error, with
    /// the position mapped back to the author's source (1-based line/column).
    #[error("{template}:{line}:{col}: parse error: {message}")]
    Parse {
        template: String,
        line: usize,
        col: usize,
        message: String,
    },

    /// A Rhai evaluation error: undefined variable, type mismatch, etc. The line
    /// is the author's source line (the transpile is 1:1, so Rhai's line already
    /// points at it).
    #[error("{template}:{line}: runtime error: {message}")]
    Runtime {
        template: String,
        line: usize,
        message: String,
    },

    /// The script called `throw expr` to assert a precondition; `value` is the
    /// thrown value rendered as text.
    #[error("{template}: thrown: {value}")]
    Thrown { template: String, value: String },
}

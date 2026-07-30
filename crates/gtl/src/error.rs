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

    /// The template ran past [`crate::MAX_OPERATIONS`] and was stopped.
    ///
    /// Almost always a loop whose condition never becomes false — the classic being a
    /// `while z > z_bottom` whose body forgets to change `z`. Kept apart from
    /// [`Self::Runtime`] because the cause and the fix are specific, and because Rhai's
    /// own wording for it ("Too many operations") describes the symptom to someone who
    /// has no idea their engine counts operations.
    ///
    /// `line` is where execution had reached, which for a runaway loop is a line *inside*
    /// it — the useful place to look.
    #[error(
        "{template}:{line}: did not finish — stopped after {limit} operations. \
         A loop is most likely never ending: check that its condition can become false \
         (e.g. that the depth in `while z > z_bottom` is actually decreasing)."
    )]
    Runaway {
        template: String,
        line: usize,
        limit: u64,
    },
}

//! # GTL — Generic Template Language
//!
//! A line-oriented scripting engine built on [Rhai]. A *template* is a Rhai
//! program in which every physical line is Rhai **except** lines whose first
//! non-whitespace character is a backtick (`` ` ``); those are **emit lines**,
//! whose text is written to the output with each `{ expr }` evaluated in scope,
//! formatted, and spliced in.
//!
//! ```text
//! `G0 X{x} Y{y}      // emit line  -> emit("G0 X" + fmt(x) + " Y" + fmt(y));
//! let z = z_retract; // rhai line  -> passed through unchanged
//! while z > z_bottom {
//!     `G1 Z{z}       // emit line inside a loop
//! }
//! ```
//!
//! ## Continuing a line
//!
//! An emit line that **also ends** with a backtick writes its text without a trailing
//! newline, so the next emit continues the same output line:
//!
//! ```text
//! `N{line * 10} `    // -> emit_raw("N" + fmt(line * 10) + " ");
//! ```
//!
//! The closing backtick is a delimiter as much as a flag: trailing whitespace sits
//! *inside* it, so it is visible in the source and survives an editor that trims line
//! ends — which is what makes a prefix like the line number above workable. A lone
//! backtick is still an opener with an empty payload, so it emits a blank line as
//! before; `` `` `` emits nothing at all.
//!
//! ## Deliberately domain-agnostic
//!
//! The crate emits *strings*, not GCode. It registers only the language surface —
//! `emit(text)` (what a backtick line compiles to) and a default `fmt(value)` for
//! plain scalars and strings. The **output dialect** — how a host type is
//! formatted, and any domain functions such as `metric()` / `imperial()` — is
//! registered by the host through [`Gtl::engine_mut`] and [`Gtl::writer`]. k2g
//! layers its *GCode* Template Language on top by registering `units`-typed `fmt`
//! overloads and its modal-unit built-ins; the engine here never learns what GCode
//! is. The three-layer scope (program/operation/call), `args!` sugar, and the
//! namespaced job context described in `docs/gcode-engine.md` are that host layer,
//! built on top of [`Gtl::run`].
//!
//! ```
//! use gtl::{Gtl, Scope};
//!
//! let gtl = Gtl::new();
//! let tmpl = gtl.compile("move", "`G0 X{x} Y{y}").unwrap();
//!
//! let mut scope = Scope::new();
//! scope.push("x", 3.2_f64);
//! scope.push("y", 7_i64);
//!
//! assert_eq!(gtl.run(&tmpl, &mut scope).unwrap(), "G0 X3.2 Y7\n");
//! ```
//!
//! [Rhai]: https://rhai.rs

mod error;
mod transpile;

use std::cell::RefCell;
use std::rc::Rc;

use rhai::{Engine, EvalAltResult, ImmutableString, AST};

pub use error::GtlError;
// Re-exported so hosts can build scopes and register dialect without a direct
// `rhai` dependency (though they may add one).
pub use rhai::{self, Dynamic, Scope};

/// A compiled template: a cached Rhai `AST` plus the author-facing name used in
/// diagnostics. Compile once, [`run`](Gtl::run) many — the parse cost is paid
/// once and amortised across the thousands of primitive calls a board produces.
#[derive(Clone, Debug)]
pub struct Template {
    name: String,
    ast: AST,
}

impl Template {
    /// The template's name, as passed to [`Gtl::compile`].
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// A cloneable handle to the engine's output buffer, so host-registered native
/// functions can emit lines that interleave, in order, with a template's own emit
/// lines. Capture it in a registered closure (e.g. a `metric()` that emits `G21`).
#[derive(Clone)]
pub struct Writer(Rc<RefCell<String>>);

impl Writer {
    /// Append `line` to the output, followed by a newline.
    pub fn emit(&self, line: &str) {
        let mut buf = self.0.borrow_mut();
        buf.push_str(line);
        buf.push('\n');
    }

    /// Append `text` with **no** trailing newline, so whatever is emitted next
    /// continues the same output line. What a closing-backtick emit line compiles to.
    pub fn emit_raw(&self, text: &str) {
        self.0.borrow_mut().push_str(text);
    }
}

/// The GTL engine: transpiles + compiles templates and runs them against a Rhai
/// scope, capturing emitted text.
///
/// One engine is built once and reused for a whole run; only the scope changes
/// between [`run`](Gtl::run) calls. The engine is intentionally single-threaded
/// (it holds a shared output buffer via `Rc`); a threaded host would build one per
/// worker and share the immutable [`Template`] ASTs.
pub struct Gtl {
    engine: Engine,
    output: Rc<RefCell<String>>,
}

/// How many Rhai operations one template may execute before it is stopped.
///
/// Templates are **author-written scripts with loops**, so `while z > z_bottom` with the
/// decrement left out does not fail — it runs forever. Without a ceiling that hangs
/// whatever thread called it, and the worst case is not the generation worker (which the
/// user can at least see is busy) but the *editor*: its live preview re-renders on every
/// keystroke, on the UI thread, so one bad loop freezes the application with no way back
/// to the template that caused it.
///
/// Rhai's own default is `0`, meaning unlimited, so this has to be set deliberately.
///
/// The value is chosen from measurement, not taste. Two figures bound it:
///
/// - **What a real template costs.** The most demanding shape a profile writes is a
///   bounded loop — a peck cycle (~20 passes), a drill chain (~50 holes), or the MASSO
///   `set_origin` building its 106-entry offset table. A deliberately extreme 2000-pass
///   loop measures at **~20–30k operations**, already an order of magnitude beyond any of
///   those.
/// - **What a runaway costs before it is stopped.** Measured at ~0.95M operations/second
///   in a debug build and ~14M/s in release. Debug is the number that matters: it is what
///   a developer feels, and it is the slower of the two.
///
/// 200k therefore leaves ~7–10× headroom over the extreme loop (~200× over a realistic
/// one) while capping the stall at roughly **210 ms debug / 15 ms release** — noticeable
/// while a loop is broken, invisible otherwise, and nothing like a hang.
///
/// It also bounds everything downstream: a `while true { … }` that emits cannot grow the
/// output buffer without bound, because each emit costs operations from the same budget.
///
/// The budget is **per `run` call**, not per engine — see
/// `the_operation_budget_resets_for_every_run`, which is load-bearing: one engine renders
/// every primitive of a board, so a cumulative counter would fail a job part-way through.
pub const MAX_OPERATIONS: u64 = 200_000;

impl Gtl {
    /// Build an engine with the language surface registered: `emit(text)` and a
    /// default `fmt(value)` for plain scalars and strings. Register the host
    /// dialect (custom-type `fmt` overloads, domain functions) through
    /// [`engine_mut`](Gtl::engine_mut).
    pub fn new() -> Self {
        let output = Rc::new(RefCell::new(String::new()));
        let mut engine = Engine::new();

        // A template that never terminates must not take the caller down with it.
        // See [`MAX_OPERATIONS`] — this is off by default in Rhai.
        engine.set_max_operations(MAX_OPERATIONS);

        let sink = output.clone();
        engine.register_fn("emit", move |text: ImmutableString| {
            let mut buf = sink.borrow_mut();
            buf.push_str(&text);
            buf.push('\n');
        });

        // The closing-backtick form: no newline, so the next emit continues the line.
        let sink = output.clone();
        engine.register_fn("emit_raw", move |text: ImmutableString| {
            sink.borrow_mut().push_str(&text);
        });

        // Default formatter for plain values. A host overrides `fmt` for its own
        // types (e.g. unit-typed values) via `engine_mut`; Rhai prefers the
        // exact-type overload, so these remain the fallback for bare scalars.
        engine.register_fn("fmt", |v: i64| v.to_string());
        engine.register_fn("fmt", |v: f64| v.to_string());
        engine.register_fn("fmt", |v: bool| v.to_string());
        engine.register_fn("fmt", |v: ImmutableString| v);

        Self { engine, output }
    }

    /// Mutable access to the underlying Rhai engine, to register the host dialect:
    /// custom types, additional `fmt` overloads, and domain functions.
    pub fn engine_mut(&mut self) -> &mut Engine {
        &mut self.engine
    }

    /// A cloneable handle to the output buffer for host natives that need to emit.
    pub fn writer(&self) -> Writer {
        Writer(self.output.clone())
    }

    /// Transpile `source` (GTL) to Rhai and compile it into a cached [`Template`].
    /// Transpile and compile errors are reported against the author's source line
    /// (the transpile is 1:1, so no line map is needed).
    pub fn compile(&self, name: &str, source: &str) -> Result<Template, GtlError> {
        let transpiled = transpile::transpile(source).map_err(|(line, col, message)| {
            GtlError::Parse {
                template: name.to_string(),
                line,
                col,
                message,
            }
        })?;
        let ast = self.engine.compile(&transpiled).map_err(|err| {
            let pos = err.position();
            GtlError::Parse {
                template: name.to_string(),
                line: pos.line().unwrap_or(0),
                col: pos.position().unwrap_or(0),
                message: err.to_string(),
            }
        })?;
        Ok(Template {
            name: name.to_string(),
            ast,
        })
    }

    /// Run a compiled template against `scope`, returning the emitted text. The
    /// scope carries the caller's variables (in k2g: the program/operation/call
    /// layers); the engine adds nothing to it. The output buffer is cleared first,
    /// so each call returns only its own emission.
    pub fn run(&self, template: &Template, scope: &mut Scope) -> Result<String, GtlError> {
        self.output.borrow_mut().clear();
        self.engine
            .run_ast_with_scope(scope, &template.ast)
            .map_err(|err| map_eval_error(&template.name, err))?;
        let out = self.output.borrow().clone();
        Ok(out)
    }
}

impl Default for Gtl {
    fn default() -> Self {
        Self::new()
    }
}

/// Map a Rhai evaluation failure to a [`GtlError`], distinguishing a scripted
/// `throw` (a deliberate precondition failure) and a runaway loop from other
/// runtime errors.
fn map_eval_error(name: &str, err: Box<EvalAltResult>) -> GtlError {
    let pos = err.position();
    let line = pos.line().unwrap_or(0);
    match err.as_ref() {
        EvalAltResult::ErrorRuntime(value, _) => GtlError::Thrown {
            template: name.to_string(),
            value: value.to_string(),
        },
        // Given its own variant so the message can name the likely cause. Rhai's
        // rendering is "Too many operations", which is true and useless to an author
        // who did not know operations were being counted.
        EvalAltResult::ErrorTooManyOperations(_) => GtlError::Runaway {
            template: name.to_string(),
            line,
            limit: MAX_OPERATIONS,
        },
        _ => GtlError::Runtime {
            template: name.to_string(),
            line,
            message: err.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(gtl: &Gtl, src: &str, scope: &mut Scope) -> Result<String, GtlError> {
        let tmpl = gtl.compile("test", src)?;
        gtl.run(&tmpl, scope)
    }

    #[test]
    fn interpolates_scalars_by_type() {
        let gtl = Gtl::new();
        let mut scope = Scope::new();
        scope.push("x", 3.2_f64);
        scope.push("y", 7_i64);
        assert_eq!(render(&gtl, "`G0 X{x} Y{y}", &mut scope).unwrap(), "G0 X3.2 Y7\n");
    }

    /// A closing backtick suppresses the newline, so several emits compose one output
    /// line. This is the end-to-end behaviour the module docs (and the app's GTL help)
    /// teach — the transpiler tests cover the parse, this covers what comes out.
    ///
    /// The **optional prefix** is the shape worth pinning: the conditional piece carries
    /// the closing backtick and an ordinary emit finishes the line, so the result is one
    /// well-formed line whether or not the condition fires.
    #[test]
    fn a_closing_backtick_lets_several_emits_compose_one_line() {
        let gtl = Gtl::new();
        let src = "if dry_run {\n    `(SIMULATED) `\n}\n`G1 X{x} F{feed}";
        let render_with = |dry: bool| {
            let mut scope = Scope::new();
            scope.push("dry_run", dry);
            scope.push("x", 3.2_f64);
            scope.push("feed", 300_i64);
            render(&gtl, src, &mut scope).unwrap()
        };
        assert_eq!(render_with(true), "(SIMULATED) G1 X3.2 F300\n");
        assert_eq!(render_with(false), "G1 X3.2 F300\n", "the line is well-formed either way");
    }

    /// A prefix-only template emits no newline of its own — what makes it a prefix.
    #[test]
    fn a_prefix_template_leaves_the_line_open() {
        let gtl = Gtl::new();
        let mut scope = Scope::new();
        scope.push("line", 1_i64);
        assert_eq!(render(&gtl, "`N{line * 10} `", &mut scope).unwrap(), "N10 ");
    }

    #[test]
    fn control_flow_emits_lines_in_order() {
        let gtl = Gtl::new();
        let mut scope = Scope::new();
        scope.push("z_retract", 2_i64);
        scope.push("z_bottom", -5_i64);
        scope.push("peck", 3_i64);
        let src = "\
`G0 Z{z_retract}
let z = z_retract;
while z > z_bottom {
    z = z - peck;
    if z < z_bottom { z = z_bottom }
    `G1 Z{z}
}";
        assert_eq!(
            render(&gtl, src, &mut scope).unwrap(),
            "G0 Z2\nG1 Z-1\nG1 Z-4\nG1 Z-5\n"
        );
    }

    #[test]
    fn doubled_braces_render_literally() {
        let gtl = Gtl::new();
        let mut scope = Scope::new();
        assert_eq!(render(&gtl, "`X{{a}}", &mut scope).unwrap(), "X{a}\n");
    }

    #[test]
    fn undefined_variable_is_a_runtime_error() {
        let gtl = Gtl::new();
        let mut scope = Scope::new();
        let err = render(&gtl, "`Z{z}", &mut scope).unwrap_err();
        assert!(matches!(err, GtlError::Runtime { .. }), "{err:?}");
    }

    #[test]
    fn throw_becomes_a_thrown_error() {
        let gtl = Gtl::new();
        let mut scope = Scope::new();
        scope.push("bad", true);
        let err = render(&gtl, "if bad { throw \"below surface\" }\n`G0", &mut scope).unwrap_err();
        match err {
            GtlError::Thrown { value, .. } => assert_eq!(value, "below surface"),
            other => panic!("expected Thrown, got {other:?}"),
        }
    }

    #[test]
    fn unterminated_interpolation_is_a_parse_error() {
        let gtl = Gtl::new();
        let err = gtl.compile("t", "`Z{z").unwrap_err();
        assert!(matches!(err, GtlError::Parse { line: 1, .. }), "{err:?}");
    }

    /// **A template that never terminates must not hang the caller.**
    ///
    /// This is not a hypothetical: templates are author-written scripts with loops, and
    /// the editor re-renders one on every keystroke *on the UI thread*. Before the
    /// operation ceiling this test would not have failed — it would have hung the run.
    ///
    /// The elapsed-time assertion is the real subject. A limit that stops the loop but
    /// takes ten seconds to do it still freezes the application. The bound is loose
    /// relative to the measured ~210 ms debug cost, so the test is about "bounded, and
    /// fast enough not to read as a hang" rather than about this machine's speed.
    #[test]
    fn a_runaway_loop_is_stopped_quickly() {
        let gtl = Gtl::new();
        let started = std::time::Instant::now();
        let error = render(&gtl, "let z = 1;\nwhile z > 0 {\n    `G1 Z{z}\n}", &mut Scope::new())
            .expect_err("an endless loop must not return a program");
        let elapsed = started.elapsed();

        match error {
            GtlError::Runaway { template, line, limit } => {
                assert_eq!(template, "test");
                assert_eq!(limit, MAX_OPERATIONS);
                assert!(line > 0, "the author needs a line to look at, got {line}");
            }
            other => panic!("expected Runaway, got {other:?}"),
        }
        assert!(
            elapsed < RUNAWAY_MUST_STOP_WITHIN,
            "the ceiling did not stop the loop in any reasonable time: took {elapsed:?}. \
             Either MAX_OPERATIONS has been raised a long way or the ceiling is gone."
        );
    }

    /// How long a runaway may take to be stopped before the test calls it broken.
    ///
    /// Deliberately loose, because the ceiling counts **operations, not time** — which is
    /// the right unit for it (a template that renders on one machine must render on
    /// every machine, so the budget cannot depend on how fast the CPU is), but it means
    /// the wall clock here measures the runner, not the code. This was 750 ms and failed
    /// on a macOS CI runner at 1.2 s: an unoptimised build of Rhai on a shared box,
    /// which says nothing about the shipped article.
    ///
    /// The number that is still worth asserting is the one that separates "stopped" from
    /// "not stopped": 200,000 operations is milliseconds in a release build and a second
    /// or so in the worst debug case seen, while a ceiling raised tenfold — or removed,
    /// which is a hang — blows straight through this.
    const RUNAWAY_MUST_STOP_WITHIN: std::time::Duration = std::time::Duration::from_secs(10);

    /// The message has to send the author somewhere. "Too many operations" — Rhai's own
    /// wording — describes the symptom to someone who did not know operations were being
    /// counted, and says nothing about loops.
    #[test]
    fn a_runaway_says_what_to_look_for() {
        let gtl = Gtl::new();
        let error =
            render(&gtl, "while true {\n    `x\n}", &mut Scope::new()).expect_err("stopped");
        let text = error.to_string();
        assert!(text.contains("did not finish"), "{text}");
        assert!(text.contains("loop"), "the likely cause must be named: {text}");
    }

    /// A runaway that emits cannot exhaust memory first: every `emit` costs operations
    /// from the same budget, so the output is bounded by the ceiling too.
    #[test]
    fn a_runaway_that_emits_cannot_grow_without_bound() {
        let gtl = Gtl::new();
        let error = render(&gtl, "while true {\n    `G1 X1 Y1 Z1 F100\n}", &mut Scope::new());
        assert!(matches!(error, Err(GtlError::Runaway { .. })), "{error:?}");
    }

    /// **The budget is per `run`, not per engine.**
    ///
    /// One engine renders every primitive of a whole board — thousands of calls — so a
    /// counter that accumulated across runs would abort a real job part-way through, and
    /// would do it as a "runaway loop" error against a template with no loop in it. The
    /// failure would scale with board size, so it would pass every small test and appear
    /// only on real work.
    ///
    /// Rhai resets the count per evaluation; this pins that, because the whole design
    /// depends on it and it is not visible at the call site.
    #[test]
    fn the_operation_budget_resets_for_every_run() {
        let gtl = Gtl::new();
        // Each pass is a few thousand operations; 200 of them far exceed one budget in
        // total, so this only passes if the count starts again each time.
        let tmpl = gtl
            .compile("t", "let z = 500;\nwhile z > 0 {\n    `G1 Z{z}\n    z -= 1;\n}")
            .unwrap();
        for pass in 0..200 {
            let out = gtl
                .run(&tmpl, &mut Scope::new())
                .unwrap_or_else(|e| panic!("pass {pass} must not exhaust a shared budget: {e}"));
            assert_eq!(out.lines().count(), 500);
        }
    }

    /// The ceiling must not be so tight that a legitimate template trips it. A peck loop
    /// over a deep hole is the most demanding shape a real profile writes, and it has to
    /// clear the limit with room to spare.
    #[test]
    fn a_long_but_finite_loop_still_completes() {
        let gtl = Gtl::new();
        // 2000 passes — an order of magnitude more than any real peck cycle.
        let out = render(
            &gtl,
            "let z = 2000;\nwhile z > 0 {\n    `G1 Z{z}\n    z -= 1;\n}",
            &mut Scope::new(),
        )
        .expect("a bounded loop must not be mistaken for a runaway");
        assert_eq!(out.lines().count(), 2000);
    }

    #[test]
    fn host_registers_dialect_functions() {
        let mut gtl = Gtl::new();
        gtl.engine_mut().register_fn("safe_z", || 5_i64);
        let mut scope = Scope::new();
        assert_eq!(render(&gtl, "`G0 Z{safe_z()}", &mut scope).unwrap(), "G0 Z5\n");
    }

    #[test]
    fn host_fmt_override_formats_a_custom_type() {
        #[derive(Clone)]
        struct Len(f64);
        let mut gtl = Gtl::new();
        gtl.engine_mut().register_type::<Len>();
        gtl.engine_mut().register_fn("fmt", |v: Len| format!("{:.1}", v.0));
        let mut scope = Scope::new();
        scope.push("z", Len(3.456));
        assert_eq!(render(&gtl, "`Z{z}", &mut scope).unwrap(), "Z3.5\n");
    }

    #[test]
    fn host_native_emits_via_writer() {
        let mut gtl = Gtl::new();
        let writer = gtl.writer();
        gtl.engine_mut().register_fn("preamble", move || {
            writer.emit("G21");
            writer.emit("G90");
        });
        let mut scope = Scope::new();
        assert_eq!(render(&gtl, "preamble();\n`G0", &mut scope).unwrap(), "G21\nG90\nG0\n");
    }
}




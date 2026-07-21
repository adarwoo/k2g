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

impl Gtl {
    /// Build an engine with the language surface registered: `emit(text)` and a
    /// default `fmt(value)` for plain scalars and strings. Register the host
    /// dialect (custom-type `fmt` overloads, domain functions) through
    /// [`engine_mut`](Gtl::engine_mut).
    pub fn new() -> Self {
        let output = Rc::new(RefCell::new(String::new()));
        let mut engine = Engine::new();

        let sink = output.clone();
        engine.register_fn("emit", move |text: ImmutableString| {
            let mut buf = sink.borrow_mut();
            buf.push_str(&text);
            buf.push('\n');
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
/// `throw` (a deliberate precondition failure) from other runtime errors.
fn map_eval_error(name: &str, err: Box<EvalAltResult>) -> GtlError {
    let pos = err.position();
    let line = pos.line().unwrap_or(0);
    if let EvalAltResult::ErrorRuntime(value, _) = err.as_ref() {
        return GtlError::Thrown {
            template: name.to_string(),
            value: value.to_string(),
        };
    }
    GtlError::Runtime {
        template: name.to_string(),
        line,
        message: err.to_string(),
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

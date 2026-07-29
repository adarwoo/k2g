//! The Coder — the app-side GCode dialect over the generic `gtl` engine
//! (docs/gcode-engine.md §1.1). It turns a CNC profile's primitive templates into
//! GCode by registering the GCode surface on the engine and running each template
//! against a scope of resolved values.
//!
//! **Phase: header.** The surface here is only what the program preamble needs —
//! `metric()`/`imperial()` (emit `G21`/`G20` and fix the active unit) and a
//! unit-aware `fmt(Length)` — enough to render the `initialise` and `conclude`
//! primitives from the program-layer values (`pcb_filename`, `timestamp`, `z_safe`).
//! The operation/call layers (tool values, coordinates) and the drilling/routing
//! primitives arrive in later phases; see the scope model in docs/gcode-engine.md §2.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gtl::{Gtl, GtlError, Scope, Template};
use units::{FeedRate, Length, RotationalSpeed, UserUnitSystem};

use crate::gcode::primitive_vars::{PrimitiveVar, VarType};

/// A `gtl` engine with the GCode dialect registered. Built once per generation run
/// (on the worker thread) and reused across the program's primitives; the active
/// unit mode is engine state that carries from `initialise` through later calls.
pub struct Coder {
    gtl: Gtl,
    /// Templates this Coder has already parsed, keyed by primitive name, each stored
    /// beside the source it was compiled from. See [`Coder::template_for`].
    cache: RefCell<HashMap<String, (String, Rc<Template>)>>,
    /// How many templates this Coder has actually parsed. The cache leaves no trace in
    /// the rendered GCode — by design — so this counter is the only way to assert that
    /// it works; the tests read it.
    compiles: Cell<usize>,
}

impl Coder {
    /// Registers the GCode surface:
    /// - `metric()` / `imperial()` — emit the modal `G21`/`G20` and set how lengths
    ///   format from here on.
    /// - `fmt(Length)` — a length as a bare coordinate number in the active machine
    ///   unit (via `units::machine::number_length`); the generic engine already
    ///   supplies `fmt` for plain scalars and strings.
    pub fn new() -> Self {
        let mut gtl = Gtl::new();

        // The active machine unit, shared by metric()/imperial() and the Length
        // formatter. It lives on the engine (not the scope) so a unit set in
        // `initialise` survives into every later primitive (docs/gcode-engine.md §3.2).
        let unit_system = Rc::new(Cell::new(UserUnitSystem::Metric));

        let writer = gtl.writer();
        let mode = unit_system.clone();
        gtl.engine_mut().register_fn("metric", move || {
            mode.set(UserUnitSystem::Metric);
            writer.emit("G21");
        });

        let writer = gtl.writer();
        let mode = unit_system.clone();
        gtl.engine_mut().register_fn("imperial", move || {
            mode.set(UserUnitSystem::Imperial);
            writer.emit("G20");
        });

        gtl.engine_mut().register_type::<Length>();
        let mode = unit_system.clone();
        gtl.engine_mut().register_fn("fmt", move |length: Length| {
            units::machine::number_length(length, mode.get())
        });

        // Feed rates and spindle speeds format like lengths: a bare number in the
        // active machine system (feed converts mm/min↔in/min; rpm is system-invariant).
        // The body phase (drilling, routing) pushes these into `{z_feedrate}`/`{rpm}`.
        gtl.engine_mut().register_type::<FeedRate>();
        let mode = unit_system.clone();
        gtl.engine_mut().register_fn("fmt", move |feed: FeedRate| {
            units::machine::number_feed(feed, mode.get())
        });

        gtl.engine_mut().register_type::<RotationalSpeed>();
        let mode = unit_system.clone();
        gtl.engine_mut().register_fn("fmt", move |rpm: RotationalSpeed| {
            units::machine::number_speed(rpm, mode.get())
        });

        Self { gtl, cache: RefCell::new(HashMap::new()), compiles: Cell::new(0) }
    }

    /// Renders one primitive template against `scope`, returning its GCode. The template
    /// is parsed on first use and reused thereafter (see [`Coder::template_for`]).
    pub fn render(&self, name: &str, source: &str, scope: &mut Scope) -> Result<String, GtlError> {
        let template = self.template_for(name, source)?;
        self.gtl.run(&template, scope)
    }

    /// The compiled template for `name`, parsing it only if this Coder has not already
    /// parsed *this* source under that name.
    ///
    /// Rendering used to transpile and parse on every call — once per hole, per routing
    /// move, and once per output line of the finished program, always from identical
    /// source. A Coder is built per machining step, so its lifetime is exactly the right
    /// cache scope: the entry dies with the program it served.
    ///
    /// Keyed on the primitive **name**, with the source compared on a hit rather than
    /// hashed. Generation renders one source per name, so every call after the first is a
    /// hit; the profile editor renders freshly-typed source under a fixed name, so the
    /// comparison invalidates exactly, with no chance of a hash collision serving stale
    /// GCode. One short string compare per call is nothing beside a parse.
    ///
    /// Handed out as an `Rc` so a hit costs a refcount bump — `Template` is `Clone`, but
    /// cloning one clones the whole AST — and so the borrow is released before the
    /// template runs.
    ///
    /// Sound only because [`gtl::Gtl::compile`] compiles with **no** scope: Rhai's
    /// optimizer therefore folds literals alone and never propagates scope constants
    /// into the AST, which is what makes one AST safe to run against every call's
    /// different scope. Compiling with a scope would quietly break that.
    fn template_for(&self, name: &str, source: &str) -> Result<Rc<Template>, GtlError> {
        let hit = self
            .cache
            .borrow()
            .get(name)
            .and_then(|(cached, template)| (cached == source).then(|| template.clone()));
        if let Some(template) = hit {
            return Ok(template);
        }

        // A failed compile is *not* cached: the editor's next keystroke may well fix it,
        // and a poisoned entry would keep reporting the old error.
        let template = Rc::new(self.gtl.compile(name, source)?);
        self.compiles.set(self.compiles.get() + 1);
        self.cache
            .borrow_mut()
            .insert(name.to_string(), (source.to_string(), template.clone()));
        Ok(template)
    }

    /// How many templates this Coder has parsed — the cache's only observable effect,
    /// and so only needed by the tests that assert it.
    #[cfg(test)]
    pub fn compiles(&self) -> usize {
        self.compiles.get()
    }

    /// Validates *and* previews a primitive: renders `source` against a scope of
    /// representative sample values for its declared `vars`. Returns the rendered
    /// GCode, or the GTL error — either a syntax error or a reference to a variable
    /// the primitive does not declare (the `z_safe`-not-found class). Because the
    /// sample scope holds *only* the declared variables, an undeclared reference
    /// fails exactly as it would during generation. Backs the primitive editor's
    /// inline validate + preview pane.
    pub fn preview(&self, name: &str, source: &str, vars: &[PrimitiveVar]) -> Result<String, GtlError> {
        let mut scope = Scope::new();
        for var in vars {
            push_sample(&mut scope, name, var);
        }
        self.render(name, source, &mut scope)
    }
}

/// Pushes one representative sample value for `var` into `scope`, typed so the
/// registered/default `fmt` overloads render it. Feed/rpm/angle are previewed as
/// plain numbers for now (the Coder formats real `Length`; the other unit types
/// gain `fmt` registrations when generation begins using them).
fn push_sample(scope: &mut Scope, primitive: &str, var: &PrimitiveVar) {
    match var.var_type {
        VarType::String => {
            scope.push(var.name.clone(), sample_string(primitive, &var.name));
        }
        VarType::Boolean => {
            // `true` so the preview of a two-branch template (`cut_arc`'s direction)
            // shows the first branch rather than an empty or negative case.
            scope.push(var.name.clone(), true);
        }
        VarType::Length => {
            scope.push(var.name.clone(), Length::from_mm(10.0));
        }
        VarType::Integer => {
            scope.push(var.name.clone(), 1_i64);
        }
        VarType::Feed => {
            scope.push(var.name.clone(), 300.0_f64);
        }
        VarType::Rpm => {
            scope.push(var.name.clone(), 12_000.0_f64);
        }
        VarType::Angle => {
            scope.push(var.name.clone(), 90.0_f64);
        }
        VarType::Number => {
            scope.push(var.name.clone(), 1.0_f64);
        }
    }
}

/// A readable sample for a string variable — a realistic value for the well-known
/// names, a neutral placeholder otherwise.
///
/// Keyed by primitive as well as by name, because `text` means two different things: a
/// caption to `banner`, and the whole program line to `line_number` — whose template may
/// branch on it, so a caption there would preview a case that never occurs.
fn sample_string(primitive: &str, name: &str) -> String {
    match (primitive, name) {
        ("line_number", "text") => "G1 X10 Y5 F600",
        (_, "pcb_filename") => "board.kicad_pcb",
        (_, "timestamp") => "2026-01-01 12:00:00",
        (_, "arc_cmd") => "G2",
        (_, "manual_message") => "(change tool)",
        (_, "message") => "Paused",
        (_, "text") => "Section",
        _ => "sample",
    }
    .to_string()
}

impl Default for Coder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `initialise` primitive renders with the program-layer values, and
    /// `metric()` emits `G21` and fixes the length unit for `{z_safe}`.
    #[test]
    fn renders_the_initialise_header() {
        let coder = Coder::new();
        let mut scope = Scope::new();
        scope.push("pcb_filename", "demo.kicad_pcb".to_string());
        scope.push("timestamp", "2026-01-01 00:00:00".to_string());
        scope.push("z_safe", Length::from_mm(5.0));

        let source = "`(k2g {pcb_filename} - {timestamp})\nmetric();\n`G0 Z{z_safe}";
        let out = coder.render("initialise", source, &mut scope).unwrap();

        assert!(out.contains("(k2g demo.kicad_pcb - 2026-01-01 00:00:00)"));
        assert!(out.contains("G21"), "metric() emits G21");
        assert!(out.contains("G0 Z5"), "z_safe formats as a bare metric number");
    }

    #[test]
    fn imperial_switches_the_length_unit() {
        let coder = Coder::new();
        let mut scope = Scope::new();
        scope.push("z_safe", Length::from_mm(25.4));
        let out = coder.render("t", "imperial();\n`G0 Z{z_safe}", &mut scope).unwrap();
        assert!(out.contains("G20"));
        assert!(out.contains("G0 Z1"), "25.4 mm reads as 1 inch after imperial()");
    }

    #[test]
    fn a_parse_error_is_reported_not_panicked() {
        let coder = Coder::new();
        let mut scope = Scope::new();
        // Unclosed interpolation → a GTL parse error, surfaced (never a panic).
        assert!(coder.render("bad", "`G0 Z{z_safe", &mut scope).is_err());
    }

    /// The whole point of the cache: `line_number` renders once per line of the finished
    /// program and `drill` once per hole, so a per-call parse is thousands of parses of
    /// identical source. Rendering leaves no trace of the cache in its output, so the
    /// parse count is what has to be asserted.
    #[test]
    fn a_repeated_primitive_is_parsed_only_once() {
        let coder = Coder::new();
        let source = "`N{line * 10} `";

        let outputs: Vec<String> = (1..=3)
            .map(|line| {
                let mut scope = Scope::new();
                scope.push("line", line as i64);
                coder.render("line_number", source, &mut scope).unwrap()
            })
            .collect();

        assert_eq!(outputs, vec!["N10 ", "N20 ", "N30 "], "each call still renders its own values");
        assert_eq!(coder.compiles(), 1, "parsed on the first call only");
    }

    /// The profile editor renders freshly-typed source under a fixed primitive name, so
    /// the cache must follow the edit. Serving the previous parse here would show the
    /// author a preview of the template they no longer have.
    #[test]
    fn a_changed_source_under_the_same_name_is_reparsed() {
        let coder = Coder::new();
        let banner = |source: &str| {
            let mut scope = Scope::new();
            scope.push("text", "Section".to_string());
            coder.render("banner", source, &mut scope).unwrap()
        };

        assert_eq!(banner("`( {text} )"), "( Section )\n");
        assert_eq!(banner("`; {text}"), "; Section\n", "the edit is what renders");
        assert_eq!(coder.compiles(), 2, "the changed source was parsed again");
    }

    /// A Coder is built per machining step, so its cache is per step too — the same
    /// isolation the unit mode already has, for the same reason.
    #[test]
    fn two_coders_do_not_share_a_cache() {
        let source = "`G21";
        let first = Coder::new();
        first.render("initialise", source, &mut Scope::new()).unwrap();
        assert_eq!(first.compiles(), 1);

        let second = Coder::new();
        assert_eq!(second.compiles(), 0, "a fresh Coder starts empty");
        second.render("initialise", source, &mut Scope::new()).unwrap();
        assert_eq!(second.compiles(), 1, "and parses for itself");
    }

    /// A failed parse must not take the name's slot: in the editor the next keystroke is
    /// usually the fix, and a poisoned entry would keep reporting the broken template.
    #[test]
    fn a_broken_template_is_not_cached() {
        let coder = Coder::new();
        assert!(coder.render("initialise", "`G0 Z{z_safe", &mut Scope::new()).is_err());
        assert_eq!(coder.compiles(), 0, "nothing was parsed, so nothing was stored");

        let fixed = coder.render("initialise", "`G0 Z5", &mut Scope::new()).unwrap();
        assert_eq!(fixed, "G0 Z5\n", "the repaired source renders");
        assert_eq!(coder.compiles(), 1);
    }

    #[test]
    fn preview_renders_declared_variables_and_rejects_undeclared_ones() {
        let coder = Coder::new();
        let vars = crate::gcode::primitive_vars::variables_for("initialise");

        // A template using only declared variables previews cleanly.
        let ok = coder
            .preview("initialise", "`(from {pcb_filename})\nmetric();\n`G0 Z{z_safe}", &vars)
            .expect("declared variables render");
        assert!(ok.contains("(from board.kicad_pcb)"), "string sample substituted");
        assert!(ok.contains("G0 Z10"), "length sample rendered as a bare number");

        // Referencing a variable this primitive does not declare fails — exactly
        // the `z_safe not found` class the editor is meant to catch early.
        let err = coder.preview("initialise", "`G0 X{feedrate}", &vars);
        assert!(err.is_err(), "undeclared variable must fail preview");
    }

    /// `line_number` gets a GCode line as its `text` sample, not a banner caption, so a
    /// template that branches on the line previews the case it will actually meet. The
    /// editor is where these templates get written; previewing the wrong branch would
    /// send the author looking for a bug that isn't there.
    #[test]
    fn the_line_number_preview_samples_a_program_line() {
        let coder = Coder::new();
        let vars = crate::gcode::primitive_vars::variables_for("line_number");

        let numbered = coder
            .preview("line_number", "if !text.starts_with(\"(\") {\n    `N{line * 10} `\n}", &vars)
            .expect("line and text are both declared");
        assert_eq!(numbered, "N10 ", "the sample line is code, so it is numbered");

        // `banner`'s caption sample is untouched by the split.
        let caption = coder.preview("banner", "`( {text} )", &crate::gcode::primitive_vars::variables_for("banner"));
        assert!(caption.unwrap().contains("Section"));
    }
}

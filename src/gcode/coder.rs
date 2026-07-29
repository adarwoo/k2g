//! The Coder — the app-side GCode dialect over the generic `gtl` engine
//! (docs/gcode-engine.md §1.1). It turns a CNC profile's primitive templates into
//! GCode by registering the GCode surface on the engine and running each template
//! against a scope of resolved values.
//!
//! Two things are registered here: the modal unit switches, `metric()`/`imperial()`,
//! which need the engine's output writer as well as the mode; and — delegated to
//! [`crate::gcode::dialect`] — everything the `units` types can do inside a script.
//!
//! What is *not* here yet is the scope model of docs/gcode-engine.md §2: a template
//! sees only the variables its call site hand-pushes, with no namespaced job context.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gtl::{Gtl, GtlError, Scope, Template};
use units::{Angle, FeedRate, Length, RotationalSpeed, UserUnitSystem};

use crate::gcode::primitive_vars::{PrimitiveVar, VarType};
use crate::gcode::program::BodyError;

/// What `metric()` and `imperial()` emit, rendered from the profile's `set_unit`
/// primitive. Empty by default, which is a machine with no unit statement — and the
/// only honest default, since the application has no business knowing that a G-code
/// machine spells this `G21`.
#[derive(Default)]
struct UnitCommands {
    metric: String,
    imperial: String,
}

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
    /// A Coder whose `metric()`/`imperial()` set the formatting mode but **emit
    /// nothing**, for callers that render primitives outside a program — the editor's
    /// preview, and the tests that are not about the unit statement.
    ///
    /// Generation uses [`Coder::with_unit_commands`], because what those calls emit is
    /// the profile's to say.
    pub fn new() -> Self {
        Self::build(UnitCommands::default())
    }

    /// A Coder whose `metric()`/`imperial()` emit the profile's `set_unit` primitive.
    ///
    /// The primitive is rendered **here, once for each unit**, and the two results are
    /// held for the calls to emit. Rendering it on demand instead would mean running a
    /// template from inside a registered function, and [`gtl::Gtl::run`] clears the
    /// shared output buffer on entry — the enclosing template's emitted lines would
    /// vanish. Rendering up front also means a broken `set_unit` is reported before any
    /// GCode is produced rather than halfway through a program.
    ///
    /// An empty template is a machine with no unit statement: the calls then only set
    /// the formatting mode, which is what a fixed-unit controller wants.
    pub fn with_unit_commands(set_unit_tpl: &str) -> Result<Self, BodyError> {
        // Rendered by a throwaway Coder: `self` cannot render its own `set_unit` before
        // it exists, and this one is identical in every respect that matters to it.
        let renderer = Self::new();
        let render_for = |metric: bool| -> Result<String, BodyError> {
            if set_unit_tpl.trim().is_empty() {
                return Ok(String::new());
            }
            let mut scope = Scope::new();
            scope.push("metric", metric);
            scope.push("unit", if metric { "metric" } else { "imperial" }.to_string());
            renderer.render("set_unit", set_unit_tpl, &mut scope).map_err(|error| {
                BodyError::Render { primitive: "set_unit".to_string(), message: error.to_string() }
            })
        };

        Ok(Self::build(UnitCommands { metric: render_for(true)?, imperial: render_for(false)? }))
    }

    /// Registers the GCode surface:
    /// - `metric()` / `imperial()` — set how every later value formats, and emit
    ///   `commands`, which came from the profile.
    /// - the typed-value surface for `Length`, `FeedRate`, `RotationalSpeed` and
    ///   `Angle` — see [`crate::gcode::dialect`].
    fn build(commands: UnitCommands) -> Self {
        let mut gtl = Gtl::new();

        // The active machine unit, shared by metric()/imperial() and the Length
        // formatter. It lives on the engine (not the scope) so a unit set in
        // `initialise` survives into every later primitive (docs/gcode-engine.md §3.2).
        let unit_system = Rc::new(Cell::new(UserUnitSystem::Metric));

        // Setting the mode and emitting the machine's word for it are deliberately one
        // call. Split apart, a profile could emit its inch word while the engine kept
        // formatting in millimetres — inch-mode geometry cut from metric numbers, with
        // nothing anywhere to notice.
        for (name, system, command) in [
            ("metric", UserUnitSystem::Metric, commands.metric),
            ("imperial", UserUnitSystem::Imperial, commands.imperial),
        ] {
            let writer = gtl.writer();
            let mode = unit_system.clone();
            gtl.engine_mut().register_fn(name, move || {
                mode.set(system);
                // Already terminated by its own emit line, so raw: adding a newline here
                // would put a blank line in every program.
                writer.emit_raw(&command);
            });
        }

        // Everything the unit types can do in a script — formatting, comparison,
        // arithmetic, `max`/`min`/`abs`/`clamp`, and the `.mm`-style accessors — lives
        // in `dialect`, which is where its rationale is written down too.
        crate::gcode::dialect::register(gtl.engine_mut(), &unit_system);

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

/// Pushes one representative sample value for `var` into `scope`, **typed as generation
/// types it** — a feed as a `FeedRate`, not as the bare number it prints as.
///
/// The distinction is the whole point of the preview: a bare `f64` renders identically
/// but supports none of the unit surface, so `{z_feedrate.mm_per_min}` or
/// `if z_feedrate > max` would be rejected here and accepted during generation. A live
/// validator that disagrees with the real run is worse than none.
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
            scope.push(var.name.clone(), FeedRate::from_mm_per_min(300.0));
        }
        VarType::Rpm => {
            scope.push(var.name.clone(), RotationalSpeed::from_rpm(12_000.0));
        }
        VarType::Angle => {
            scope.push(var.name.clone(), Angle::from_degrees(90.0));
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
///
/// The `line_number` sample is the one machine-code-shaped string left in this file, and
/// it is not emitted anywhere: it stands in for "the line about to be numbered" so a
/// template that inspects it (to leave comments unnumbered, say) previews a realistic
/// case. A profile in another language previews against a G-code-looking line, which is
/// cosmetic — nothing derived from it reaches a program.
fn sample_string(primitive: &str, name: &str) -> String {
    match (primitive, name) {
        ("line_number", "text") => "G1 X10 Y5 F600",
        (_, "pcb_filename") => "board.kicad_pcb",
        (_, "timestamp") => "2026-01-01 12:00:00",
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
    /// `metric()` emits the profile's word for it and fixes the length unit for
    /// `{z_safe}`.
    #[test]
    fn renders_the_initialise_header() {
        let coder = Coder::with_unit_commands(&crate::gcode::program::sample_set_unit_tpl()).unwrap();
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

    /// The two halves of `imperial()` — the word the machine is sent, and how every
    /// later value formats — arrive together. They are one call precisely so a program
    /// cannot end up in inch mode carrying millimetre numbers.
    #[test]
    fn imperial_emits_the_profiles_word_and_switches_the_length_unit() {
        let coder = Coder::with_unit_commands(&crate::gcode::program::sample_set_unit_tpl()).unwrap();
        let mut scope = Scope::new();
        scope.push("z_safe", Length::from_mm(25.4));
        let out = coder.render("t", "imperial();\n`G0 Z{z_safe}", &mut scope).unwrap();
        assert_eq!(out, "G20\nG0 Z1\n", "the profile's word, then 25.4 mm read as 1 inch");
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

    // ---- the unit statement belongs to the profile -------------------------------

    /// The application knows *that* a machine has a unit mode, never *how* it is
    /// spelled. A Coder with no profile behind it emits nothing for either call — which
    /// is what makes the next test possible.
    #[test]
    fn the_application_emits_no_unit_word_of_its_own() {
        let coder = Coder::new();
        let out = coder
            .render("t", "metric();\nimperial();\n`X", &mut Scope::new())
            .unwrap();
        assert_eq!(out, "X\n", "no G21, no G20 — the profile had not said what to emit");
    }

    /// The point of the primitive: a machine whose language is not G-code at all still
    /// gets its unit statement, and the engine never learns what it means.
    #[test]
    fn a_machine_that_is_not_gcode_states_its_units_its_own_way() {
        let coder = Coder::with_unit_commands("`{if metric { \"METRIC,TZ\" } else { \"INCH,TZ\" }}")
            .unwrap();
        let out = coder.render("t", "metric();\n`T01C0.8", &mut Scope::new()).unwrap();
        assert_eq!(out, "METRIC,TZ\nT01C0.8\n", "an Excellon header, from the profile alone");
    }

    /// A controller with no unit statement — fixed to one system — leaves the primitive
    /// empty. The calls then only set the formatting, and nothing is emitted, not even a
    /// blank line.
    #[test]
    fn an_empty_unit_primitive_emits_nothing() {
        let coder = Coder::with_unit_commands("   ").unwrap();
        let mut scope = Scope::new();
        scope.push("z", Length::from_mm(25.4));
        assert_eq!(coder.render("t", "imperial();\n`Z{z}", &mut scope).unwrap(), "Z1\n");
    }

    /// A broken `set_unit` is reported before a single line of the program exists,
    /// against the primitive that could not render — not as a mystery halfway through.
    #[test]
    fn a_broken_unit_primitive_is_named() {
        match Coder::with_unit_commands("`{nope}") {
            Err(BodyError::Render { primitive, .. }) => assert_eq!(primitive, "set_unit"),
            Err(other) => panic!("expected a Render error, got {other:?}"),
            Ok(_) => panic!("a set_unit naming an unknown value must not build a Coder"),
        }
    }

    // ---- the typed-value surface (see `crate::gcode::dialect`) -------------------

    /// Renders `source` against a scope built by `setup`.
    fn rendered(setup: impl FnOnce(&mut Scope), source: &str) -> Result<String, GtlError> {
        let coder = Coder::new();
        let mut scope = Scope::new();
        setup(&mut scope);
        coder.render("t", source, &mut scope)
    }

    /// **The landmine.** A script asks whether two values are the same *quantity*; Rust's
    /// derived `PartialEq` answers whether they are the same *written form*. If the
    /// registered `==` were ever wired to the derived one, `10mm` and `1cm` would compare
    /// unequal and a depth check would silently take the wrong branch.
    #[test]
    fn the_same_length_written_two_ways_compares_equal_in_script() {
        assert_ne!(
            Length::from_mm(10.0),
            Length::from_cm(1.0),
            "Rust equality is structural — this is exactly what the script must not inherit"
        );

        let out = rendered(
            |s| {
                s.push("a", Length::from_mm(10.0));
                s.push("b", Length::from_cm(1.0));
            },
            "`{a == b} {a >= b} {a <= b} {a > b} {a != b}",
        )
        .unwrap();
        assert_eq!(out, "true true true false false\n");

        // Across unit systems too, where the conversion is not a power of ten — so this
        // passes on the tolerance, not on luck.
        let out = rendered(
            |s| {
                s.push("a", Length::from_mm(25.4));
                s.push("b", Length::from_inch(1.0));
            },
            "`{a == b}",
        )
        .unwrap();
        assert_eq!(out, "true\n");
    }

    /// The manual peck cycle from `docs/gcode-template-language.md` §8.3 — the example
    /// every author is shown, run verbatim.
    #[test]
    fn the_documented_peck_loop_renders() {
        let out = rendered(
            |s| {
                s.push("x", Length::from_mm(3.0));
                s.push("y", Length::from_mm(4.0));
                s.push("z_retract", Length::from_mm(1.0));
                s.push("z_bottom", Length::from_mm(-2.4));
                s.push("peck", Length::from_mm(1.0));
                s.push("z_feed", FeedRate::from_mm_per_min(200.0));
            },
            "`G0 X{x} Y{y}\n\
             `G0 Z{z_retract}\n\
             let z = z_retract;\n\
             while z > z_bottom {\n\
                 z = max(z - peck, z_bottom);\n\
                 `G1 Z{z} F{z_feed}\n\
                 `G0 Z{z_retract}\n\
             }",
        )
        .unwrap();

        assert_eq!(
            out,
            "G0 X3 Y4\n\
             G0 Z1\n\
             G1 Z0 F200\nG0 Z1\n\
             G1 Z-1 F200\nG0 Z1\n\
             G1 Z-2 F200\nG0 Z1\n\
             G1 Z-2.4 F200\nG0 Z1\n"
        );
    }

    /// The last pass reaches the bound exactly and the loop stops. It stops because
    /// `max` hands back the *argument*: a rebuilt value would come back through
    /// `as_mm()`'s multiply-then-divide an ulp away, and `z > z_bottom` would be true
    /// once more, emitting a duplicate move at the bottom of the hole.
    #[test]
    fn a_depth_loop_terminates_on_a_bound_written_in_another_unit() {
        let out = rendered(
            |s| {
                s.push("z_retract", Length::from_mm(0.0));
                s.push("z_bottom", Length::from_inch(-0.1)); // -2.54 mm
                s.push("peck", Length::from_mm(1.27));
            },
            "let z = z_retract;\n\
             while z > z_bottom {\n\
                 z = max(z - peck, z_bottom);\n\
                 `G1 Z{z}\n\
             }",
        )
        .unwrap();
        assert_eq!(out, "G1 Z-1.27\nG1 Z-2.54\n", "exactly two passes, no repeat of the last");
    }

    /// Arithmetic keeps the type, so the result still formats to the active unit at emit
    /// rather than freezing into whichever unit the operands were written in.
    #[test]
    fn arithmetic_stays_typed_and_converts_at_emit() {
        let setup = |s: &mut Scope| {
            s.push("z_safe", Length::from_mm(25.4));
            s.push("clearance", Length::from_mm(12.7));
            s.push("feed", FeedRate::from_mm_per_min(300.0));
        };
        assert_eq!(rendered(setup, "`G0 Z{z_safe - clearance}").unwrap(), "G0 Z12.7\n");
        // `rendered` uses a Coder with no profile, so `imperial()` only switches the
        // formatting — there is no unit word for the application to invent.
        assert_eq!(rendered(setup, "imperial();\n`G0 Z{z_safe - clearance}").unwrap(), "G0 Z0.5\n");
        // Scaling from either side, integer or float; and a ratio of two lengths is a
        // plain number, which is how a pass count gets written.
        assert_eq!(rendered(setup, "`{z_safe * 2} {2 * z_safe} {z_safe * 0.5} {z_safe / clearance}").unwrap(), "50.8 50.8 12.7 2\n");
        assert_eq!(rendered(setup, "`F{feed * 2}").unwrap(), "F600\n");
        assert_eq!(rendered(setup, "`Z{-clearance}").unwrap(), "Z-12.7\n");
    }

    /// `-=` and `+=` are not registered — Rhai rewrites a missing op-assignment to
    /// `var = var - rhs` and finds the binary operator. Pinned because that fallback is
    /// what keeps the registration list half the size.
    #[test]
    fn compound_assignment_falls_back_to_the_binary_operator() {
        let out = rendered(
            |s| {
                s.push("z", Length::from_mm(5.0));
                s.push("peck", Length::from_mm(2.0));
            },
            "let d = z;\nd -= peck;\n`G1 Z{d}",
        )
        .unwrap();
        assert_eq!(out, "G1 Z3\n");
    }

    /// The accessors are the escape hatch: a plain number in a named unit, unaffected by
    /// the modal state — which is the whole reason they exist.
    #[test]
    fn accessors_give_plain_numbers_and_ignore_the_modal_unit() {
        let setup = |s: &mut Scope| {
            s.push("z", Length::from_mm(25.4));
            s.push("f", FeedRate::from_mm_per_min(254.0));
            s.push("rpm", RotationalSpeed::from_rpm(12_000.0));
            s.push("a", Angle::from_degrees(180.0));
        };
        assert_eq!(rendered(setup, "`{z.mm} {z.cm} {z.inch} {z.mil}").unwrap(), "25.4 2.54 1 1000\n");
        assert_eq!(rendered(setup, "`{f.mm_per_min} {f.in_per_min} {rpm.rpm}").unwrap(), "254 10 12000\n");
        assert_eq!(rendered(setup, "`{a.degrees} {a.radians}").unwrap(), "180 3.141592653589793\n");
        assert_eq!(
            rendered(setup, "imperial();\n`{z.mm}").unwrap(),
            "25.4\n",
            "an accessor names its unit, so the modal state cannot move it"
        );
    }

    #[test]
    fn max_min_abs_and_clamp_work_on_lengths() {
        let setup = |s: &mut Scope| {
            s.push("a", Length::from_mm(2.0));
            s.push("b", Length::from_mm(-5.0));
        };
        assert_eq!(rendered(setup, "`{max(a, b)} {min(a, b)} {abs(b)}").unwrap(), "2 -5 5\n");
        assert_eq!(rendered(setup, "`{clamp(b, a, a)}").unwrap(), "2\n");
        assert!(
            rendered(setup, "`{clamp(a, a, b)}").is_err(),
            "a low bound above the high bound is the author's mistake, not something to reinterpret"
        );
    }

    /// Rhai ships no `clamp` at all, for any type.
    #[test]
    fn clamp_works_on_plain_numbers_too() {
        assert_eq!(rendered(|_| {}, "`{clamp(7, 1, 5)} {clamp(0.5, 1.0, 5.0)}").unwrap(), "5 1\n");
    }

    /// **The safety net.** Rhai answers a comparison between two different non-numeric
    /// types with a constant `false` — no error. A `while z > z_bottom` whose bound is
    /// accidentally a bare number would then be a loop that never runs, and the step
    /// would emit no cutting moves at all.
    ///
    /// Each case must be an `Err`. Asserting "nothing was emitted" would pass on exactly
    /// the bug this guards against.
    #[test]
    fn comparing_a_length_with_anything_else_is_an_error() {
        let setup = |s: &mut Scope| {
            s.push("z", Length::from_mm(1.0));
            s.push("rpm", RotationalSpeed::from_rpm(12_000.0));
            s.push("empty", gtl::rhai::Map::new());
        };

        for source in [
            "`{z > 5}",              // unit-ambiguous: five what?
            "`{5 > z}",              // and the same the other way round
            "`{z > 5.0}",
            "`{z == rpm}",           // two unit types, no shared meaning
            "`{z > \"x\"}",          // anything at all, not just the known types
            "`{z > empty.nope}",     // a missing field is `()` — the namespace typo case
            "if z > 5 {\n    `G1\n}", // in the position that actually costs a hole
        ] {
            let err = rendered(setup, source).unwrap_err();
            assert!(
                err.to_string().contains("Length"),
                "{source} must name the type it could not compare: {err}"
            );
        }

        // `!=` folds to a constant `true`, which is the same hole facing the other way.
        assert!(rendered(setup, "`{z != 5}").is_err());
    }

    /// Errors name the type as an author writes it, not as Rust spells it.
    #[test]
    fn an_unknown_accessor_names_the_type_the_author_knows() {
        let err = rendered(
            |s| {
                s.push("z", Length::from_mm(1.0));
            },
            "`{z.metres}",
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("'Length'"), "{message}");
        assert!(!message.contains("units::types"), "no Rust paths in an author's error: {message}");
    }

    /// An infinity would format as `inf` and reach the controller as a coordinate.
    #[test]
    fn dividing_by_zero_is_an_error_not_an_infinity() {
        let setup = |s: &mut Scope| {
            s.push("z", Length::from_mm(10.0));
            s.push("zero", Length::from_mm(0.0));
        };
        assert!(rendered(setup, "`Z{z / 0}").is_err());
        assert!(rendered(setup, "`Z{z / 0.0}").is_err());
        assert!(rendered(setup, "`{z / zero}").is_err());
    }

    /// The examples an author is shown before writing a template — the depth loop in
    /// `assets/help/gtl.md` and in `schemas/cnc.yaml`'s `primitives:` comment, and the
    /// branching example beside them.
    ///
    /// Both went in using names no primitive declares (`peck`, `z_feed`,
    /// `has_positioning_pins`), so anyone copying one met "variable not found". They are
    /// rendered here through `preview` — the same path the editor pane uses, with the
    /// same declared-variables-only scope — so a doc example that cannot run is a failing
    /// test rather than a support question.
    #[test]
    fn the_examples_shown_to_authors_actually_run() {
        let coder = Coder::new();

        let depth_loop = "`G0 X{x} Y{y}\n\
                          `G0 Z{z_retract}\n\
                          let z = z_retract;\n\
                          let step = (z_retract - z_bottom) / 3;\n\
                          while z > z_bottom {\n\
                              z = max(z - step, z_bottom);\n\
                              `G1 Z{z} F{z_feedrate}\n\
                              `G0 Z{z_retract}\n\
                          }";
        let out = coder
            .preview("drill", depth_loop, &crate::gcode::primitive_vars::variables_for("drill"))
            .expect("the depth-loop example");
        assert!(out.starts_with("G0 X10 Y10\nG0 Z10\n"), "{out}");

        let branching = "if clockwise {\n\
                             `G2 X{x} Y{y} I{i} J{j} F{xy_feedrate}\n\
                         } else {\n\
                             `G3 X{x} Y{y} I{i} J{j} F{xy_feedrate}\n\
                         }";
        let out = coder
            .preview("cut_arc", branching, &crate::gcode::primitive_vars::variables_for("cut_arc"))
            .expect("the branching example");
        assert_eq!(out, "G2 X10 Y10 I10 J10 F300\n");

        // The optional-prefix shape, from the help's "continuing a line" section.
        let prefix = "if z_bottom < z_retract {\n\
                          `(plunging) `\n\
                      }\n\
                      `G1 Z{z_bottom} F{z_feedrate}";
        let out = coder
            .preview("drill", prefix, &crate::gcode::primitive_vars::variables_for("drill"))
            .expect("the optional-prefix example");
        assert_eq!(out, "G1 Z10 F300\n", "the samples are equal, so the prefix is skipped");
    }

    /// The preview samples must be typed exactly as generation types them, or the editor
    /// rejects a template that would have run — the one failure a live validator must
    /// never produce.
    #[test]
    fn preview_samples_carry_their_unit_types() {
        let coder = Coder::new();
        let vars = crate::gcode::primitive_vars::variables_for("drill");
        let out = coder
            .preview("drill", "`F{z_feedrate.mm_per_min} Z{z_bottom.mm}", &vars)
            .expect("a typed sample supports the unit surface");
        assert_eq!(out, "F300 Z10\n");
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

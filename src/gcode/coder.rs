//! The Coder — the app-side GCode dialect over the generic `gtl` engine
//! (docs/gcode-engine.md §1.1). It turns a CNC profile's primitive templates into
//! GCode by registering the GCode surface on the engine and running each template
//! against a scope of resolved values.
//!
//! Two things are registered here: the **callable primitives** — `metric()`,
//! `imperial()` and `set_origin()` — which need the engine's output writer, and (for the
//! first two) the unit mode; and — delegated to [`crate::gcode::dialect`] — everything
//! the `units` types can do inside a script.
//!
//! What is *not* here yet is the scope model of docs/gcode-engine.md §2: a template
//! sees only the variables its call site hand-pushes, with no namespaced job context.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gtl::rhai::{EvalAltResult, ImmutableString, Position};
use gtl::{Gtl, GtlError, Scope, Template};
use units::{Angle, FeedRate, Length, RotationalSpeed, UserUnitSystem};

use crate::gcode::primitive_vars::{PrimitiveVar, VarType};
use crate::gcode::program::BodyError;
use crate::gcode::step_data::StepValue;

/// What the callable primitives emit, each rendered from its own profile template.
///
/// Empty by default — a machine with no unit statement and no origin selection. That is
/// the only honest default: the application has no business knowing that a G-code machine
/// spells these `G21` and `G54`.
#[derive(Default)]
struct EmitCommands {
    metric: String,
    imperial: String,
    origin: String,
    /// The `comment`/`message`/`pause` templates, in [`TEXT_CALLABLES`] order. These are
    /// **not** pre-rendered: their text is the call site's, so they are rendered when
    /// called. See [`Coder::build`].
    text: [String; 3],
}

/// The program-layer templates a [`Coder`] registers as callables, plus the one value they
/// read.
///
/// Grouped rather than passed as loose arguments because they arrive together, from the
/// same step's CNC and fixture, and because the list keeps growing.
#[derive(Default)]
pub struct ProgramPrimitives<'a> {
    /// The CNC's `set_unit` template — what `metric()`/`imperial()` emit.
    pub set_unit: &'a str,
    /// The CNC's `set_origin` template — what `set_origin()` emits.
    pub set_origin: &'a str,
    /// The step fixture's Machine Origin Reference, exactly as the operator entered it.
    /// `set_origin` normalises and validates it; passing it raw is what lets the
    /// template quote the original text back in an error.
    pub origin_reference: &'a str,
    /// The operator callables, each taking the text passed at the call site:
    /// `comment("…")`, `message("…")`, `pause("…")`.
    pub comment: &'a str,
    pub message: &'a str,
    pub pause: &'a str,
}

/// The three operator callables, by the name a template calls them and the template each
/// renders. Kept as one list so registering them is a loop rather than three near-copies.
const TEXT_CALLABLES: [&str; 3] = ["comment", "message", "pause"];

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
    /// A Coder whose callable primitives take effect but **emit nothing**, for callers
    /// that render primitives outside a program — the editor's preview, and the tests
    /// that are not about the unit statement or the origin.
    ///
    /// Generation uses [`Coder::with_program_primitives`], because what those calls emit
    /// is the profile's to say.
    pub fn new() -> Self {
        Self::build(EmitCommands::default())
    }

    /// A Coder whose `metric()`/`imperial()`/`set_origin()` emit the profile's own
    /// primitives.
    ///
    /// Each is rendered **here, once** (twice for `set_unit`, one result per unit) and the
    /// results held for the calls to emit. Rendering on demand instead would mean running
    /// a template from inside a registered function, and [`gtl::Gtl::run`] clears the
    /// shared output buffer on entry — the enclosing template's emitted lines would
    /// vanish. Rendering up front also means a broken template, or a `set_origin` that
    /// rejects the fixture's origin reference, is reported before any GCode exists rather
    /// than halfway through a program.
    ///
    /// An empty template means the machine has nothing to say: `metric()` then only sets
    /// the formatting mode (what a fixed-unit controller wants) and `set_origin()` emits
    /// nothing.
    pub fn with_program_primitives(primitives: &ProgramPrimitives) -> Result<Self, BodyError> {
        // Rendered by a throwaway Coder: `self` cannot render its own primitives before
        // it exists, and this one is identical in every respect that matters to them.
        let renderer = Self::new();

        let render_unit = |metric: bool| -> Result<String, BodyError> {
            if primitives.set_unit.trim().is_empty() {
                return Ok(String::new());
            }
            let mut scope = Scope::new();
            scope.push("metric", metric);
            scope.push("unit", if metric { "metric" } else { "imperial" }.to_string());
            renderer
                .render("set_unit", primitives.set_unit, &mut scope)
                .map_err(|error| render_error("set_unit", error))
        };

        let origin = if primitives.set_origin.trim().is_empty() {
            String::new()
        } else {
            let mut scope = Scope::new();
            scope.push("origin_reference", primitives.origin_reference.to_string());
            renderer
                .render("set_origin", primitives.set_origin, &mut scope)
                .map_err(|error| render_error("set_origin", error))?
        };

        Ok(Self::build(EmitCommands {
            metric: render_unit(true)?,
            imperial: render_unit(false)?,
            origin,
            text: [
                primitives.comment.to_string(),
                primitives.message.to_string(),
                primitives.pause.to_string(),
            ],
        }))
    }

    /// Registers the GCode surface:
    /// - `metric()` / `imperial()` — set how every later value formats, and emit
    ///   `commands`, which came from the profile.
    /// - `set_origin()` — emits the profile's validated origin selection.
    /// - the typed-value surface for `Length`, `FeedRate`, `RotationalSpeed` and
    ///   `Angle` — see [`crate::gcode::dialect`].
    fn build(commands: EmitCommands) -> Self {
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

        // The origin selection. No mode to set — unlike the unit, which changes how
        // every later value formats, this emits one statement and is done.
        let writer = gtl.writer();
        let origin = commands.origin;
        gtl.engine_mut().register_fn("set_origin", move || {
            // Already terminated by its own emit line, so raw — as for the unit words.
            writer.emit_raw(&origin);
        });

        // The operator callables — `comment("…")`, `message("…")`, `pause("…")`.
        //
        // Unlike the three above, these cannot be pre-rendered: their `text` is whatever
        // the call site passes. And they cannot render on *this* engine either, because
        // `Gtl::run` clears the shared output buffer on entry — rendering `comment` from
        // inside `program_begin` would discard everything `program_begin` had emitted so
        // far. So they render on a **second engine** with its own buffer, and the result is
        // pushed into this one.
        //
        // The sub-engine shares `unit_system`, so a length inside a comment formats in the
        // program's current mode rather than reverting to millimetres. It does *not* get
        // these callables registered on it, which makes recursion impossible by
        // construction: `comment()` inside a comment template is function-not-found, not a
        // hang.
        // Built only when a profile actually has one of these — an engine costs ~330 µs
        // and this Coder is rebuilt on every keystroke in the primitive editor, where all
        // three are empty. The calls are still *registered* either way, so previewing a
        // header that calls `comment(...)` behaves the same as one that calls
        // `set_origin()`: it emits nothing rather than failing as an unknown function.
        let sub = commands.text.iter().any(|t| !t.trim().is_empty()).then(|| {
            let mut sub_gtl = Gtl::new();
            crate::gcode::dialect::register(sub_gtl.engine_mut(), &unit_system);
            Rc::new(sub_gtl)
        });

        for (name, source) in TEXT_CALLABLES.into_iter().zip(commands.text) {
            let sub = sub.clone();
            let writer = gtl.writer();
            // Parsed on first call and kept: an operator callable is used a handful of
            // times per program, but a template that comments every tool block would
            // otherwise re-parse on each one.
            let compiled: RefCell<Option<Rc<Template>>> = RefCell::new(None);
            gtl.engine_mut().register_fn(
                name,
                move |text: ImmutableString| -> Result<(), Box<EvalAltResult>> {
                    // No template, no sub-renderer, nothing to emit — the machine has no
                    // word for this. Both conditions move together; see above.
                    let (Some(sub), false) = (sub.as_ref(), source.trim().is_empty()) else {
                        return Ok(());
                    };
                    let template = {
                        let mut slot = compiled.borrow_mut();
                        match slot.as_ref() {
                            Some(template) => template.clone(),
                            None => {
                                let template = Rc::new(sub.compile(name, &source).map_err(to_rhai)?);
                                *slot = Some(template.clone());
                                template
                            }
                        }
                    };
                    let mut scope = Scope::new();
                    scope.push("text", text.to_string());
                    writer.emit_raw(&sub.run(&template, &mut scope).map_err(to_rhai)?);
                    Ok(())
                },
            );
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

/// Surfaces a sub-render failure to the *calling* template.
///
/// A callable renders on its own engine, so its error has to be handed back across the
/// boundary as a Rhai error or the calling template would carry on as though the call had
/// succeeded — emitting a program with a silently missing comment, or worse, ignoring a
/// `throw` the profile author wrote as a precondition. `ErrorRuntime` is what a scripted
/// `throw` produces, so the caller's failure reads as one; the [`GtlError`] `Display`
/// already names the inner template, so the message says which one broke.
fn to_rhai(error: GtlError) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(error.to_string().into(), Position::NONE))
}

/// Turns a template failure into a [`BodyError`], keeping the message readable.
///
/// A scripted `throw` carries the author's own operator-facing sentence, and
/// [`BodyError::message`] already prefixes `primitive '<name>': ` — so the value is taken
/// bare. Passing `GtlError::Thrown` through `to_string()` instead would name the primitive
/// a second time (`primitive 'set_origin': set_origin: thrown: …`). Parse and runtime
/// errors keep their full rendering, because that is what carries the line number.
fn render_error(primitive: &str, error: GtlError) -> BodyError {
    let message = match error {
        GtlError::Thrown { value, .. } => value,
        other => other.to_string(),
    };
    BodyError::Render { primitive: primitive.to_string(), message }
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
            scope.push(var.name.clone(), sample_integer(&var.name));
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
        VarType::List => {
            scope.push(var.name.clone(), crate::gcode::step_data::to_array(&sample_steps()));
        }
    }
}

/// A representative machining profile for previewing `steps`.
///
/// **This is where the preview's guarantee is weaker than it is elsewhere, and it is
/// worth being plain about it.** For a scalar the sample scope holds exactly the names
/// the primitive declares, so a reference to anything else fails in the preview for the
/// same reason it would fail during generation. A list of objects has no such list of
/// names to hold — its fields are `machining.yaml`'s, and the profile being previewed
/// against does not exist yet. So the sample can only carry a *representative* shape,
/// and a mistyped field previews as "function not found: fmt(())" rather than as
/// "variable not found". Different message, same outcome: an error, before any program
/// exists. `sample_steps_match_the_schema` pins the shape against the schema so the two
/// cannot drift apart silently.
///
/// **Two** steps, not one, because the interesting thing a header can now say is which
/// of several setups it is — a one-step sample would preview `{step_index + 1} of
/// {steps.len()}` as "1 of 1" and tell the author nothing. `step_index` samples as 0
/// (see [`sample_integer`]), so the previewed program is the first of the two.
fn sample_steps() -> Vec<StepValue> {
    let allowance = |relative: &str, max: f64| {
        StepValue::Map(vec![
            ("relative".into(), StepValue::Text(relative.into())),
            ("max".into(), StepValue::Length(Length::from_mm(max))),
        ])
    };
    let holes = |oblong: &str| {
        StepValue::Map(vec![(
            "holes".into(),
            StepValue::Map(vec![
                ("route_fallback".into(), StepValue::Bool(true)),
                ("drill_first".into(), StepValue::Bool(true)),
                ("pilot".into(), StepValue::Bool(false)),
                ("oversize".into(), allowance("8%", 0.10)),
                ("undersize".into(), allowance("6%", 0.08)),
                ("oblong".into(), StepValue::Text(oblong.into())),
            ]),
        )])
    };
    let retention = |count: i64| {
        StepValue::Map(vec![
            ("mode".into(), StepValue::Text("tabs".into())),
            ("count".into(), StepValue::Int(count)),
            ("width".into(), StepValue::Length(Length::from_mm(2.0))),
            ("mouse_bites".into(), StepValue::Bool(false)),
        ])
    };
    let route_board = StepValue::Map(vec![
        (
            "outline".into(),
            StepValue::Map(vec![
                ("cut".into(), StepValue::Text("route".into())),
                ("vgroove_depth".into(), StepValue::Text("80%".into())),
                ("retention".into(), retention(4)),
            ]),
        ),
        (
            "cutouts".into(),
            StepValue::Map(vec![
                ("enabled".into(), StepValue::Bool(true)),
                ("retention".into(), retention(2)),
            ]),
        ),
        ("finishing".into(), StepValue::Length(Length::from_mm(0.1))),
    ]);
    let mill_board = StepValue::Map(vec![(
        "finishing".into(),
        StepValue::Map(vec![
            ("clearance".into(), StepValue::Length(Length::from_mm(0.1))),
            ("direction".into(), StepValue::Text("climb".into())),
        ]),
    )]);

    // The ids are the shape a real step carries — a UUID the operator never reads — and
    // the `_name` beside each is what a header would actually print. Both are sampled so
    // a template written against either previews truthfully.
    let step = |name: &str, face: &str, operations: &[&str], cnc: &str| {
        StepValue::Map(vec![
            ("name".into(), StepValue::Text(name.into())),
            ("cnc".into(), StepValue::Text("019f9d89-93d2-7441-bd17-1185d43a7bd8".into())),
            ("fixture".into(), StepValue::Text("019f9d89-93d2-7441-bd17-1185d43a7bd9".into())),
            ("toolset".into(), StepValue::Text("019f9d89-93d2-7441-bd17-1185d43a7bda".into())),
            ("board_face".into(), StepValue::Text(face.into())),
            (
                "operations".into(),
                StepValue::List(
                    operations.iter().map(|op| StepValue::Text((*op).into())).collect(),
                ),
            ),
            (
                "drill_locating_pins".into(),
                StepValue::Map(vec![(
                    "pin_diameter".into(),
                    // Text, not a Length: the schema offers a fixed list of pin sizes as
                    // an `enum`, so it is stored and read as the string the operator
                    // picked. A template printing it gets "3.2mm" verbatim rather than a
                    // value that follows the program's unit mode.
                    StepValue::Text("3.2mm".into()),
                )]),
            ),
            ("drill_pth".into(), holes("drill_ends_then_route")),
            ("drill_npth".into(), holes("drill_ends_then_route")),
            ("route_board".into(), route_board.clone()),
            (
                "route_cutouts".into(),
                StepValue::Map(vec![
                    ("retain_island".into(), StepValue::Bool(true)),
                    ("island_tab".into(), StepValue::Text("4%".into())),
                    ("drill_sharp_corners".into(), StepValue::Bool(true)),
                ]),
            ),
            ("mill_board".into(), mill_board.clone()),
            ("cnc_name".into(), StepValue::Text(cnc.into())),
            ("fixture_name".into(), StepValue::Text("Vacuum bed".into())),
            ("toolset_name".into(), StepValue::Text("PCB rack".into())),
        ])
    };

    vec![
        step("Drill", "front", &["drill_pth", "drill_npth"], "Sample mill"),
        step("Route outline", "front", &["route_board"], "Sample mill"),
    ]
}

/// A representative sample for an integer variable.
///
/// `index`/`count` are the pair a modal template branches on, so a bare `1` for both would
/// preview `index == 0` false *and* `index == count - 1` false — the middle case, which for
/// the shipped modal `drill` template emits nothing at all. An author would see an empty
/// preview for a correct template. First-of-three shows the branch that opens the cycle,
/// which is the one worth seeing.
///
/// `step_index` is first-of-two for the same reason, against the two-step sample from
/// [`sample_steps`]: at 0 a first-setup banner (`if step_index == 0`) *and* a footer
/// naming what comes next (`if step_index + 1 < steps.len()`) both preview visibly. At
/// the last step neither would, and a header that only ever previews as blank teaches
/// its author nothing.
fn sample_integer(name: &str) -> i64 {
    match name {
        "index" | "step_index" => 0,
        "count" => 3,
        _ => 1,
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
        ("line_format", "text") => "G1 X10 Y5 F600",
        // The real one, not a placeholder: the preview is meant to render what a real
        // run would, and this is the one sample value that is knowable exactly.
        (_, "k2g_version") => env!("CARGO_PKG_VERSION"),
        (_, "filename") => "board.kicad_pcb",
        (_, "timestamp") => "2026-01-01 12:00:00",
        (_, "manual_message") => "(change tool)",
        // `G55`, not `G54`: every bundled profile accepts it, including the Bantam,
        // which reserves G54 for the machine's own reference. A sample that some
        // machine's `set_origin` legitimately rejects would preview a valid template
        // as an error.
        (_, "origin_reference") => "G55",
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

    /// A Coder built from a `set_unit` template alone — for the tests that are about the
    /// unit statement and have nothing to say about the origin.
    fn unit_only(set_unit: &str) -> Result<Coder, BodyError> {
        Coder::with_program_primitives(&ProgramPrimitives { set_unit, ..Default::default() })
    }

    /// A Coder built from a `set_origin` template and the reference to feed it — for the
    /// tests that are about the origin and have nothing to say about units.
    fn origin_only(set_origin: &str, origin_reference: &str) -> Result<Coder, BodyError> {
        Coder::with_program_primitives(&ProgramPrimitives {
            set_origin,
            origin_reference,
            ..Default::default()
        })
    }

    // The `set_origin` templates under test are pulled out of the files that actually
    // ship, not retyped here. A copy would let a fix to a shipped profile pass a test
    // asserting the old behaviour — and these templates are the *only* thing standing
    // between a mistyped origin and a board cut in the wrong place.
    const CNC_SCHEMA_YAML: &str = include_str!("../../schemas/cnc.yaml");
    const MASSO_ATC_YAML: &str = include_str!("../../assets/cnc_templates/masso_g3_with_atc.yaml");
    const MASSO_NO_ATC_YAML: &str = include_str!("../../assets/cnc_templates/masso_g3_no_atc.yaml");
    const GENMITSU_YAML: &str = include_str!("../../assets/cnc_templates/genmitsu_3018.yaml");
    const BATAM_YAML: &str = include_str!("../../assets/cnc_templates/batam.yaml");

    /// The `set_origin` template from a bundled CNC profile.
    fn profile_set_origin(yaml: &str) -> String {
        let doc: serde_yaml::Value = serde_yaml::from_str(yaml).expect("template parses");
        doc["primitives"]["set_origin"]
            .as_str()
            .expect("the profile declares set_origin")
            .to_string()
    }

    /// The `set_origin` **schema default** — what a profile written before this primitive
    /// existed gets injected on load, and so what most machines will actually run.
    fn schema_default_set_origin() -> String {
        let doc: serde_yaml::Value =
            serde_yaml::from_str(CNC_SCHEMA_YAML).expect("the CNC schema parses");
        doc["properties"]["primitives"]["properties"]["set_origin"]["default"]
            .as_str()
            .expect("set_origin carries a default")
            .to_string()
    }

    /// Renders `set_origin` for `reference`, as generation would: the whole Coder is built,
    /// so a rejected reference surfaces exactly as it does in a real run.
    fn origin_output(template: &str, reference: &str) -> Result<String, BodyError> {
        let coder = origin_only(template, reference)?;
        coder
            .render("t", "set_origin();", &mut Scope::new())
            .map_err(|error| render_error("t", error))
    }

    /// The `initialise` primitive renders with the program-layer values, and
    /// `metric()` emits the profile's word for it and fixes the length unit for
    /// `{z_safe}`.
    #[test]
    fn renders_the_initialise_header() {
        let coder = unit_only(&crate::gcode::program::sample_set_unit_tpl()).unwrap();
        let mut scope = Scope::new();
        scope.push("filename", "demo.kicad_pcb".to_string());
        scope.push("timestamp", "2026-01-01 00:00:00".to_string());
        scope.push("z_safe", Length::from_mm(5.0));

        let source = "`(k2g {filename} - {timestamp})\nmetric();\n`G0 Z{z_safe}";
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
        let coder = unit_only(&crate::gcode::program::sample_set_unit_tpl()).unwrap();
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
        let coder = unit_only("`{if metric { \"METRIC,TZ\" } else { \"INCH,TZ\" }}").unwrap();
        let out = coder.render("t", "metric();\n`T01C0.8", &mut Scope::new()).unwrap();
        assert_eq!(out, "METRIC,TZ\nT01C0.8\n", "an Excellon header, from the profile alone");
    }

    /// A controller with no unit statement — fixed to one system — leaves the primitive
    /// empty. The calls then only set the formatting, and nothing is emitted, not even a
    /// blank line.
    #[test]
    fn an_empty_unit_primitive_emits_nothing() {
        let coder = unit_only("   ").unwrap();
        let mut scope = Scope::new();
        scope.push("z", Length::from_mm(25.4));
        assert_eq!(coder.render("t", "imperial();\n`Z{z}", &mut scope).unwrap(), "Z1\n");
    }

    /// A broken `set_unit` is reported before a single line of the program exists,
    /// against the primitive that could not render — not as a mystery halfway through.
    #[test]
    fn a_broken_unit_primitive_is_named() {
        match unit_only("`{nope}") {
            Err(BodyError::Render { primitive, .. }) => assert_eq!(primitive, "set_unit"),
            Err(other) => panic!("expected a Render error, got {other:?}"),
            Ok(_) => panic!("a set_unit naming an unknown value must not build a Coder"),
        }
    }

    // ---- set_origin: the machine origin selection --------------------------------

    /// `set_origin()` emits where the call sits, not at the top — the whole reason it is a
    /// registered function rather than a separately-rendered primitive. If the pre-render
    /// leaked into the enclosing template's buffer, the origin would land in the wrong
    /// place (or the surrounding lines would vanish).
    #[test]
    fn an_initialise_calling_set_origin_emits_the_reference_in_place() {
        let coder = origin_only("`{origin_reference}", "G56").unwrap();
        let mut scope = Scope::new();
        scope.push("z_safe", Length::from_mm(5.0));
        let out = coder
            .render("initialise", "`G17 G40 G49 G80 G90\nset_origin();\n`G0 Z{z_safe}", &mut scope)
            .unwrap();
        assert_eq!(out, "G17 G40 G49 G80 G90\nG56\nG0 Z5\n");
    }

    /// A machine that selects no origin leaves the primitive empty; the call then emits
    /// nothing at all, not even a blank line.
    #[test]
    fn an_empty_origin_primitive_emits_nothing() {
        let coder = origin_only("   ", "G54").unwrap();
        assert_eq!(coder.render("t", "`A\nset_origin();\n`B", &mut Scope::new()).unwrap(), "A\nB\n");
    }

    /// The schema default — what a profile written before `set_origin` existed runs once
    /// the default is injected — accepts every offset a plain G-code controller has.
    #[test]
    fn the_schema_default_accepts_the_six_standard_offsets() {
        let template = schema_default_set_origin();
        for reference in ["G54", "G55", "G56", "G57", "G58", "G59"] {
            assert_eq!(
                origin_output(&template, reference).unwrap(),
                format!("{reference}\n"),
                "{reference} is one of the six"
            );
        }
    }

    /// Normalisation, on the shipped default: the operator's spacing and case are theirs
    /// to get wrong.
    #[test]
    fn an_origin_reference_is_trimmed_and_upper_cased() {
        let template = schema_default_set_origin();
        for entered in ["  G55", "g55  ", " g55 ", "G 5 5"] {
            assert_eq!(
                origin_output(&template, entered).unwrap(),
                "G55\n",
                "'{entered}' is the operator writing G55"
            );
        }
    }

    /// The MASSO's extended bank, which is the case the old ordinal could not express at
    /// all. The space is stripped for matching and put back for the output, so `sub_string(6)`
    /// has to land on the P-number at both one and three digits.
    #[test]
    fn the_masso_form_accepts_and_respaces_the_extended_offsets() {
        let template = profile_set_origin(MASSO_ATC_YAML);
        for (entered, expected) in [
            ("G54.1 P1", "G54.1 P1\n"),
            ("G54.1 P7", "G54.1 P7\n"),
            ("g54.1p100", "G54.1 P100\n"),
            ("G54.1P42", "G54.1 P42\n"),
            // The plain six still work on the same machine.
            ("G54", "G54\n"),
            ("G59", "G59\n"),
        ] {
            assert_eq!(origin_output(&template, entered).unwrap(), expected, "entered '{entered}'");
        }
    }

    /// The two MASSO profiles differ only in their tool changer, so their origin handling
    /// must not drift apart — a fix applied to one and not the other is a silent
    /// inconsistency between two profiles the same operator is likely to use.
    ///
    /// Compared by **what they do**, not by their source text. The two templates are the
    /// same machine's rule written by hand twice, so they legitimately differ in comment
    /// wording, in where a long `throw` message wraps, and in whether the extended form is
    /// emitted as one line or as a raw fragment plus its P-number — none of which an
    /// operator can observe. What must never differ is which references each accepts and
    /// what each emits for them, so that is what is asserted, over the whole range the
    /// MASSO documents plus the forms an operator might type by hand.
    ///
    /// A byte comparison stood here before and had to go: it failed on a reworded comment,
    /// which trains the reader to update the expectation without reading it — exactly the
    /// habit that lets a real drift through.
    #[test]
    fn both_masso_profiles_select_the_origin_identically() {
        let atc = profile_set_origin(MASSO_ATC_YAML);
        let no_atc = profile_set_origin(MASSO_NO_ATC_YAML);

        for entered in [
            // The plain bank, its ends, and one past each end.
            "G53", "G54", "G59", "G60",
            // The extended bank: both ends, one past, and the spacing/case an operator
            // actually types.
            "G54.1 P1", "G54.1 P100", "G54.1 P101", "g54.1p42", "  G54.1P7  ",
            // The blank that must be refused rather than rendered empty.
            "", "   ",
            // Not an offset at all.
            "M06",
        ] {
            match (origin_output(&atc, entered), origin_output(&no_atc, entered)) {
                (Ok(a), Ok(b)) => assert_eq!(a, b, "'{entered}' must emit the same on both"),
                (Err(_), Err(_)) => {}
                (a, b) => panic!(
                    "'{entered}' is accepted by one MASSO profile and refused by the other: \
                     ATC {a:?}, no-ATC {b:?}"
                ),
            }
        }
    }

    /// A blank reference is the dangerous case: left unchecked the program runs against
    /// whatever origin the controller happens to have active. Every shipped profile must
    /// refuse it — asserted as an `Err`, never as an empty render, because an empty render
    /// *is* the bug.
    #[test]
    fn a_blank_origin_reference_refuses_to_generate() {
        for (name, template) in [
            ("schema default", schema_default_set_origin()),
            ("masso", profile_set_origin(MASSO_ATC_YAML)),
            ("genmitsu", profile_set_origin(GENMITSU_YAML)),
            ("batam", profile_set_origin(BATAM_YAML)),
        ] {
            for blank in ["", "   "] {
                let error = origin_output(&template, blank)
                    .expect_err(&format!("{name} must refuse a blank origin reference"));
                assert!(
                    error.message().contains("no origin reference"),
                    "{name} must say what is wrong, got: {}",
                    error.message()
                );
            }
        }
    }

    /// An offset the machine does not have, on every shipped profile. The message must
    /// quote what the operator actually typed — which is why the template is handed the
    /// raw value rather than a normalised one.
    #[test]
    fn an_offset_the_machine_does_not_have_refuses_to_generate() {
        for (name, template) in [
            ("schema default", schema_default_set_origin()),
            ("masso", profile_set_origin(MASSO_ATC_YAML)),
            ("genmitsu", profile_set_origin(GENMITSU_YAML)),
            ("batam", profile_set_origin(BATAM_YAML)),
        ] {
            for bad in ["G60", "G53", "X54", "54", "G5", "G54.2 P1"] {
                let error = origin_output(&template, bad)
                    .expect_err(&format!("{name} must refuse '{bad}'"));
                assert!(
                    error.message().contains(bad),
                    "{name} must quote what was entered ('{bad}'), got: {}",
                    error.message()
                );
            }
        }
    }

    /// The extended bank is bounded at both ends, and only exists on the machine that has
    /// it — a GRBL box asked for `G54.1 P1` must say so rather than emit a word it cannot
    /// honour.
    #[test]
    fn the_extended_bank_is_bounded_and_machine_specific() {
        let masso = profile_set_origin(MASSO_ATC_YAML);
        for out_of_range in ["G54.1 P0", "G54.1 P101", "G54.1 P200"] {
            assert!(
                origin_output(&masso, out_of_range).is_err(),
                "the MASSO bank stops at P100, so '{out_of_range}' is out of range"
            );
        }

        for (name, template) in [
            ("genmitsu", profile_set_origin(GENMITSU_YAML)),
            ("batam", profile_set_origin(BATAM_YAML)),
            ("schema default", schema_default_set_origin()),
        ] {
            assert!(
                origin_output(&template, "G54.1 P1").is_err(),
                "{name} has no extended bank and must refuse G54.1 P1"
            );
        }
    }

    /// The Bantam reserves G54 for the machine's own reference, so its first usable offset
    /// is G55. Expressing that was impossible while the fixture held an ordinal and each
    /// profile mapped it arithmetically — it is the case that justifies the whole change.
    #[test]
    fn the_bantam_reserves_g54_for_the_machine() {
        let batam = profile_set_origin(BATAM_YAML);
        let error = origin_output(&batam, "G54").expect_err("G54 is the machine's own reference");
        assert!(
            error.message().contains("reserves G54"),
            "the refusal must say why, got: {}",
            error.message()
        );
        assert_eq!(origin_output(&batam, "G55").unwrap(), "G55\n", "G55 is the first usable one");
    }

    /// The operator sees the author's sentence once, prefixed by the primitive that raised
    /// it — not `primitive 'set_origin': set_origin: thrown: …`, which is what passing a
    /// `Thrown` through `Display` produces.
    #[test]
    fn a_thrown_origin_is_reported_once_not_three_times() {
        // `Coder` is not `Debug`, so the Ok arm cannot be unwrapped into a panic message.
        let Err(error) = origin_only("throw \"nope\";", "G54") else {
            panic!("a set_origin that throws must not build a Coder");
        };
        match &error {
            BodyError::Render { primitive, message } => {
                assert_eq!(primitive, "set_origin");
                assert_eq!(message, "nope", "the thrown value, bare");
            }
            other => panic!("expected a Render error, got {other:?}"),
        }
        assert_eq!(error.message(), "primitive 'set_origin': nope");
    }

    /// A `set_origin` with a real fault (not a `throw`) keeps its line number, which is
    /// what the author needs and what the thrown path has no room for.
    #[test]
    fn a_broken_origin_primitive_keeps_its_line_number() {
        match origin_only("`A\n`{nope}", "G54").map(|_| ()) {
            Err(BodyError::Render { primitive, message }) => {
                assert_eq!(primitive, "set_origin");
                assert!(message.contains(":2"), "the fault is on line 2, got: {message}");
            }
            other => panic!("expected a Render error naming the line, got {other:?}"),
        }
    }

    /// A one-step job's header says so by saying nothing.
    ///
    /// The MASSO banner is assembled from emit-raw fragments around an `if steps.len() > 1`,
    /// so the single-step case — much the commonest job — takes a different path through
    /// the template from the one pinned above. Left untested it could emit "step 1 of 1",
    /// a stray separator, or an unclosed comment, and the two-step pin would still pass.
    #[test]
    fn a_one_step_job_gets_a_header_with_no_step_clause() {
        let doc: serde_yaml::Value =
            serde_yaml::from_str(MASSO_ATC_YAML).expect("template parses");
        let template = doc["primitives"]["program_begin"].as_str().expect("declared");

        let coder = Coder::with_program_primitives(&ProgramPrimitives {
            set_unit: doc["primitives"]["set_unit"].as_str().expect("declared"),
            set_origin: doc["primitives"]["set_origin"].as_str().expect("declared"),
            origin_reference: "G55",
            ..Default::default()
        })
        .expect("the MASSO profile builds");

        let mut scope = Scope::new();
        scope.push("filename", "demo.kicad_pcb".to_string());
        scope.push("timestamp", "2026-01-01 00:00:00".to_string());
        scope.push("z_safe", Length::from_mm(20.0));
        scope.push("origin_reference", "G55".to_string());
        scope.push(
            "steps",
            crate::gcode::step_data::to_array(&[StepValue::Map(vec![(
                "name".into(),
                StepValue::Text("Drill PTH".into()),
            )])]),
        );
        scope.push("step_index", 0_i64);

        let header = coder.render("program_begin", template, &mut scope).expect("renders");
        assert_eq!(
            header.lines().next().unwrap_or_default(),
            "(Created by K2G from 'demo' - 2026-01-01 00:00:00)",
            "one step names the board and the time, and nothing about steps:\n{header}"
        );
    }

    /// Each shipped profile's **own `initialise`** renders with its **own `set_origin`** —
    /// the integration point the individual tests above each cover half of. A profile whose
    /// header forgot the call, or still mapped an ordinal, would pass every other test here
    /// and then emit a program with no origin selected at all.
    #[test]
    fn every_shipped_profile_header_selects_its_origin() {
        // Every shipped `program_begin` also calls `metric()`, so the real `set_unit` has
        // to come along — this renders the profile as it actually ships, not a subset.
        //
        // A missing key **panics** rather than defaulting to empty: an empty template
        // renders as an empty header, which would let a renamed-but-not-updated key pass
        // half the assertions below by producing nothing at all to disagree with.
        let primitive = |yaml: &str, name: &str| -> String {
            let doc: serde_yaml::Value = serde_yaml::from_str(yaml).expect("template parses");
            doc["primitives"][name]
                .as_str()
                .unwrap_or_else(|| panic!("the profile declares no '{name}' primitive"))
                .to_string()
        };

        // `G55` is the one reference all four accept (the Bantam reserves G54).
        for (name, yaml) in [
            ("masso_g3_with_atc", MASSO_ATC_YAML),
            ("masso_g3_no_atc", MASSO_NO_ATC_YAML),
            ("genmitsu_3018", GENMITSU_YAML),
            ("batam", BATAM_YAML),
        ] {
            let coder = Coder::with_program_primitives(&ProgramPrimitives {
                set_unit: &primitive(yaml, "set_unit"),
                set_origin: &primitive(yaml, "set_origin"),
                origin_reference: "G55",
                ..Default::default()
            })
            .unwrap_or_else(|error| panic!("{name}: {}", error.message()));

            // The whole program-layer scope, matching what `render_step_program` builds —
            // two steps, and this program is the first of them, so a header that names
            // its setup is exercised rather than merely tolerated.
            let step = |step_name: &str| {
                StepValue::Map(vec![
                    ("name".into(), StepValue::Text(step_name.into())),
                    ("board_face".into(), StepValue::Text("front".into())),
                    ("cnc_name".into(), StepValue::Text("MASSO G3".into())),
                ])
            };
            let mut scope = Scope::new();
            scope.push("filename", "demo.kicad_pcb".to_string());
            scope.push("timestamp", "2026-01-01 00:00:00".to_string());
            scope.push("z_safe", Length::from_mm(20.0));
            scope.push("origin_reference", "G55".to_string());
            scope.push(
                "steps",
                crate::gcode::step_data::to_array(&[step("Drill PTH"), step("Route outline")]),
            );
            scope.push("step_index", 0_i64);

            let header = coder
                .render("program_begin", &primitive(yaml, "program_begin"), &mut scope)
                .unwrap_or_else(|error| panic!("{name} header must render: {error}"));

            assert!(
                header.lines().any(|line| line.trim() == "G55"),
                "{name}'s header must select the fixture's origin on a line of its own:\n{header}"
            );

            // One profile pinned exactly, so a stray blank line or a doubled origin shows
            // up as a diff rather than passing the "contains G55" check above. The MASSO
            // is the one that names its step, so the pin also covers the emit-raw run
            // that assembles the banner from four separate fragments — the place a stray
            // newline or a lost space would otherwise hide.
            //
            // The order is the safety contract and is asserted as such:
            //   1. modal state, moving nothing (G94 included, so an inherited G95
            //      cannot turn every F word into feed-per-revolution)
            //   2. units, before any coordinate
            //   3. only then the retract, in MACHINE coordinates and with an
            //      explicit G0 — `G53` is non-modal and supplies no motion, so a
            //      bare `G53 Z0` after a program that left G1 modal is a feed move
            //   4. the work offset last; nothing before it needed one
            // and no trailing descent to a work-frame height: G53 Z0 already parked
            // the tool at the top of travel, which is clear of everything.
            if name == "masso_g3_with_atc" {
                assert_eq!(
                    header,
                    "(Created by K2G from 'demo' - step 1 of 2: Drill PTH - \
                     2026-01-01 00:00:00)\n\
                     (Target: MASSO G3 firmware 5.13)\n\
                     G90 G94 G17 G40 G80\n\
                     G21\n\
                     G53 G0 Z0\n\
                     G55\n"
                );
            }
        }
    }

    /// The machine-state invariants every shipped profile must hold, checked against the
    /// text each one actually emits.
    ///
    /// This is the test that was missing. A build guard already compiled every shipped
    /// template, and every template compiled — while three of the four emitted programs
    /// that were unsafe or unrunnable, because nothing looked at the *output*. One of them
    /// crashed a machine: `M06` on a controller with an automatic tool setter ends with
    /// the spindle over the setter, and the next drill cycle's R-plane rapid descended
    /// into it.
    ///
    /// The assertions are deliberately about machine state rather than exact text, so a
    /// profile is free to spell things its own way — but not free to leave the tool
    /// somewhere unknown.
    #[test]
    fn every_shipped_profile_holds_the_machine_state_invariants() {
        let primitive = |yaml: &str, name: &str| -> String {
            let doc: serde_yaml::Value = serde_yaml::from_str(yaml).expect("template parses");
            doc["primitives"][name]
                .as_str()
                .unwrap_or_else(|| panic!("the profile declares no '{name}' primitive"))
                .to_string()
        };

        // Detecting a move is fiddlier than it looks, and both traps below were hit
        // while writing this test:
        //
        //   - A substring test does not survive contact with G-code. "G17 G40 G80"
        //     contains "G1", so `contains("G1")` reads the safety line as a feed move.
        //   - Whole-word matching alone is not enough either: the MASSO banner reads
        //     "(Target: MASSO G3 firmware 5.13)", and "G3" is the machine's name here,
        //     not an arc.
        //
        // So a move is a motion word AND an axis word. Comments and modal-state lines
        // have neither.
        let has_axis = |line: &str| line.contains('X') || line.contains('Y') || line.contains('Z');
        let motion_word = |line: &str, words: &[&str]| {
            line.split_whitespace().any(|word| words.contains(&word))
        };
        let is_motion = |line: &str| {
            motion_word(line, &["G0", "G00", "G1", "G01", "G2", "G02", "G3", "G03"]) && has_axis(line)
        };
        let is_rapid = |line: &str| motion_word(line, &["G0", "G00"]) && has_axis(line);

        for (name, yaml, homes) in [
            ("masso_g3_with_atc", MASSO_ATC_YAML, true),
            ("masso_g3_no_atc", MASSO_NO_ATC_YAML, true),
            ("genmitsu_3018", GENMITSU_YAML, false),
            ("batam", BATAM_YAML, false),
        ] {
            let coder = Coder::with_program_primitives(&ProgramPrimitives {
                set_unit: &primitive(yaml, "set_unit"),
                set_origin: &primitive(yaml, "set_origin"),
                origin_reference: "G55",
                pause: &primitive(yaml, "pause"),
                ..Default::default()
            })
            .unwrap_or_else(|error| panic!("{name}: {}", error.message()));

            // ---- program_begin ------------------------------------------------------
            let mut scope = Scope::new();
            scope.push("filename", "demo.kicad_pcb".to_string());
            scope.push("timestamp", "2026-01-01 00:00:00".to_string());
            scope.push("z_safe", Length::from_mm(20.0));
            scope.push("origin_reference", "G55".to_string());
            scope.push("steps", crate::gcode::step_data::to_array(&[]));
            scope.push("step_index", 0_i64);
            let header = coder
                .render("program_begin", &primitive(yaml, "program_begin"), &mut scope)
                .unwrap_or_else(|e| panic!("{name} header: {e}"));

            // G94 pins feed-per-minute. Inherited G95 turns every F into feed-per-rev.
            assert!(
                header.contains("G94"),
                "{name}: the header must select feed-per-minute:\n{header}"
            );
            // Units before any coordinate is emitted.
            let unit_line = header
                .lines()
                .position(|l| l.split_whitespace().any(|w| w == "G21" || w == "G20"));
            let first_move = header.lines().position(is_motion);
            if let (Some(unit), Some(mv)) = (unit_line, first_move) {
                assert!(unit < mv, "{name}: units must precede the first move:\n{header}");
            }

            // ---- tool_change --------------------------------------------------------
            let mut scope = Scope::new();
            scope.push("manual_message", "(load tool T2)".to_string());
            scope.push("slot", 2_i64);
            scope.push("rpm", RotationalSpeed::from_rpm(24000.0));
            let change = coder
                .render("tool_change", &primitive(yaml, "tool_change"), &mut scope)
                .unwrap_or_else(|e| panic!("{name} tool_change: {e}"));

            // A modal cycle survives a tool change, and any macro motion during the
            // change would then be read as another hole. Only asked of a profile whose
            // drill actually opens one — GRBL and TinyG have no canned cycles, and a
            // G80 there would be cargo-cult G-code.
            let uses_canned_cycle = primitive(yaml, "drill").contains("G81");
            if uses_canned_cycle {
                assert!(
                    change.contains("G80"),
                    "{name}: tool_change must cancel the modal cycle before changing \
                     tool — G81 survives M06, so macro motion during the change is read \
                     as another hole:\n{change}"
                );
            }
            // And it must end at a known, clear height — the crash.
            let last_move = change
                .lines()
                .rev()
                .find(|l| is_rapid(l))
                .unwrap_or_else(|| panic!("{name}: tool_change makes no retract:\n{change}"));
            assert!(
                last_move.contains('Z'),
                "{name}: tool_change must end with a Z retract:\n{change}"
            );
            if homes {
                assert!(
                    last_move.contains("G53"),
                    "{name} homes, so its tool_change retract must be in machine \
                     coordinates — a work-frame Z resolves against an offset M06 may \
                     have changed:\n{change}"
                );
            } else {
                assert!(
                    !change.contains("G53"),
                    "{name} does not declare has_repeatable_home, so machine \
                     coordinates are wherever it powered on. G53 here is not a safe \
                     move, it is an arbitrary one:\n{change}"
                );
            }

            // ---- drill --------------------------------------------------------------
            let render_hole = |index: i64, count: i64| {
                let mut scope = Scope::new();
                scope.push("x", Length::from_mm(10.0));
                scope.push("y", Length::from_mm(20.0));
                scope.push("z_bottom", Length::from_mm(-2.4));
                scope.push("z_retract", Length::from_mm(1.0));
                scope.push("z_feedrate", FeedRate::from_mm_per_min(300.0));
                scope.push("index", index);
                scope.push("count", count);
                coder
                    .render("drill", &primitive(yaml, "drill"), &mut scope)
                    .unwrap_or_else(|e| panic!("{name} drill: {e}"))
            };
            let first = render_hole(0, 3);
            let last = render_hole(2, 3);

            // Whichever form the profile uses, the block must be safe to enter from an
            // unknown Z and must not leave a cycle live.
            if first.contains("G81") {
                assert!(
                    first.contains("G99"),
                    "{name}: a canned cycle must select R-plane return explicitly — \
                     inherited G98 climbs back to the entry level after every hole, and \
                     after a tool change that level is the top of travel:\n{first}"
                );
                assert!(
                    last.contains("G80"),
                    "{name}: the last hole must cancel the cycle:\n{last}"
                );
            } else {
                // Expanded form (no canned cycles on this controller): the first hole
                // has to lift before it traverses.
                let lift = first.lines().position(|l| is_rapid(l) && l.contains('Z'));
                let traverse = first.lines().position(|l| is_rapid(l) && l.contains('X'));
                assert!(
                    matches!((lift, traverse), (Some(l), Some(t)) if l < t),
                    "{name}: the first hole must retract before it traverses, so the \
                     block is safe to enter from an unknown Z:\n{first}"
                );
            }

            // ---- program_end --------------------------------------------------------
            let mut scope = Scope::new();
            scope.push("filename", "demo.kicad_pcb".to_string());
            scope.push("timestamp", "2026-01-01 00:00:00".to_string());
            scope.push("z_safe", Length::from_mm(20.0));
            scope.push("origin_reference", "G55".to_string());
            scope.push("steps", crate::gcode::step_data::to_array(&[]));
            scope.push("step_index", 0_i64);
            let footer = coder
                .render("program_end", &primitive(yaml, "program_end"), &mut scope)
                .unwrap_or_else(|e| panic!("{name} program_end: {e}"));

            // Z lifts before X/Y, and in its own block: a combined move is a diagonal
            // that drags the tool across the work on the way out.
            let park_z = footer.lines().position(|l| l.contains('Z'));
            let park_xy = footer.lines().position(|l| l.contains('X') || l.contains('Y'));
            if let (Some(z), Some(xy)) = (park_z, park_xy) {
                assert!(z < xy, "{name}: the footer must lift before parking:\n{footer}");
                assert!(
                    !footer.lines().nth(z).unwrap().contains('X'),
                    "{name}: the footer's retract must not also traverse:\n{footer}"
                );
            }
            assert!(
                footer.contains("M2") || footer.contains("M30"),
                "{name}: the program must end with an end-of-program word:\n{footer}"
            );
        }
    }

    /// The editor's validate/preview must accept every shipped `set_origin`. It renders
    /// against the sample scope, so the sample reference has to be one that *no* bundled
    /// profile rejects — otherwise a valid template previews as an error.
    #[test]
    fn set_origin_previews_with_a_sample_reference() {
        let coder = Coder::new();
        let vars = crate::gcode::primitive_vars::variables_for("set_origin");
        assert!(!vars.is_empty(), "set_origin declares its variable in the schema");
        for (name, template) in [
            ("schema default", schema_default_set_origin()),
            ("masso", profile_set_origin(MASSO_ATC_YAML)),
            ("genmitsu", profile_set_origin(GENMITSU_YAML)),
            ("batam", profile_set_origin(BATAM_YAML)),
        ] {
            let preview = coder.preview("set_origin", &template, &vars);
            assert!(preview.is_ok(), "{name} must preview cleanly, got: {preview:?}");
        }
    }

    // ---- runaway templates ---------------------------------------------------------

    /// **The editor must not be hangable by a template.**
    ///
    /// `preview` runs on the UI thread and re-renders on every keystroke, so a template
    /// with a loop is being executed *while it is half-written* — `while z > z_bottom {`
    /// is a runaway for as long as it takes to type the body. Without the engine's
    /// operation ceiling this call would never return and the application would be gone,
    /// with no way back to the template that did it.
    #[test]
    fn a_runaway_template_cannot_hang_the_editor_preview() {
        let coder = Coder::new();
        let vars = crate::gcode::primitive_vars::variables_for("drill");
        let started = std::time::Instant::now();

        // The half-typed peck loop: the body that would decrease `z` is not written yet,
        // so the condition can never become false. (`>=` rather than `>` because the
        // preview samples every length as 10 mm, which would make `>` false at once — the
        // shape under test is the loop that *does* start.)
        let result =
            coder.preview("drill", "let z = z_retract;\nwhile z >= z_bottom {\n    `G1 Z{z}\n}", &vars);

        assert!(result.is_err(), "an endless loop must not preview as a program");
        assert!(
            result.unwrap_err().to_string().contains("did not finish"),
            "and must say why, in terms the author can act on"
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "a keystroke must not stall: took {:?}",
            started.elapsed()
        );
    }

    /// The same ceiling applies during generation, and the failure names the primitive —
    /// a board is thousands of renders, and "which template" is the only useful thing to
    /// say about one that would not stop.
    #[test]
    fn a_runaway_primitive_fails_generation_against_its_own_name() {
        let coder = Coder::new();
        let error = crate::gcode::program::format_lines(&coder, "G21", "while true {\n    `x\n}");
        match error {
            Err(BodyError::Render { primitive, message }) => {
                assert_eq!(primitive, "line_format");
                assert!(message.contains("did not finish"), "{message}");
            }
            other => panic!("expected a named Render error, got {other:?}"),
        }
    }

    // ---- the operator callables --------------------------------------------------

    /// A Coder with the three operator callables wired to simple templates.
    fn with_operator_callables() -> Coder {
        Coder::with_program_primitives(&ProgramPrimitives {
            comment: "`( {text} )",
            message: "`MSG {text}",
            pause: "`MSG {text}\n`M01",
            ..Default::default()
        })
        .expect("plain templates build")
    }

    /// **The regression guard for the sub-renderer.** `comment("…")` renders a template
    /// from inside another template, which [`gtl::Gtl::run`] cannot do on one engine —
    /// it clears the shared output buffer on entry, so the enclosing template's lines
    /// would vanish. This asserts they do not.
    #[test]
    fn a_callable_emits_at_the_call_site_without_losing_the_enclosing_output() {
        let coder = with_operator_callables();
        let out = coder
            .render(
                "program_begin",
                "`G17 G90\ncomment(\"Outline pass\");\n`G0 Z5",
                &mut Scope::new(),
            )
            .unwrap();
        assert_eq!(out, "G17 G90\n( Outline pass )\nG0 Z5\n");
    }

    /// Each callable renders its own primitive, and a multi-line one keeps its shape.
    #[test]
    fn each_operator_callable_renders_its_own_primitive() {
        let coder = with_operator_callables();
        let out = coder
            .render(
                "t",
                "comment(\"c\");\nmessage(\"m\");\npause(\"p\");",
                &mut Scope::new(),
            )
            .unwrap();
        assert_eq!(out, "( c )\nMSG m\nMSG p\nM01\n");
    }

    /// A machine with no word for one of these leaves the template empty; the call then
    /// emits nothing at all rather than a blank line.
    #[test]
    fn an_empty_operator_template_emits_nothing() {
        let coder = Coder::with_program_primitives(&ProgramPrimitives {
            comment: "   ",
            ..Default::default()
        })
        .unwrap();
        assert_eq!(coder.render("t", "`A\ncomment(\"x\");\n`B", &mut Scope::new()).unwrap(), "A\nB\n");
    }

    /// The sub-engine shares the unit mode, so a length inside a comment formats the way
    /// the surrounding program does. A separate engine with its own mode would silently
    /// print millimetres into an inch program.
    #[test]
    fn a_callable_formats_lengths_in_the_programs_current_unit() {
        let coder = Coder::with_program_primitives(&ProgramPrimitives {
            set_unit: "`{if metric { \"G21\" } else { \"G20\" }}",
            comment: "`( depth {d} )",
            ..Default::default()
        })
        .unwrap();
        let mut scope = Scope::new();
        scope.push("d", Length::from_mm(25.4));
        let out = coder.render("t", "imperial();\ncomment(\"ignored\");", &mut scope);
        // `d` is the caller's variable, not the callable's — the callable sees only
        // `text` — so this proves the *mode* is shared, via a template of its own.
        assert!(out.is_err(), "a callable's scope holds only `text`");

        // With the length inside the caller instead, the shared mode shows up directly.
        let mut scope = Scope::new();
        scope.push("d", Length::from_mm(25.4));
        let out = coder.render("t", "imperial();\n`Z{d}", &mut scope).unwrap();
        assert_eq!(out, "G20\nZ1\n", "25.4 mm reads as 1 inch after imperial()");
    }

    /// A `throw` inside a callable's template must refuse the program, not be swallowed at
    /// the engine boundary and leave the caller carrying on as though it had emitted.
    #[test]
    fn a_throw_inside_a_callable_refuses_the_program() {
        let coder = Coder::with_program_primitives(&ProgramPrimitives {
            comment: "throw \"comments are not supported on this controller\";",
            ..Default::default()
        })
        .unwrap();
        let error = coder.render("program_begin", "`A\ncomment(\"x\");", &mut Scope::new());
        match error {
            Err(err) => assert!(
                err.to_string().contains("not supported"),
                "the author's sentence must survive the engine boundary: {err}"
            ),
            Ok(out) => panic!("a throwing callable must not produce a program: {out:?}"),
        }
    }

    /// Recursion is impossible by construction: the callables are registered on the main
    /// engine only, so the sub-engine has never heard of them. This is what makes a
    /// `comment` template that calls `comment()` an error rather than a hang.
    #[test]
    fn a_callable_cannot_call_itself() {
        let coder = Coder::with_program_primitives(&ProgramPrimitives {
            comment: "comment(\"again\");",
            ..Default::default()
        })
        .unwrap();
        assert!(
            coder.render("t", "comment(\"x\");", &mut Scope::new()).is_err(),
            "must be function-not-found, not an infinite loop"
        );
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
        let vars = crate::gcode::primitive_vars::variables_for("program_begin");

        // A template using only declared variables previews cleanly.
        let ok = coder
            .preview("program_begin", "`(from {filename})\nmetric();\n`G0 Z{z_safe}", &vars)
            .expect("declared variables render");
        assert!(ok.contains("(from board.kicad_pcb)"), "string sample substituted");
        assert!(ok.contains("G0 Z10"), "length sample rendered as a bare number");

        // Referencing a variable this primitive does not declare fails — exactly
        // the `z_safe not found` class the editor is meant to catch early.
        let err = coder.preview("program_begin", "`G0 X{feedrate}", &vars);
        assert!(err.is_err(), "undeclared variable must fail preview");
    }

    /// `line_format` gets a GCode line as its `text` sample, not a comment caption, so a
    /// template that branches on the line previews the case it will actually meet. The
    /// editor is where these templates get written; previewing the wrong branch would
    /// send the author looking for a bug that isn't there.
    #[test]
    fn the_line_filter_preview_samples_a_program_line() {
        let coder = Coder::new();
        let vars = crate::gcode::primitive_vars::variables_for("line_format");

        let numbered = coder
            .preview("line_format", "`N{(index + 1) * 10} {text}", &vars)
            .expect("index and text are both declared");
        assert_eq!(
            numbered, "N10 G1 X10 Y5 F600\n",
            "the sample is a real program line, and the filter emits the whole of it"
        );

        // `comment`'s caption sample is untouched by the split.
        let caption = coder
            .preview("comment", "`( {text} )", &crate::gcode::primitive_vars::variables_for("comment"));
        assert!(caption.unwrap().contains("Section"));
    }

    /// The header's step list previews the way it generates: indexed, walked, counted.
    ///
    /// Two sample steps rather than one, so `{step_index + 1} of {steps.len()}` previews
    /// as "1 of 2" — an author checking a multi-step header against a one-step sample
    /// would see "1 of 1" and learn nothing about the thing they were writing.
    #[test]
    fn the_header_preview_indexes_and_walks_the_sample_steps() {
        let coder = Coder::new();
        let vars = crate::gcode::primitive_vars::variables_for("program_begin");

        let out = coder
            .preview(
                "program_begin",
                "metric();\n\
                 `(step {step_index + 1} of {steps.len()}: {steps[step_index].name})\n\
                 `(on {steps[step_index].cnc_name}, finish \
                 {steps[step_index].route_board.finishing})\n\
                 for s in steps {\n`(- {s.name})\n}",
                &vars,
            )
            .expect("the declared step list previews");

        assert!(out.contains("(step 1 of 2: Drill)"), "indexed by step_index:\n{out}");
        assert!(out.contains("(on Sample mill, finish 0.1)"), "resolved name + unit:\n{out}");
        assert!(out.contains("(- Drill)") && out.contains("(- Route outline)"), "walked:\n{out}");
    }

    /// The sample step must carry the same fields a real one does.
    ///
    /// This is the guard on the one place the preview is weaker than elsewhere: for a
    /// scalar the sample scope holds exactly the declared names, so an undeclared
    /// reference cannot preview. A step's fields are `machining.yaml`'s, and nothing but
    /// this test stops the hand-written sample drifting from them — at which point the
    /// editor would reject a header that generates perfectly well, or accept one that
    /// does not.
    #[test]
    fn sample_steps_match_the_schema() {
        const MACHINING_SCHEMA: &str = include_str!("../../schemas/machining.yaml");

        let schema: serde_yaml::Value =
            serde_yaml::from_str(MACHINING_SCHEMA).expect("machining.yaml parses");
        let declared: Vec<String> = schema
            .get("$defs")
            .and_then(|v| v.get("step"))
            .and_then(|v| v.get("properties"))
            .and_then(serde_yaml::Value::as_mapping)
            .expect("the step definition lists its properties")
            .keys()
            .filter_map(|key| Some(key.as_str()?.to_string()))
            .collect();

        let steps = sample_steps();
        let StepValue::Map(fields) = &steps[0] else { panic!("a step samples as an object") };
        let sampled: Vec<&str> = fields.iter().map(|(key, _)| key.as_str()).collect();

        for name in &declared {
            assert!(
                sampled.contains(&name.as_str()),
                "machining.yaml declares `{name}` on a step but the preview sample has no \
                 such field, so `{{steps[0].{name}}}` fails in the editor and renders in \
                 the program"
            );
        }
        // …and nothing beyond the schema except the three names resolved beside the ids.
        for name in sampled {
            assert!(
                declared.iter().any(|d| d == name)
                    || ["cnc_name", "fixture_name", "toolset_name"].contains(&name),
                "the sample carries `{name}`, which no real step has — a header written \
                 against it would preview clean and then fail to render"
            );
        }
    }
}



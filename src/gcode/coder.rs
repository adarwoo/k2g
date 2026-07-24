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

use std::cell::Cell;
use std::rc::Rc;

use gtl::{Gtl, GtlError, Scope};
use units::{Length, UserUnitSystem};

use crate::gcode::primitive_vars::{PrimitiveVar, VarType};

/// A `gtl` engine with the GCode dialect registered. Built once per generation run
/// (on the worker thread) and reused across the program's primitives; the active
/// unit mode is engine state that carries from `initialise` through later calls.
pub struct Coder {
    gtl: Gtl,
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

        Self { gtl }
    }

    /// Compiles and runs one primitive template against `scope`, returning its GCode.
    pub fn render(&self, name: &str, source: &str, scope: &mut Scope) -> Result<String, GtlError> {
        let template = self.gtl.compile(name, source)?;
        self.gtl.run(&template, scope)
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
            push_sample(&mut scope, var);
        }
        self.render(name, source, &mut scope)
    }
}

/// Pushes one representative sample value for `var` into `scope`, typed so the
/// registered/default `fmt` overloads render it. Feed/rpm/angle are previewed as
/// plain numbers for now (the Coder formats real `Length`; the other unit types
/// gain `fmt` registrations when generation begins using them).
fn push_sample(scope: &mut Scope, var: &PrimitiveVar) {
    match var.var_type {
        VarType::String => {
            scope.push(var.name.clone(), sample_string(&var.name));
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
fn sample_string(name: &str) -> String {
    match name {
        "pcb_filename" => "board.kicad_pcb",
        "timestamp" => "2026-01-01 12:00:00",
        "arc_cmd" => "G2",
        "manual_message" => "(change tool)",
        "message" => "Paused",
        "text" => "Section",
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
}

//! RHAI-backed GCode template renderer.
//!
//! This library renders text templates where each `{...}` segment is evaluated as
//! a Rhai expression and replaced with the resulting value.
//!
//! It is designed for CNC/GCode generation scenarios where templates mix plain
//! GCode text with scriptable expressions and conditionals.
//!
//! # Example
//! ```rust
//! use gcode_rhai_lab::{GcodeTemplateParser, Length, TemplateContext};
//!
//! let parser = GcodeTemplateParser::new();
//! let ctx = TemplateContext {
//!     pcb_filename: "pulsegen.kicad_pcb".to_string(),
//!     has_positioning_pins: true,
//!     z_safe_height: Length::from_mm(-40.0),
//!     ..TemplateContext::default()
//! };
//!
//! let template = "{use_metric()}G21\nG0 Z{z_safe_height}\n{if has_positioning_pins {\"G56\"} else {\"G54\"}}";
//! let out = parser.render(template, &ctx).unwrap();
//!
//! assert!(out.contains("G0 Z-40"));
//! assert!(out.contains("G56"));
//! ```

use chrono::{DateTime, Local};
use rhai::{Dynamic, Engine, EvalAltResult, ImmutableString, Map, Scope};
use thiserror::Error;
use std::sync::{Arc, RwLock};

pub use units::{Angle, FeedRate, Length, RotationalSpeed};

const CONTROL_USE_METRIC: &str = "__k2g_control_use_metric__";
const CONTROL_USE_IMPERIAL: &str = "__k2g_control_use_imperial__";

/// Precision floor below which a converted value is treated as a whole number.
const UNIT_MAP_EPS: f64 = 1e-12;

/// Exposes a [`Length`] to Rhai as a map keyed by unit (`mm`, `inches`, …).
///
/// This lives here rather than in the `units` crate so that crate stays free of
/// any scripting-engine dependency: the map is rebuilt purely from the length's
/// public accessors.
fn length_to_rhai_map(length: &Length) -> Map {
    let mut map = Map::new();
    insert_unit_number(&mut map, "nm", length.as_nm());
    insert_unit_number(&mut map, "um", length.as_um());
    insert_unit_number(&mut map, "mm", length.as_mm());
    insert_unit_number(&mut map, "cm", length.as_cm());
    insert_unit_number(&mut map, "mil", length.as_mil());
    insert_unit_number(&mut map, "inches", length.as_inch());
    insert_unit_number(&mut map, "inch", length.as_inch());
    map
}

/// Inserts a converted unit value, preferring an integer when it is whole.
fn insert_unit_number(map: &mut Map, key: &str, value: f64) {
    let rounded = round_significant(value, 14);
    if rounded.fract().abs() < UNIT_MAP_EPS {
        map.insert(key.into(), Dynamic::from_int(rounded.round() as i64));
    } else {
        map.insert(key.into(), Dynamic::from_float(rounded));
    }
}

/// Rounds `value` to `digits` significant figures (used for tidy Rhai output).
fn round_significant(value: f64, digits: usize) -> f64 {
    if value == 0.0 {
        return 0.0;
    }

    let scale = 10f64.powi(digits as i32 - 1 - value.abs().log10().floor() as i32);
    (value * scale).round() / scale
}

/// Rendering mode for unit system conversions.
/// See `use_metric()` in templates to switch to metric mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RenderUnitMode {
    Raw,
    Metric,
    Imperial,
}

/// Input data made available to Rhai expressions during rendering.
///
/// The renderer preloads these fields into Rhai scope so templates can directly
/// reference them (for example: `pcb_filename`, `has_positioning_pins`, and
/// `z_safe_height.mm`).
#[derive(Debug, Clone)]
pub struct TemplateContext {
    /// Source PCB file name.
    pub pcb_filename: String,
    /// Whether positioning pins are present.
    pub has_positioning_pins: bool,
    /// Safe Z height as a typed length.
    pub z_safe_height: Length,
    /// Additional arbitrary values exposed as `ctx` in Rhai scope.
    pub extras: Map,
}

impl Default for TemplateContext {
    fn default() -> Self {
        Self {
            pcb_filename: "board.kicad_pcb".to_string(),
            has_positioning_pins: false,
            z_safe_height: Length::from_mm(-40.0),
            extras: Map::new(),
        }
    }
}

impl TemplateContext {
    /// Backward-friendly accessor for integrations that still need raw nm.
    pub fn z_safe_height_nm(&self) -> i64 {
        self.z_safe_height.as_nm().round() as i64
    }

    fn to_scope(&self) -> Scope<'static> {
        let mut scope = Scope::new();
        scope.push("pcb_filename", self.pcb_filename.clone());
        scope.push("has_positioning_pins", self.has_positioning_pins);
        scope.push("z_safe_height", length_to_rhai_map(&self.z_safe_height));
        scope.push("ctx", self.extras.clone());
        scope
    }
}

/// Rendering errors produced by parsing or Rhai evaluation.
#[derive(Debug, Error)]
pub enum RenderError {
    /// Template syntax/parsing failure (for example, unclosed `{`).
    #[error("template parse error: {0}")]
    Parse(String),
    /// Rhai expression evaluation failure.
    #[error("rhai evaluation error: {0}")]
    Rhai(#[from] Box<EvalAltResult>),
}

#[derive(Debug, Clone)]
struct RhaiDateTime(DateTime<Local>);

/// Template renderer that evaluates `{...}` blocks using Rhai.
///
/// The parser supports nested braces inside expressions and registers helper
/// functions including `now()`, `format(...)`, `clamp(...)`, and `mm_to_nm(...)`.
pub struct GcodeTemplateParser {
    engine: Engine,
    unit_mode: Arc<RwLock<RenderUnitMode>>,
}

impl Default for GcodeTemplateParser {
    fn default() -> Self {
        Self::new()
    }
}

impl GcodeTemplateParser {
    /// Creates a new parser with pre-registered helper functions and types.
    pub fn new() -> Self {
        let mut engine = Engine::new();
        let unit_mode = Arc::new(RwLock::new(RenderUnitMode::Raw));

        engine.register_type_with_name::<RhaiDateTime>("DateTime");
        engine.register_fn("now", || RhaiDateTime(Local::now()));
        engine.register_fn("format", |dt: &mut RhaiDateTime, fmt: &str| -> String {
            dt.0.format(fmt).to_string()
        });

        engine.register_fn("clamp", |value: f64, min: f64, max: f64| -> f64 {
            value.max(min).min(max)
        });

        {
            let mode = Arc::clone(&unit_mode);
            engine.register_fn("use_metric", move || {
                *mode.write().expect("unit mode lock poisoned") = RenderUnitMode::Metric;
            });
        }
        {
            let mode = Arc::clone(&unit_mode);
            engine.register_fn("use_imperial", move || {
                *mode.write().expect("unit mode lock poisoned") = RenderUnitMode::Imperial;
            });
        }

        engine.register_fn("unit", |v: f64| -> f64 { v });
        engine.register_fn("unit", |v: i64| -> f64 { v as f64 });

        {
            let mode = Arc::clone(&unit_mode);
            engine.register_fn("unit", move |map: Map| -> f64 {
                let current = *mode.read().expect("unit mode lock poisoned");
                unit_from_map_for_mode(&map, current)
                    .unwrap_or_else(|| panic!("unit(...) expects a numeric scalar or a unit map"))
            });
        }

        Self { engine, unit_mode }
    }

    /// Renders a template by replacing each `{...}` expression with its Rhai result.
    ///
    /// Literal text is preserved as-is. Expression outputs are stringified and
    /// concatenated in order.
    pub fn render(&self, template: &str, context: &TemplateContext) -> Result<String, RenderError> {
        {
            let mut mode = self.unit_mode.write().expect("unit mode lock poisoned");
            *mode = RenderUnitMode::Raw;
        }

        let segments = parse_segments(template)?;
        let mut scope = context.to_scope();
        let mut out = String::with_capacity(template.len() + 64);

        for segment in segments {
            match segment {
                Segment::Literal(text) => out.push_str(&text),
                Segment::Expr(expr) => {
                    let value = self.engine.eval_with_scope::<Dynamic>(&mut scope, &expr)?;
                    let current_mode = *self.unit_mode.read().expect("unit mode lock poisoned");
                    out.push_str(&dynamic_to_string(value, current_mode));
                }
            }
        }

        Ok(out)
    }
}

fn render_length_map_for_mode(map: &Map, mode: &RenderUnitMode) -> Option<String> {
    let key = match mode {
        RenderUnitMode::Metric => "mm",
        RenderUnitMode::Imperial => "inches",
        RenderUnitMode::Raw => return None,
    };

    let value = map.get(key)?;
    if value.is::<i64>() {
        return Some(value.clone_cast::<i64>().to_string());
    }
    if value.is::<f64>() {
        return Some(value.clone_cast::<f64>().to_string());
    }
    if value.is::<rhai::FLOAT>() {
        return Some(value.clone_cast::<rhai::FLOAT>().to_string());
    }
    Some(value.to_string())
}

fn dynamic_to_string(value: Dynamic, mode: RenderUnitMode) -> String {
    if value.is::<()>() {
        return String::new();
    } else if value.is::<ImmutableString>() {
        return value.cast::<ImmutableString>().to_string();
    }

    if value.is::<Map>() {
        let map = value.cast::<Map>();
        if let Some(number) = render_length_map_for_mode(&map, &mode) {
            return number;
        }
        return Dynamic::from(map).to_string();
    }

    value.to_string()
}

fn dynamic_to_f64(v: &Dynamic) -> Option<f64> {
    if v.is::<rhai::FLOAT>() {
        Some(v.clone_cast::<rhai::FLOAT>())
    } else if v.is::<f64>() {
        Some(v.clone_cast::<f64>())
    } else if v.is::<i64>() {
        Some(v.clone_cast::<i64>() as f64)
    } else {
        None
    }
}

fn unit_from_map_for_mode(map: &Map, mode: RenderUnitMode) -> Option<f64> {
    let primary = match mode {
        RenderUnitMode::Imperial => "inches",
        RenderUnitMode::Metric | RenderUnitMode::Raw => "mm",
    };

    map.get(primary)
        .and_then(dynamic_to_f64)
        .or_else(|| map.get("mm").and_then(dynamic_to_f64))
        .or_else(|| map.get("inches").and_then(dynamic_to_f64))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Literal(String),
    Expr(String),
}

fn parse_segments(template: &str) -> Result<Vec<Segment>, RenderError> {
    let chars: Vec<(usize, char)> = template.char_indices().collect();
    let mut segments = Vec::new();
    let mut i = 0usize;
    let mut literal_start = 0usize;

    while i < chars.len() {
        let (open_byte, ch) = chars[i];
        if ch != '{' {
            i += 1;
            continue;
        }

        if literal_start < open_byte {
            segments.push(Segment::Literal(template[literal_start..open_byte].to_string()));
        }

        let expr_start = open_byte + 1;
        i += 1;
        let mut depth = 1i32;
        let mut in_string: Option<char> = None;
        let mut escaped = false;

        while i < chars.len() {
            let (byte_idx, c) = chars[i];

            if let Some(quote) = in_string {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == quote {
                    in_string = None;
                }
                i += 1;
                continue;
            }

            if c == '"' || c == '\'' {
                in_string = Some(c);
                i += 1;
                continue;
            }

            if c == '{' {
                depth += 1;
                i += 1;
                continue;
            }

            if c == '}' {
                depth -= 1;
                if depth == 0 {
                    let expr_end = byte_idx;
                    let expr = template[expr_start..expr_end].trim().to_string();
                    segments.push(Segment::Expr(expr));
                    i += 1;
                    literal_start = if i < chars.len() {
                        chars[i].0
                    } else {
                        template.len()
                    };
                    break;
                }
                i += 1;
                continue;
            }

            i += 1;
        }

        if depth != 0 {
            return Err(RenderError::Parse(format!(
                "unclosed '{{' starting at byte {}",
                open_byte
            )));
        }
    }

    if literal_start < template.len() {
        segments.push(Segment::Literal(template[literal_start..].to_string()));
    }

    Ok(segments)
}

#[cfg(test)]
mod tests {
    use units::FeedRate;

use super::{length_to_rhai_map, GcodeTemplateParser, Length, RenderError, TemplateContext};

    #[test]
    fn preserves_literal_text_exactly() {
        let parser = GcodeTemplateParser::new();
        let template = "(Header)\nG0 X0 Y0\nM30\n";

        let rendered = parser
            .render(template, &TemplateContext::default())
            .expect("literal-only template should render");

        assert_eq!(rendered, template);
    }

    #[test]
    fn renders_literal_and_expression_segments_together() {
        let parser = GcodeTemplateParser::new();
        let context = TemplateContext {
            has_positioning_pins: true,
            ..TemplateContext::default()
        };

        let rendered = parser
            .render("N10 G0\n{if has_positioning_pins {\"G56\"} else {\"G54\"}}\nM30", &context)
            .expect("mixed literal/expression template should render");

        assert_eq!(rendered, "N10 G0\nG56\nM30");
    }

    const SPEC_TEMPLATE: &str = r#"(Created by k2g from '{pcb_filename}' - {now().format("%Y-%m-%d %H:%M:%S")})
(Reset all back to safe defaults)
G17 G54 G40 G49 G80 G90
{use_metric()}G21
G10 P0
(Establish the Z-Safe)
G0 Z{z_safe_height}
{if has_positioning_pins {"G56"} else {"G54"}}
"#;

    const SPEC_TEMPLATE2: &str = r#"  {let out = "";
   let current_z = unit(z_retract);
   let target_z = unit(z_bottom);
   let peck_depth = unit(peck);

   out += "G0 X" + unit(x).to_string() + " Y" + unit(y).to_string() + "\n";
   out += "G0 Z" + current_z.to_string() + "\n";

   while current_z > target_z {
     let next_z = (current_z - peck_depth).max(target_z);
     out += "G1 Z" + next_z.to_string() + " F" + unit(z_feedrate).to_string() + "\n";
     out += "G0 Z" + unit(z_retract).to_string() + "\n";
     current_z = next_z;
   }
   out}
"#;

    #[test]
    fn renders_spec_template_with_prefilled_context() {
        let parser = GcodeTemplateParser::new();
        let context = TemplateContext {
            pcb_filename: "pulsegen.kicad_pcb".to_string(),
            has_positioning_pins: true,
            z_safe_height: Length::from_mm(-40.0),
            ..TemplateContext::default()
        };

        let rendered = parser
            .render(SPEC_TEMPLATE, &context)
            .expect("template should render");

        assert!(rendered.contains("(Created by k2g from 'pulsegen.kicad_pcb' - "));
        assert!(rendered.contains("G0 Z-40"));
        assert!(rendered.lines().any(|line| line.trim() == "G56"));
    }

    #[test]
    fn renders_spec_template2_with_prefilled_context() {
        let parser = GcodeTemplateParser::new();
        let context = TemplateContext {
            z_retract: Length::from_mm(-5.0),
            z_bottom: Length::from_mm(0.0),
            peck: Length::from_mm(0.5),
            z_feedrate: FeedRate::from_mm_per_min(400),
            z_safe_height: Length::from_mm(-40.0),
            ..TemplateContext::default()
        };

        let rendered = parser
            .render(SPEC_TEMPLATE2, &context)
            .expect("template should render");
    }



    #[test]
    fn supports_nested_braces_inside_rhai_expression() {
        let parser = GcodeTemplateParser::new();
        let rendered = parser
            .render(
                "{if (1 < 2) { \"A\" } else { \"B\" }}",
                &TemplateContext::default(),
            )
            .expect("template should render");

        assert_eq!(rendered, "A");
    }

    #[test]
    fn renders_false_branch_for_positioning_pins() {
        let parser = GcodeTemplateParser::new();
        let context = TemplateContext {
            has_positioning_pins: false,
            ..TemplateContext::default()
        };

        let rendered = parser
            .render(
                "{if has_positioning_pins {\"G56\"} else {\"G54\"}}",
                &context,
            )
            .expect("template should render");

        assert_eq!(rendered, "G54");
    }

    #[test]
    fn use_metric_makes_plain_length_expression_render_mm() {
        let parser = GcodeTemplateParser::new();

        let context = TemplateContext {
            z_safe_height: Length::from_mm(-40.0),
            ..TemplateContext::default()
        };

        let rendered = parser
            .render("{use_metric()}{z_safe_height}", &context)
            .expect("template should render");

        assert_eq!(rendered, "-40");
    }

    #[test]
    fn use_imperial_makes_plain_length_expression_render_inches() {
        let parser = GcodeTemplateParser::new();

        let context = TemplateContext {
            z_safe_height: Length::from_mm(-25.4),
            ..TemplateContext::default()
        };

        let rendered = parser
            .render("{use_imperial()}{z_safe_height}", &context)
            .expect("template should render");

        assert_eq!(rendered, "-1");
    }

    #[test]
    fn length_unit_mode_switches_within_single_template() {
        let parser = GcodeTemplateParser::new();

        let context = TemplateContext {
            z_safe_height: Length::from_mm(-25.4),
            ..TemplateContext::default()
        };

        let rendered = parser
            .render("{use_metric()}{z_safe_height}|{use_imperial()}{z_safe_height}|{use_metric()}{z_safe_height}", &context)
            .expect("template should render");

        assert_eq!(rendered, "-25.4|-1|-25.4");
    }

    #[test]
    fn explicit_length_member_access_is_stable_across_mode_switches() {
        let parser = GcodeTemplateParser::new();

        let context = TemplateContext {
            z_safe_height: Length::from_mm(-25.4),
            ..TemplateContext::default()
        };

        let rendered = parser
            .render("{use_metric()}{z_safe_height.mm}|{use_imperial()}{z_safe_height.mm}", &context)
            .expect("template should render");

        assert_eq!(rendered, "-25.4|-25.4");
    }

    #[test]
    fn returns_parse_error_for_unclosed_expression() {
        let parser = GcodeTemplateParser::new();
        let error = parser
            .render("G0 Z{z_safe_height.mm", &TemplateContext::default())
            .expect_err("parser should reject malformed template");

        match error {
            RenderError::Parse(message) => assert!(message.contains("unclosed")),
            _ => panic!("expected parse error"),
        }
    }

    #[test]
    fn reports_parse_error_byte_offset_for_unclosed_expression() {
        let parser = GcodeTemplateParser::new();
        let malformed = "AB{if true {\"X\"}";

        let error = parser
            .render(malformed, &TemplateContext::default())
            .expect_err("parser should reject malformed template");

        match error {
            RenderError::Parse(message) => {
                assert!(message.contains("unclosed '{' starting at byte 2"));
            }
            _ => panic!("expected parse error"),
        }
    }

    #[test]
    fn reports_rhai_eval_error_for_unknown_symbol() {
        let parser = GcodeTemplateParser::new();

        let error = parser
            .render("G0 Z{unknown_symbol}", &TemplateContext::default())
            .expect_err("renderer should bubble up Rhai evaluation errors");

        match error {
            RenderError::Rhai(eval_error) => {
                let message = eval_error.to_string();
                assert!(message.contains("unknown_symbol"));
            }
            _ => panic!("expected Rhai error"),
        }
    }
}

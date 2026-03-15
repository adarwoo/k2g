
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
//! let template = "G0 Z{z_safe_height.mm}\n{if has_positioning_pins {\"G56\"} else {\"G54\"}}";
//! let out = parser.render(template, &ctx).unwrap();
//!
//! assert!(out.contains("G0 Z-40"));
//! assert!(out.contains("G56"));
//! ```

use chrono::{DateTime, Local};
use rhai::{Dynamic, Engine, EvalAltResult, ImmutableString, Map, Scope};
use thiserror::Error;

pub mod units;

pub use units::{Angle, FeedRate, Length, RotationalSpeed};

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
        scope.push("z_safe_height", self.z_safe_height.to_rhai_map());
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

        engine.register_type_with_name::<RhaiDateTime>("DateTime");
        engine.register_fn("now", || RhaiDateTime(Local::now()));
        engine.register_fn("format", |dt: &mut RhaiDateTime, fmt: &str| -> String {
            dt.0.format(fmt).to_string()
        });

        engine.register_fn("clamp", |value: f64, min: f64, max: f64| -> f64 {
            value.max(min).min(max)
        });
        engine.register_fn("mm_to_nm", |mm: f64| -> i64 { (mm * 1_000_000.0).round() as i64 });

        Self { engine }
    }

    /// Renders a template by replacing each `{...}` expression with its Rhai result.
    ///
    /// Literal text is preserved as-is. Expression outputs are stringified and
    /// concatenated in order.
    pub fn render(&self, template: &str, context: &TemplateContext) -> Result<String, RenderError> {
        let segments = parse_segments(template)?;
        let mut scope = context.to_scope();
        let mut out = String::with_capacity(template.len() + 64);

        for segment in segments {
            match segment {
                Segment::Literal(text) => out.push_str(&text),
                Segment::Expr(expr) => {
                    let value = self
                        .engine
                        .eval_expression_with_scope::<Dynamic>(&mut scope, &expr)?;
                    out.push_str(&dynamic_to_string(value));
                }
            }
        }

        Ok(out)
    }
}

fn dynamic_to_string(value: Dynamic) -> String {
    if value.is::<()>() {
        String::new()
    } else if value.is::<ImmutableString>() {
        value.cast::<ImmutableString>().to_string()
    } else {
        value.to_string()
    }
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
    use super::{GcodeTemplateParser, Length, RenderError, TemplateContext};

    const SPEC_TEMPLATE: &str = r#"(Created by k2g from '{pcb_filename}' - {now().format("%Y-%m-%d %H:%M:%S")})
(Reset all back to safe defaults)
G17 G54 G40 G49 G80 G90
G21
G10 P0
(Establish the Z-Safe)
G0 Z{z_safe_height.mm}
{if has_positioning_pins {"G56"} else {"G54"}}
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
}

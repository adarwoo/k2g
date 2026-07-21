//! GTL → Rhai transpiler.
//!
//! Each physical source line becomes exactly one line of transpiled Rhai, so line
//! numbering is preserved **1:1**: the "line map" is the identity, and Rhai's
//! reported error lines already point at the author's source (see [`crate::error`]).
//!
//! - A line whose first non-whitespace character is a backtick is an *emit line*;
//!   everything after the backtick is its payload, compiled to a single
//!   `emit(...)` statement with each `{ expr }` spliced in as `fmt(expr)`.
//! - Every other line is passed through to Rhai unchanged.

/// A segment of an emit-line payload.
enum Segment {
    /// Literal text, with `{{`/`}}` already unescaped to `{`/`}`.
    Text(String),
    /// A `{ expr }` interpolation, holding the raw Rhai expression.
    Expr(String),
}

/// Transpile GTL `source` into Rhai source. On an interpolation syntax error,
/// returns `(line, col, message)` positioned in the author's source (1-based).
pub(crate) fn transpile(source: &str) -> Result<String, (usize, usize, String)> {
    let mut out = String::new();
    for (idx, raw) in source.lines().enumerate() {
        let line_no = idx + 1;
        let trimmed = raw.trim_start();
        let leading = raw.len() - trimmed.len();
        if let Some(payload) = trimmed.strip_prefix('`') {
            // `off` is the char offset of the open brace within the payload; the
            // author column is that plus the dropped indentation and the backtick.
            let segments = scan_payload(payload)
                .map_err(|(off, message)| (line_no, leading + 1 + off + 1, message))?;
            out.push_str(&assemble(&segments));
        } else {
            out.push_str(raw);
        }
        out.push('\n');
    }
    Ok(out)
}

/// Scan an emit payload into literal/expression segments. On an unterminated
/// interpolation, returns `(char_offset_of_open_brace, message)`.
fn scan_payload(payload: &str) -> Result<Vec<Segment>, (usize, String)> {
    let chars: Vec<char> = payload.chars().collect();
    let mut segments = Vec::new();
    let mut text = String::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '{' if chars.get(i + 1) == Some(&'{') => {
                text.push('{');
                i += 2;
            }
            '}' if chars.get(i + 1) == Some(&'}') => {
                text.push('}');
                i += 2;
            }
            '{' => {
                if !text.is_empty() {
                    segments.push(Segment::Text(std::mem::take(&mut text)));
                }
                let (expr, next) = scan_expr(&chars, i + 1).map_err(|message| (i, message))?;
                segments.push(Segment::Expr(expr));
                i = next;
            }
            c => {
                // A lone `}` is treated as literal text: GCode never uses braces,
                // so forcing it to be doubled would be a needless nuisance.
                text.push(c);
                i += 1;
            }
        }
    }
    if !text.is_empty() {
        segments.push(Segment::Text(text));
    }
    Ok(segments)
}

/// Scan a Rhai expression inside `{ … }`, starting just after the opening brace.
/// Brace-depth and string-literal aware, so a `}` inside a nested block or a
/// string literal does not close the interpolation. Returns the trimmed
/// expression and the index just past the closing brace.
fn scan_expr(chars: &[char], start: usize) -> Result<(String, usize), String> {
    let mut expr = String::new();
    let mut depth = 0i32;
    let mut i = start;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '"' | '\'' => {
                // Consume the string/char literal verbatim, honouring `\` escapes,
                // so its contents never influence brace matching.
                expr.push(c);
                i += 1;
                while i < chars.len() {
                    let d = chars[i];
                    expr.push(d);
                    i += 1;
                    if d == '\\' {
                        if let Some(&escaped) = chars.get(i) {
                            expr.push(escaped);
                            i += 1;
                        }
                    } else if d == c {
                        break;
                    }
                }
            }
            '{' => {
                depth += 1;
                expr.push(c);
                i += 1;
            }
            '}' if depth == 0 => return Ok((expr.trim().to_string(), i + 1)),
            '}' => {
                depth -= 1;
                expr.push(c);
                i += 1;
            }
            _ => {
                expr.push(c);
                i += 1;
            }
        }
    }
    Err("unterminated `{` interpolation".to_string())
}

/// Assemble parsed segments into one `emit(...)` Rhai statement. An empty payload
/// (a bare backtick) becomes `emit("")`, i.e. one blank output line.
fn assemble(segments: &[Segment]) -> String {
    if segments.is_empty() {
        return "emit(\"\");".to_string();
    }
    let parts: Vec<String> = segments
        .iter()
        .map(|seg| match seg {
            Segment::Text(t) => rhai_string_literal(t),
            Segment::Expr(e) => format!("fmt({e})"),
        })
        .collect();
    format!("emit({});", parts.join(" + "))
}

/// Render `s` as a double-quoted Rhai string literal, escaping `\` and `"`.
fn rhai_string_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::transpile;

    #[test]
    fn emit_line_becomes_emit_call() {
        assert_eq!(transpile("`G0 X{x}").unwrap(), "emit(\"G0 X\" + fmt(x));\n");
    }

    #[test]
    fn only_interpolation_needs_no_literal() {
        assert_eq!(transpile("`{x}").unwrap(), "emit(fmt(x));\n");
    }

    #[test]
    fn bare_backtick_emits_an_empty_line() {
        assert_eq!(transpile("`").unwrap(), "emit(\"\");\n");
    }

    #[test]
    fn rhai_line_is_passed_through() {
        assert_eq!(transpile("let z = 1;").unwrap(), "let z = 1;\n");
    }

    #[test]
    fn indentation_before_backtick_is_dropped() {
        assert_eq!(transpile("    `A{b}").unwrap(), "emit(\"A\" + fmt(b));\n");
    }

    #[test]
    fn doubled_braces_are_literal() {
        assert_eq!(transpile("`{{x}}").unwrap(), "emit(\"{x}\");\n");
    }

    #[test]
    fn brace_inside_expression_string_does_not_close() {
        assert_eq!(transpile("`{ \"a}b\" }").unwrap(), "emit(fmt(\"a}b\"));\n");
    }

    #[test]
    fn unterminated_interpolation_reports_author_position() {
        let (line, col, message) = transpile("`Z{z").unwrap_err();
        assert_eq!((line, col), (1, 3));
        assert!(message.contains("interpolation"), "{message}");
    }
}

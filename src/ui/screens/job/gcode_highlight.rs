//! GCode syntax highlighting for the Job "Code" view.
//!
//! GCode is a small, regular language, so rather than pull in a full Sublime
//! grammar we drive the pure-Rust [`synoptic`] highlighter with a handful of
//! programmatic rules. Highlighting yields per-line [`Span`]s carrying a CSS
//! class, which the view renders as themed `<span>`s (colours come from the app
//! theme's variables, so light/dark both work).

use synoptic::{Highlighter, TokOpt};

/// A styled fragment of one GCode line: its text and the CSS class to apply
/// (empty string ⇒ unstyled plain text).
#[derive(Clone, PartialEq)]
pub struct Span {
    pub text: String,
    pub class: &'static str,
}

/// Builds a GCode highlighter (rules only). The regexes are tiny; rebuilding per
/// highlight pass costs microseconds and keeps the highlighter free of per-run
/// state leaking between calls.
///
/// Token start-characters are mutually exclusive across rules (G / M / N / T /
/// axis letters / `(` / `;`), so no two keyword rules contend for the same
/// position, and a bounded `( … )` comment swallows any codes inside it.
fn gcode_highlighter() -> Highlighter {
    let mut highlighter = Highlighter::new(4);

    // Comments first: parenthesised inline comments and `;`-to-end-of-line.
    // (`bounded` start/end are regexes, so the parentheses are escaped.)
    highlighter.bounded("comment", r"\(", r"\)", false);
    highlighter.keyword("comment", r";.*");

    // Preparatory/motion G-words and machine M-words (case-insensitive: some
    // post-processors emit lowercase).
    highlighter.keyword("gword", r"(?i)G\d+\.?\d*");
    highlighter.keyword("mword", r"(?i)M\d+");

    // Line numbers (N) and tool selects (T).
    highlighter.keyword("linenum", r"(?i)N\d+");
    highlighter.keyword("tool", r"(?i)T\d+");

    // Coordinate axes with their signed values …
    highlighter.keyword("axis", r"(?i)[XYZABCUVW]-?\d*\.?\d+");
    // … and feed/speed/cycle parameters with theirs.
    highlighter.keyword("param", r"(?i)[FSPQRIJK]-?\d*\.?\d+");

    highlighter
}

/// Maps a synoptic token name to its stable CSS class.
fn class_for(name: &str) -> &'static str {
    match name {
        "comment" => "gcode-comment",
        "gword" => "gcode-gword",
        "mword" => "gcode-mword",
        "linenum" => "gcode-linenum",
        "tool" => "gcode-tool",
        "axis" => "gcode-axis",
        "param" => "gcode-param",
        _ => "",
    }
}

/// Highlights a whole program into per-line span lists. A blank input yields a
/// single empty line so the caller always has at least one row to render.
pub fn highlight_program(source: &str) -> Vec<Vec<Span>> {
    let lines: Vec<String> = source.split('\n').map(str::to_string).collect();

    let mut highlighter = gcode_highlighter();
    highlighter.run(&lines);

    lines
        .iter()
        .enumerate()
        .map(|(y, line)| {
            highlighter
                .line(y, line)
                .into_iter()
                .map(|token| match token {
                    TokOpt::Some(text, name) => Span { text, class: class_for(&name) },
                    TokOpt::None(text) => Span { text, class: "" },
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classes_of<'a>(spans: &'a [Span], substr: &str) -> Vec<&'a str> {
        spans
            .iter()
            .filter(|s| s.text.contains(substr))
            .map(|s| s.class)
            .collect()
    }

    #[test]
    fn classifies_the_common_gcode_tokens() {
        let program = "N10 G1 X10.5 Y-3 F1200 M3 S24000\n(a comment G1 not highlighted)";
        let lines = highlight_program(program);
        assert_eq!(lines.len(), 2);

        // Reassembling spans must reproduce the original line exactly.
        let rebuilt: String = lines[0].iter().map(|s| s.text.as_str()).collect();
        assert_eq!(rebuilt, "N10 G1 X10.5 Y-3 F1200 M3 S24000");

        assert_eq!(classes_of(&lines[0], "N10"), vec!["gcode-linenum"]);
        assert_eq!(classes_of(&lines[0], "G1"), vec!["gcode-gword"]);
        assert_eq!(classes_of(&lines[0], "X10.5"), vec!["gcode-axis"]);
        assert_eq!(classes_of(&lines[0], "Y-3"), vec!["gcode-axis"]);
        assert_eq!(classes_of(&lines[0], "F1200"), vec!["gcode-param"]);
        assert_eq!(classes_of(&lines[0], "M3"), vec!["gcode-mword"]);
        assert_eq!(classes_of(&lines[0], "S24000"), vec!["gcode-param"]);

        // The parenthesised comment is one comment span; the `G1` inside it is
        // NOT separately highlighted.
        assert_eq!(lines[1].len(), 1);
        assert_eq!(lines[1][0].class, "gcode-comment");
        assert!(lines[1][0].text.contains("G1"));
    }

    #[test]
    fn preserves_blank_and_semicolon_comment_lines() {
        let lines = highlight_program("\nG0 Z5 ; retract");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].len(), 0, "a blank line has no spans");

        let comment: Vec<&str> = lines[1]
            .iter()
            .filter(|s| s.class == "gcode-comment")
            .map(|s| s.text.as_str())
            .collect();
        assert_eq!(comment, vec!["; retract"]);
    }
}

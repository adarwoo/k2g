//! `x-show-when` — schema-declared conditional visibility for generated forms.
//!
//! A field can declare that it only makes sense alongside a particular sibling value:
//!
//! ```yaml
//! vgroove_depth:
//!   x-show-when: { cut: [vgroove] }
//! retention:
//!   x-show-when: { cut: [route, mill] }
//! ```
//!
//! [`SchemaForm`](crate::ui::bindings::SchemaForm) consults this before rendering a
//! child, so a board being scored does not offer a tab count and a routed one does not
//! offer a V-groove depth.
//!
//! **Display only — never validation.** The obvious alternative is JSON Schema's
//! `if`/`then`, gating the field's *presence*. That does not work here: the loader
//! materialises every default, so a non-vgroove outline still carries a `vgroove_depth`
//! and an `if cut != vgroove then vgroove_depth absent` rule would reject it. There is a
//! comment in `machining.yaml` recording where that was discovered. Hiding the widget
//! costs validation nothing and keeps the stored document uniform.
//!
//! **Siblings only.** Every condition the schemas need is on a sibling key, and a
//! general pointer expression would be a query language nobody asked for. A condition
//! naming a key that is not a sibling simply never matches, so the field stays hidden —
//! visible in the UI as a missing field, which is the failure mode that gets noticed.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde_json::Value;

/// The schemas that may carry `x-show-when`. Adding one here is all it takes.
const SCHEMAS: &[&str] = &[include_str!("../../schemas/machining.yaml")];

/// One field's visibility condition: the sibling to look at, and the values of it that
/// make this field relevant.
#[derive(Clone, Debug, PartialEq)]
pub struct ShowWhen {
    pub sibling: String,
    pub values: Vec<Value>,
}

impl ShowWhen {
    /// Whether `sibling_value` — the current value of [`Self::sibling`] — makes the
    /// field visible. Compared loosely across the string/bool boundary, because a
    /// schema author writes `[true]` or `[vgroove]` and means either.
    pub fn matches(&self, sibling_value: Option<&Value>) -> bool {
        let Some(actual) = sibling_value else {
            // The sibling is absent, so nothing can satisfy the condition. Hiding is the
            // safer of the two answers: a field whose precondition is unknown is a field
            // the operator cannot answer meaningfully.
            return false;
        };
        self.values.iter().any(|want| loose_eq(want, actual))
    }
}

/// Equality across the shapes a YAML author might write. `vgroove` matches `"vgroove"`,
/// and `true` matches `"true"` — the alternative is a schema that silently never shows a
/// field because its condition was quoted.
fn loose_eq(want: &Value, actual: &Value) -> bool {
    if want == actual {
        return true;
    }
    let as_text = |v: &Value| match v {
        Value::String(s) => Some(s.clone()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    };
    match (as_text(want), as_text(actual)) {
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(&b),
        _ => false,
    }
}

/// The condition declared for the field at `pointer`, if any.
///
/// Keyed by the last **two** path segments rather than by the full pointer, so one
/// declaration inside a shared `$defs` block covers every place that block is used:
/// `retention/count` is written once and applies under both `outline` and `cutouts`.
/// Falls back to the bare field name, so a declaration at a unique name still resolves.
///
/// The parent matters because the same field name can be declared at two use sites with
/// different conditions — `outline/retention` depends on `cut`, `cutouts/retention` on
/// `enabled`.
pub fn show_when(pointer: &str) -> Option<&'static ShowWhen> {
    let mut segments = pointer.rsplit('/');
    let name = segments.next()?;
    if let Some(parent) = segments.next() {
        if let Some(found) = registry().get(&format!("{parent}/{name}")) {
            return Some(found);
        }
    }
    registry().get(name)
}

fn registry() -> &'static HashMap<String, ShowWhen> {
    static REGISTRY: OnceLock<HashMap<String, ShowWhen>> = OnceLock::new();
    REGISTRY.get_or_init(build_registry)
}

/// Walks every schema for `x-show-when` declarations. A malformed schema yields an empty
/// registry rather than a panic: the form then shows every field, which is the harmless
/// failure.
fn build_registry() -> HashMap<String, ShowWhen> {
    let mut out = HashMap::new();
    for source in SCHEMAS {
        let Some(schema) = serde_yaml::from_str::<serde_yaml::Value>(source)
            .ok()
            .and_then(|yaml| serde_json::to_value(yaml).ok())
        else {
            continue;
        };
        collect(&schema, None, None, &mut out);
    }
    out
}

/// Recursively finds `x-show-when` anywhere in the schema tree, recording each against
/// `"<parent>/<name>"` — the two segments [`show_when`] looks up.
///
/// Both `properties` and `$defs` name their children, which is what lets a condition
/// written once inside `$defs/retention` resolve for every use of that block.
fn collect(
    node: &Value,
    parent: Option<&str>,
    property: Option<&str>,
    out: &mut HashMap<String, ShowWhen>,
) {
    let Some(object) = node.as_object() else { return };

    if let (Some(name), Some(condition)) = (property, object.get("x-show-when")) {
        if let Some((sibling, values)) = condition.as_object().and_then(|c| c.iter().next()) {
            let values = match values {
                Value::Array(items) => items.clone(),
                single => vec![single.clone()],
            };
            let key = match parent {
                Some(parent) => format!("{parent}/{name}"),
                None => name.to_string(),
            };
            out.insert(key, ShowWhen { sibling: sibling.clone(), values });
        }
    }

    for (key, child) in object {
        if key == "properties" || key == "$defs" {
            if let Some(named) = child.as_object() {
                for (child_name, child_node) in named {
                    collect(child_node, property, Some(child_name), out);
                }
                continue;
            }
        }
        // Anything else (`default`, `items`, …) keeps the enclosing names.
        collect(child, parent, property, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The declarations the machining schema actually carries, resolved through real
    /// field pointers.
    #[test]
    fn the_machining_schema_declarations_are_found() {
        let base = "/steps/0/route_board";

        let vgroove = show_when(&format!("{base}/outline/vgroove_depth")).expect("declared");
        assert_eq!(vgroove.sibling, "cut");
        assert!(vgroove.matches(Some(&json!("vgroove"))));
        assert!(!vgroove.matches(Some(&json!("route"))), "hidden for a routed edge");

        // Shared `$defs` are keyed by `<block>/<field>`, so the one declaration inside
        // `$defs/retention` serves the block wherever it is used.
        let count = show_when(&format!("{base}/outline/retention/count")).expect("declared");
        assert_eq!(count.sibling, "mode");
        assert!(count.matches(Some(&json!("tabs"))));
        assert!(!count.matches(Some(&json!("none"))));
    }

    /// The key carries the parent as well as the field name, so a declaration answers
    /// only for the block it was written in.
    ///
    /// `retention` used to appear twice under `route_board` — once for the outline and
    /// once for the interior cutouts, each following a different sibling — which is what
    /// the parent-qualified key was for. The cutouts block has since moved out to
    /// `route_cutouts`, so the property is now asserted by the miss: the same field name
    /// under a block that does not declare it resolves to nothing rather than borrowing
    /// the outline's condition.
    #[test]
    fn a_shared_field_name_resolves_per_use_site() {
        let outline = show_when("/steps/0/route_board/outline/retention").expect("declared");
        assert_eq!(outline.sibling, "cut", "the outline's retention follows how it is cut");
        assert!(outline.matches(Some(&json!("route"))));
        assert!(!outline.matches(Some(&json!("score"))), "a scored board is not cut free");

        assert!(
            show_when("/steps/0/route_cutouts/retention").is_none(),
            "another block's field of the same name does not inherit the declaration"
        );
    }

    /// A field with no declaration is always shown — the overwhelmingly common case.
    #[test]
    fn an_undeclared_field_has_no_condition() {
        assert!(show_when("/steps/0/route_board/outline/cut").is_none());
        assert!(show_when("/steps/0/does_not_exist").is_none());
    }

    /// An absent sibling hides the field rather than showing it: a precondition that
    /// cannot be read is not a precondition that is satisfied.
    #[test]
    fn an_absent_sibling_hides_the_field() {
        let condition = ShowWhen { sibling: "cut".into(), values: vec![json!("vgroove")] };
        assert!(!condition.matches(None));
    }

    /// Booleans survive the YAML/JSON round trip whether the author quoted them or not.
    #[test]
    fn conditions_compare_loosely_across_quoting() {
        let condition = ShowWhen { sibling: "enabled".into(), values: vec![json!(true)] };
        assert!(condition.matches(Some(&json!(true))));
        assert!(condition.matches(Some(&json!("true"))), "a quoted true still matches");
        assert!(!condition.matches(Some(&json!(false))));
    }
}

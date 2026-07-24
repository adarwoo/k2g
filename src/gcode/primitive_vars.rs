//! The per-primitive GTL variable registry.
//!
//! Each CNC primitive template runs against a scope of variables specific to that
//! primitive (coordinates, feed rates, messages, …) on top of the shared
//! emit/`fmt()`/`metric()` surface. Those variables are **documented in the schema**
//! (`schemas/cnc.yaml`, each primitive's `x-variables`) so there is a single source
//! of truth; this module reads that metadata so the primitive editor can show the
//! reference panel and the validator/preview can build a matching sample scope.
//!
//! The intent is that the real generation scopes are built to match this list —
//! keeping the documented contract and what generation actually provides in step.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde_json::Value;

/// The embedded CNC schema — the authority on each primitive's variables.
const CNC_SCHEMA: &str = include_str!("../../schemas/cnc.yaml");

/// The value kind of a primitive variable — drives the reference label and the
/// sample value the preview substitutes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VarType {
    String,
    Length,
    Feed,
    Rpm,
    Angle,
    Integer,
    Number,
}

impl VarType {
    fn parse(raw: &str) -> Self {
        match raw {
            "length" => Self::Length,
            "feed" => Self::Feed,
            "rpm" => Self::Rpm,
            "angle" => Self::Angle,
            "integer" => Self::Integer,
            "number" => Self::Number,
            _ => Self::String,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Length => "length",
            Self::Feed => "feed",
            Self::Rpm => "rpm",
            Self::Angle => "angle",
            Self::Integer => "integer",
            Self::Number => "number",
        }
    }
}

/// One variable available to a primitive's GTL.
#[derive(Clone)]
pub struct PrimitiveVar {
    pub name: String,
    pub var_type: VarType,
    pub description: String,
}

/// The declared variables for `primitive` (empty for a primitive that takes none,
/// or an unknown name).
pub fn variables_for(primitive: &str) -> Vec<PrimitiveVar> {
    registry().get(primitive).cloned().unwrap_or_default()
}

/// The last path segment of a `/primitives/<name>` JSON pointer, or `None` if the
/// pointer is not a primitive field. Convenience for the editor, which is handed a
/// field pointer.
pub fn primitive_name_from_pointer(pointer: &str) -> Option<&str> {
    pointer.strip_prefix("/primitives/").filter(|rest| !rest.contains('/'))
}

fn registry() -> &'static HashMap<String, Vec<PrimitiveVar>> {
    static REGISTRY: OnceLock<HashMap<String, Vec<PrimitiveVar>>> = OnceLock::new();
    REGISTRY.get_or_init(build_registry)
}

/// Parses `x-variables` out of every primitive definition in the CNC schema. A
/// malformed schema yields an empty registry rather than a panic — the editor then
/// simply shows no reference, never crashes.
fn build_registry() -> HashMap<String, Vec<PrimitiveVar>> {
    let mut out = HashMap::new();

    let Some(schema) = serde_yaml::from_str::<serde_yaml::Value>(CNC_SCHEMA)
        .ok()
        .and_then(|yaml| serde_json::to_value(yaml).ok())
    else {
        return out;
    };

    let Some(primitives) = schema
        .pointer("/properties/primitives/properties")
        .and_then(Value::as_object)
    else {
        return out;
    };

    for (name, def) in primitives {
        let vars = def
            .get("x-variables")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| {
                        let name = entry.get("name")?.as_str()?.to_string();
                        let var_type = VarType::parse(
                            entry.get("type").and_then(Value::as_str).unwrap_or("string"),
                        );
                        let description = entry
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        Some(PrimitiveVar { name, var_type, description })
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.insert(name.clone(), vars);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_documented_variables_for_a_primitive() {
        let vars = variables_for("initialise");
        let names: Vec<&str> = vars.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names, vec!["pcb_filename", "timestamp", "z_safe"]);
        assert_eq!(vars[2].var_type, VarType::Length);
        assert!(!vars[2].description.is_empty(), "descriptions are carried through");
    }

    #[test]
    fn a_primitive_with_no_variables_yields_an_empty_list() {
        assert!(variables_for("conclude").is_empty());
        assert!(variables_for("stop_spindle").is_empty());
        // Unknown primitives are empty, not a panic.
        assert!(variables_for("does_not_exist").is_empty());
    }

    #[test]
    fn drill_variables_carry_their_types() {
        let vars = variables_for("drill");
        let feed = vars.iter().find(|v| v.name == "z_feedrate").expect("z_feedrate present");
        assert_eq!(feed.var_type, VarType::Feed);
    }

    #[test]
    fn primitive_name_is_extracted_from_a_field_pointer() {
        assert_eq!(primitive_name_from_pointer("/primitives/drill"), Some("drill"));
        assert_eq!(primitive_name_from_pointer("/machine/scaling"), None);
        assert_eq!(primitive_name_from_pointer("/primitives/drill/extra"), None);
    }
}

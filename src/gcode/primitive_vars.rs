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
    Boolean,
    Length,
    Feed,
    Rpm,
    Angle,
    Integer,
    Number,
    /// An indexable list of objects — today only `steps`. Unlike the scalars, its
    /// *shape* is described in the variable's own description rather than by its type,
    /// because the shape is `machining.yaml`'s and belongs where that is documented.
    List,
}

impl VarType {
    fn parse(raw: &str) -> Self {
        match raw {
            "boolean" => Self::Boolean,
            "length" => Self::Length,
            "feed" => Self::Feed,
            "rpm" => Self::Rpm,
            "angle" => Self::Angle,
            "integer" => Self::Integer,
            "number" => Self::Number,
            "list" => Self::List,
            _ => Self::String,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Boolean => "boolean",
            Self::Length => "length",
            Self::Feed => "feed",
            Self::Rpm => "rpm",
            Self::Angle => "angle",
            Self::Integer => "integer",
            Self::Number => "number",
            Self::List => "list",
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

/// **How** a primitive is invoked — the schema's `x-kind`.
///
/// This is the fact the CNC editor could not previously tell an author, and the one that
/// bites: a filled-in `set_origin` does nothing at all unless `program_begin` calls it, and
/// nothing on screen said so.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PrimitiveKind {
    /// The application emits it, at a defined point in the program.
    Generator,
    /// Nothing emits it — a template calls it by name (`set_origin();`, `comment("…")`).
    Callable,
    /// Applied to every line of the finished program.
    Filter,
}

impl PrimitiveKind {
    fn parse(raw: &str) -> Self {
        match raw {
            "callable" => Self::Callable,
            "filter" => Self::Filter,
            _ => Self::Generator,
        }
    }

    /// The badge shown beside the field.
    pub fn label(self) -> &'static str {
        match self {
            Self::Generator => "Generator",
            Self::Callable => "Callable",
            Self::Filter => "Filter",
        }
    }

    /// One line saying what that badge means, in terms of what the author must do.
    pub fn hint(self) -> &'static str {
        match self {
            Self::Generator => "Emitted automatically, at a fixed point in the program.",
            Self::Callable => {
                "Nothing emits this on its own — call it by name from another template."
            }
            Self::Filter => "Applied to every line of the finished program.",
        }
    }
}

/// Where a primitive belongs in the editor — the schema's `x-category`.
///
/// Independent of [`PrimitiveKind`]: `Operator` holds three callables and `Program` holds
/// generators and callables together, which is simply what is true of them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PrimitiveCategory {
    Program,
    Tools,
    Motion,
    Drilling,
    Operator,
    Formatting,
}

impl PrimitiveCategory {
    /// Display order in the editor: roughly the order a program is built.
    pub const ORDER: [PrimitiveCategory; 6] = [
        Self::Program,
        Self::Tools,
        Self::Motion,
        Self::Drilling,
        Self::Operator,
        Self::Formatting,
    ];

    fn parse(raw: &str) -> Self {
        match raw {
            "tools" => Self::Tools,
            "motion" => Self::Motion,
            "drilling" => Self::Drilling,
            "operator" => Self::Operator,
            "formatting" => Self::Formatting,
            _ => Self::Program,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Program => "Program",
            Self::Tools => "Tools and spindle",
            Self::Motion => "Motion",
            Self::Drilling => "Drilling",
            Self::Operator => "Operator",
            Self::Formatting => "Output formatting",
        }
    }
}

/// One primitive as the schema declares it.
#[derive(Clone)]
pub struct Primitive {
    pub name: String,
    pub kind: PrimitiveKind,
    pub category: PrimitiveCategory,
    pub vars: Vec<PrimitiveVar>,
    /// What a **blank** template for this primitive falls back to — the schema's
    /// `x-fallback`. `None` for the great majority, where blank means "this machine has
    /// no word for it" and nothing is emitted.
    pub fallback: Option<String>,
    /// Position in the schema's own declaration order, so the editor lists a category's
    /// primitives the way the schema reads rather than alphabetically (`program_begin`
    /// before `program_end`, `tool_change` before `tool_measure`).
    order: usize,
}

/// The declared variables for `primitive` (empty for a primitive that takes none,
/// or an unknown name).
pub fn variables_for(primitive: &str) -> Vec<PrimitiveVar> {
    registry().get(primitive).map(|p| p.vars.clone()).unwrap_or_default()
}

/// How `primitive` is invoked, or `None` for a name the schema does not declare.
pub fn kind_of(primitive: &str) -> Option<PrimitiveKind> {
    registry().get(primitive).map(|p| p.kind)
}

/// What a blank `primitive` falls back to, or `None` if blank simply means "emits
/// nothing" — which is every primitive but the two motion ones.
///
/// The distinction is the schema's to make, not the renderer's: a blank `tool_measure`
/// means *this machine needs no measurement block* and must keep meaning that, while a
/// blank `cut_arc` means *this machine has no arc word, cut it as straight moves*. Only a
/// primitive that declares `x-fallback` is degraded.
pub fn fallback_for(primitive: &str) -> Option<String> {
    registry().get(primitive).and_then(|p| p.fallback.clone())
}

/// The schema's primitives in `category`, in declaration order.
///
/// The editor builds its groups from this rather than from a hardcoded list, so adding a
/// primitive to the schema is enough to make it appear — the old list had to be edited in
/// two places and silently omitted whatever was forgotten.
pub fn primitives_in(category: PrimitiveCategory) -> Vec<Primitive> {
    let mut found: Vec<Primitive> = registry()
        .values()
        .filter(|p| p.category == category)
        .cloned()
        .collect();
    found.sort_by_key(|p| p.order);
    found
}

/// The last path segment of a `/primitives/<name>` JSON pointer, or `None` if the
/// pointer is not a primitive field. Convenience for the editor, which is handed a
/// field pointer.
pub fn primitive_name_from_pointer(pointer: &str) -> Option<&str> {
    pointer.strip_prefix("/primitives/").filter(|rest| !rest.contains('/'))
}

/// The primitives a CNC profile must define, from the schema's own `required` list.
///
/// Read from the schema rather than restated in Rust because a restatement drifted, and
/// did so silently in the worst possible direction: the readiness gate's copy still named
/// the pre-rename primitives (`initialise`, `change_tool`, …) after `PRIMITIVE_RENAMES`
/// migrated every profile to the new ones. Every CNC profile in existence therefore
/// reported seven missing required fields, so the gate refused to generate for *any* job
/// and said only "Referenced CNC profile is incomplete" — a complaint no operator could
/// act on, about fields that were all present.
///
/// An unparsable schema yields an empty list, matching [`registry`]: refusing to name any
/// required primitive fails open, which is right for a check whose whole purpose is to
/// explain itself. Failing closed would block generation with no reason to show.
pub fn required_primitives() -> &'static [String] {
    static REQUIRED: OnceLock<Vec<String>> = OnceLock::new();
    REQUIRED.get_or_init(|| {
        let Ok(schema) = serde_yaml::from_str::<serde_yaml::Value>(CNC_SCHEMA) else {
            return Vec::new();
        };
        schema
            .get("properties")
            .and_then(|v| v.get("primitives"))
            .and_then(|v| v.get("required"))
            .and_then(serde_yaml::Value::as_sequence)
            .map(|names| {
                names.iter().filter_map(|n| n.as_str().map(str::to_string)).collect()
            })
            .unwrap_or_default()
    })
}

fn registry() -> &'static HashMap<String, Primitive> {
    static REGISTRY: OnceLock<HashMap<String, Primitive>> = OnceLock::new();
    REGISTRY.get_or_init(build_registry)
}

/// Parses `x-kind`, `x-category` and `x-variables` out of every primitive definition in the
/// CNC schema. A malformed schema yields an empty registry rather than a panic — the editor
/// then simply shows no reference, never crashes.
fn build_registry() -> HashMap<String, Primitive> {
    let mut out = HashMap::new();

    let Some(yaml) = serde_yaml::from_str::<serde_yaml::Value>(CNC_SCHEMA).ok() else {
        return out;
    };
    // Declaration order, taken from the YAML because `serde_json::Map` is a `BTreeMap`
    // here (no `preserve_order` feature) and would sort the names alphabetically. The
    // schema lists each category in the order a program is built — `tool_change` before
    // `spindle_start`, `program_begin` before `program_end` — which is far more use to an
    // author than the alphabet.
    let declaration_order: HashMap<String, usize> = yaml
        .get("properties")
        .and_then(|v| v.get("primitives"))
        .and_then(|v| v.get("properties"))
        .and_then(serde_yaml::Value::as_mapping)
        .map(|map| {
            map.keys()
                .enumerate()
                .filter_map(|(i, k)| Some((k.as_str()?.to_string(), i)))
                .collect()
        })
        .unwrap_or_default();

    let Some(schema) = serde_json::to_value(yaml).ok() else {
        return out;
    };

    let Some(primitives) = schema
        .pointer("/properties/primitives/properties")
        .and_then(Value::as_object)
    else {
        return out;
    };

    for (name, def) in primitives {
        let order = declaration_order.get(name).copied().unwrap_or(usize::MAX);
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
        let kind =
            PrimitiveKind::parse(def.get("x-kind").and_then(Value::as_str).unwrap_or_default());
        let category = PrimitiveCategory::parse(
            def.get("x-category").and_then(Value::as_str).unwrap_or_default(),
        );
        let fallback = def
            .get("x-fallback")
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|target| !target.trim().is_empty());
        out.insert(
            name.clone(),
            Primitive { name: name.clone(), kind, category, vars, fallback, order },
        );
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_documented_variables_for_a_primitive() {
        let vars = variables_for("program_begin");
        let names: Vec<&str> = vars.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["filename", "timestamp", "z_safe", "origin_reference", "steps", "step_index"]
        );
        assert_eq!(vars[2].var_type, VarType::Length);
        assert_eq!(
            vars[3].var_type,
            VarType::String,
            "the origin reference is the machine's own word for an offset, not a number"
        );
        assert_eq!(
            vars[4].var_type,
            VarType::List,
            "`steps` is indexed, not printed — a `string` here would have the preview \
             push text where generation pushes an array, and only the preview would agree"
        );
        assert_eq!(vars[5].var_type, VarType::Integer);
        assert!(!vars[2].description.is_empty(), "descriptions are carried through");
    }

    #[test]
    fn a_primitive_with_no_variables_yields_an_empty_list() {
        assert!(variables_for("spindle_stop").is_empty());
        // Unknown primitives are empty, not a panic.
        assert!(variables_for("does_not_exist").is_empty());
    }

    #[test]
    fn program_end_declares_the_program_layer_variables() {
        // The footer retracts to `z_safe` (and may echo the file/timestamp), so it
        // shares `program_begin`'s program-layer scope. Generation must provide these.
        let vars = variables_for("program_end");
        let names: Vec<&str> = vars.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["filename", "timestamp", "z_safe", "origin_reference", "steps", "step_index"]
        );
    }

    /// Every primitive declares how it is invoked and where it belongs. A missing `x-kind`
    /// silently defaults to Generator, which would tell an author that a callable is
    /// emitted automatically — the exact trap the taxonomy exists to close.
    #[test]
    fn every_primitive_declares_its_kind_and_category() {
        let listed: Vec<String> = PrimitiveCategory::ORDER
            .iter()
            .flat_map(|c| primitives_in(*c))
            .map(|p| p.name)
            .collect();
        assert_eq!(listed.len(), registry().len(), "every primitive lands in exactly one group");

        for (name, expected) in [
            ("program_begin", PrimitiveKind::Generator),
            ("program_end", PrimitiveKind::Generator),
            ("set_unit", PrimitiveKind::Callable),
            ("set_origin", PrimitiveKind::Callable),
            ("comment", PrimitiveKind::Callable),
            ("message", PrimitiveKind::Callable),
            ("pause", PrimitiveKind::Callable),
            ("line_format", PrimitiveKind::Filter),
            ("drill", PrimitiveKind::Generator),
            ("tool_measure", PrimitiveKind::Generator),
        ] {
            assert_eq!(kind_of(name), Some(expected), "{name}");
        }
    }

    /// A category lists its primitives in the order the schema declares them, which is the
    /// order a program is built — not alphabetically, which would put `spindle_start`
    /// before the `tool_change` that must precede it.
    #[test]
    fn a_category_lists_its_primitives_in_program_order() {
        let names: Vec<String> =
            primitives_in(PrimitiveCategory::Tools).into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["tool_change", "tool_measure", "spindle_start", "spindle_stop"]);

        let program: Vec<String> =
            primitives_in(PrimitiveCategory::Program).into_iter().map(|p| p.name).collect();
        assert_eq!(program, ["program_begin", "program_end", "set_unit", "set_origin"]);
    }

    /// `drill` carries its place in the block, which is what lets a profile emit a modal
    /// cycle (open on the first hole, cancel on the last) instead of a full cycle per hole.
    #[test]
    fn drill_knows_its_place_in_the_block() {
        let vars = variables_for("drill");
        for name in ["index", "count"] {
            let var = vars.iter().find(|v| v.name == name).expect("declared");
            assert_eq!(var.var_type, VarType::Integer, "{name} counts holes");
        }
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

    /// The schema's declared fallback must be the one the renderer implements.
    ///
    /// `x-fallback` is only a promise until something checks it: `degrade_moves` names the
    /// templates directly rather than walking a table, so the schema and the renderer could
    /// drift with nothing to notice. This pins them together, and pins the shape of the
    /// chain — it must terminate, and it must terminate on the one move every machine can
    /// make.
    #[test]
    fn the_declared_fallback_chain_is_the_one_the_renderer_walks() {
        assert_eq!(fallback_for("cut_arc").as_deref(), Some("cut_linear"));
        assert_eq!(
            fallback_for("cut_linear"),
            None,
            "cut_linear is the floor: a machine that cannot cut a straight line has              nothing left to fall back to, and the render must fail rather than quietly              emit a program with the cuts missing"
        );

        // Blankness is not a general rule. A blank `tool_measure` means "this machine
        // needs no measurement block" and must keep meaning exactly that.
        for untouched in ["tool_measure", "set_origin", "comment", "line_format", "drill"] {
            assert_eq!(
                fallback_for(untouched),
                None,
                "`{untouched}` must not be degraded when blank"
            );
        }

        // Walking the chain from the top must reach the floor without looping.
        let mut seen = vec!["cut_arc".to_string()];
        while let Some(next) = fallback_for(seen.last().expect("non-empty")) {
            assert!(!seen.contains(&next), "the fallback chain loops at `{next}`");
            assert!(seen.len() < 8, "the fallback chain does not terminate: {seen:?}");
            seen.push(next);
        }
        assert_eq!(seen, ["cut_arc", "cut_linear"]);
    }
}

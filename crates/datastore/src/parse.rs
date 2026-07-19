//! Parsing: validate a data file, apply schema defaults, and decode it into the
//! annotated [`Document`] tree.
//!
//! The flow for one file is: parse YAML → run JSON Schema validation (collecting
//! *all* errors, non-fatally) → walk the schema and data together, classifying
//! each field, decoding units/ids/refs, and filling in defaults for absent
//! fields. References are recorded but left [`RefState::Unresolved`] until
//! [`crate::DataStore::resolve`].

use std::path::PathBuf;

use serde_json::Value;
use uuid::Uuid;

use crate::error::{DataError, DataErrorKind, Reason};
use crate::model::{
    Constraints, Document, FieldKind, Meta, Node, NodeValue, Reference, RefState, Status,
};
use crate::schema::{ref_target, yaml_to_json, RefTarget, SchemaSet};
use crate::units_bridge::{decode_unit, UnitKind};

/// Shared per-file context threaded through the walk.
struct Ctx<'a> {
    schema_id: &'a str,
    source: &'a Option<PathBuf>,
    errors: &'a mut Vec<DataError>,
}

impl Ctx<'_> {
    fn push(&mut self, pointer: &str, kind: DataErrorKind, message: impl Into<String>) {
        self.errors
            .push(DataError::new(self.schema_id, self.source, pointer, kind, message));
    }
}

/// Parses one data file into a [`Document`], appending any problems to `errors`.
/// Returns `None` only when the file cannot be parsed as YAML or names an
/// unknown schema (nothing meaningful to annotate).
pub(crate) fn parse_document(
    set: &SchemaSet,
    schema_id: &str,
    source: Option<PathBuf>,
    text: &str,
    errors: &mut Vec<DataError>,
) -> Option<Document> {
    let mut data = match yaml_to_json(text) {
        Ok(value) => value,
        Err(message) => {
            errors.push(DataError::new(schema_id, &source, "", DataErrorKind::Yaml, message));
            return None;
        }
    };

    let root_schema = match set.root(schema_id) {
        Some(root) => root,
        None => {
            errors.push(DataError::new(
                schema_id,
                &source,
                "",
                DataErrorKind::Validation,
                format!("unknown schema '{schema_id}'"),
            ));
            return None;
        }
    };

    // `$schema` is a reserved meta-key: strip it before validation (schemas do
    // not declare it and many are `additionalProperties: false`). It is
    // re-stamped on write.
    if let Value::Object(map) = &mut data {
        map.remove("$schema");
    }

    // Version gating — only when the schema declares `x-schema-version`.
    if let Some(current) = root_schema.get("x-schema-version").and_then(Value::as_i64) {
        match data.get("schema_version").and_then(Value::as_i64) {
            None => {
                errors.push(DataError::new(
                    schema_id,
                    &source,
                    "/schema_version",
                    DataErrorKind::SchemaVersion,
                    format!("missing schema_version (schema is version {current})"),
                ));
                return None;
            }
            Some(v) if v > current => {
                errors.push(DataError::new(
                    schema_id,
                    &source,
                    "/schema_version",
                    DataErrorKind::SchemaVersion,
                    format!("file schema_version {v} is newer than supported {current}; rejected"),
                ));
                return None;
            }
            Some(v) if v < current => {
                errors.push(DataError::new(
                    schema_id,
                    &source,
                    "/schema_version",
                    DataErrorKind::SchemaVersion,
                    format!("file schema_version {v} is older than {current}; upgrade not yet supported"),
                ));
                return None;
            }
            Some(_) => {}
        }
    }

    if let Some(validator) = set.validator(schema_id) {
        for error in validator.iter_errors(&data) {
            errors.push(DataError::new(
                schema_id,
                &source,
                error.instance_path.to_string(),
                DataErrorKind::Validation,
                error.to_string(),
            ));
        }
    }

    let (root, mut annotate_errors) = annotate(set, schema_id, &source, schema_id, root_schema, &data);
    errors.append(&mut annotate_errors);

    let status = aggregate_status(&root);
    Some(Document {
        schema_id: schema_id.to_string(),
        source,
        root,
        status,
    })
}

/// Walks a schema node and data value together into an annotated [`Node`],
/// returning any decode/missing-required problems. Shared by parsing and the
/// factory (which passes a synthesized default value).
pub(crate) fn annotate(
    set: &SchemaSet,
    schema_id: &str,
    source: &Option<PathBuf>,
    base_id: &str,
    schema_node: &Value,
    data: &Value,
) -> (Node, Vec<DataError>) {
    let mut errors = Vec::new();
    let mut ctx = Ctx {
        schema_id,
        source,
        errors: &mut errors,
    };
    let root = build_node(set, base_id, schema_node, "", Some(data), false, "", &mut ctx);
    drop(ctx);
    (root, errors)
}

/// The classified kind of a field plus, for structured kinds, the resolved
/// schema node (and its base id) to recurse into.
struct Classification<'a> {
    kind: FieldKind,
    structural: Option<Structural<'a>>,
}

struct Structural<'a> {
    node: &'a Value,
    base: String,
}

/// Classifies a property schema, following `anyOf`/`$ref` as needed.
fn classify<'a>(set: &'a SchemaSet, base_id: &str, node: &'a Value) -> Classification<'a> {
    // An explicit x-ref wins over everything (even a sibling `$ref` to uuid_v7).
    if let Some(target) = node.get("x-ref").and_then(Value::as_str) {
        return Classification {
            kind: FieldKind::Ref {
                target: Some(target.to_string()),
            },
            structural: None,
        };
    }

    let eff = pick_effective(node);

    if let Some(ref_str) = eff.get("$ref").and_then(Value::as_str) {
        match ref_target(ref_str) {
            RefTarget::Units(name) => {
                if let Some(kind) = UnitKind::from_def_name(name) {
                    return Classification {
                        kind: FieldKind::Unit(kind),
                        structural: None,
                    };
                }
            }
            RefTarget::Id => {
                return Classification {
                    kind: FieldKind::Id,
                    structural: None,
                };
            }
            RefTarget::Structural => {
                if let Some(resolved) = set.resolve_ref(base_id, ref_str) {
                    return classify(set, &resolved.base, resolved.value);
                }
            }
        }
    }

    if let Some(Value::Array(values)) = eff.get("enum") {
        let variants = values
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
        return Classification {
            kind: FieldKind::Enum(variants),
            structural: Some(Structural {
                node: eff,
                base: base_id.to_string(),
            }),
        };
    }

    let kind = match eff.get("type").and_then(Value::as_str) {
        Some("object") => FieldKind::Object,
        Some("array") => FieldKind::Array,
        _ => FieldKind::Scalar,
    };
    Classification {
        kind,
        structural: Some(Structural {
            node: eff,
            base: base_id.to_string(),
        }),
    }
}

/// Unwraps a single level of `anyOf`/`oneOf` to the first non-`null` branch.
fn pick_effective(node: &Value) -> &Value {
    for key in ["anyOf", "oneOf"] {
        if let Some(Value::Array(branches)) = node.get(key) {
            if let Some(branch) = branches.iter().find(|b| !is_null_type(b)) {
                return branch;
            }
        }
    }
    node
}

fn is_null_type(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("null")
}

/// Builds one annotated node from a property schema and (optional) data value.
fn build_node(
    set: &SchemaSet,
    base_id: &str,
    schema_node: &Value,
    name: &str,
    data: Option<&Value>,
    required: bool,
    pointer: &str,
    ctx: &mut Ctx,
) -> Node {
    let cls = classify(set, base_id, schema_node);
    let constraint_src = cls.structural.as_ref().map(|s| s.node).unwrap_or(schema_node);

    let meta = Meta {
        name: name.to_string(),
        title: string_field(schema_node, "title"),
        description: string_field(schema_node, "description"),
        required,
        kind: cls.kind.clone(),
        default_applied: false,
        constraints: constraints_from(constraint_src),
    };

    let Some(value) = data else {
        return Node::new(NodeValue::Null, meta);
    };

    let (node_value, status) = match &cls.kind {
        FieldKind::Object => build_object(set, &cls, value, pointer, ctx),
        FieldKind::Array => (build_array(set, &cls, value, pointer, ctx), Status::Complete),
        FieldKind::Unit(kind) => (build_unit(*kind, value, pointer, ctx), Status::Complete),
        FieldKind::Id => (build_id(value, pointer, ctx), Status::Complete),
        FieldKind::Ref { target } => (build_ref(target.clone(), value, pointer, ctx), Status::Complete),
        FieldKind::Enum(_) | FieldKind::Scalar => (scalar_value(value), Status::Complete),
    };

    Node {
        value: node_value,
        meta,
        status,
    }
}

/// Builds an object node, recursing into schema `properties`, honoring
/// `required`, filling defaults for absent fields, and preserving any extra
/// data keys for lossless round-tripping.
fn build_object(
    set: &SchemaSet,
    cls: &Classification,
    data: &Value,
    pointer: &str,
    ctx: &mut Ctx,
) -> (NodeValue, Status) {
    let structural = cls.structural.as_ref().expect("object has structural node");
    let base = structural.base.as_str();
    let schema_node = structural.node;

    let data_obj = data.as_object();
    let required_set = required_names(schema_node);
    let mut map = indexmap::IndexMap::new();
    let mut status = Status::Complete;

    if let Some(Value::Object(properties)) = schema_node.get("properties") {
        for (prop_name, prop_schema) in properties {
            let child_pointer = join_pointer(pointer, prop_name);
            let is_required = required_set.iter().any(|r| r == prop_name);
            let child_data = data_obj.and_then(|o| o.get(prop_name));

            if let Some(value) = child_data {
                let child = build_node(
                    set,
                    base,
                    prop_schema,
                    prop_name,
                    Some(value),
                    is_required,
                    &child_pointer,
                    ctx,
                );
                map.insert(prop_name.clone(), child);
            } else if let Some(default) = compute_default(set, base, prop_schema) {
                let mut child = build_node(
                    set,
                    base,
                    prop_schema,
                    prop_name,
                    Some(&default),
                    is_required,
                    &child_pointer,
                    ctx,
                );
                child.meta.default_applied = true;
                map.insert(prop_name.clone(), child);
            } else if is_required {
                status.add_reason(Reason::MissingRequired(child_pointer.clone()));
                ctx.push(
                    &child_pointer,
                    DataErrorKind::MissingRequired,
                    format!("required property '{prop_name}' is missing"),
                );
            }
        }
    }

    // Preserve data keys the schema does not describe (additionalProperties).
    if let Some(obj) = data_obj {
        for (key, value) in obj {
            if !map.contains_key(key) {
                let child_pointer = join_pointer(pointer, key);
                map.insert(key.clone(), generic_node(key, value, &child_pointer));
            }
        }
    }

    (NodeValue::Object(map), status)
}

/// Builds an array node from the schema's `items`.
fn build_array(
    set: &SchemaSet,
    cls: &Classification,
    data: &Value,
    pointer: &str,
    ctx: &mut Ctx,
) -> NodeValue {
    let structural = cls.structural.as_ref().expect("array has structural node");
    let base = structural.base.as_str();
    let items_schema = structural.node.get("items");

    let mut items = Vec::new();
    if let Value::Array(values) = data {
        for (idx, value) in values.iter().enumerate() {
            let child_pointer = format!("{pointer}/{idx}");
            let child = match items_schema {
                Some(schema) => build_node(set, base, schema, "", Some(value), false, &child_pointer, ctx),
                None => generic_node("", value, &child_pointer),
            };
            items.push(child);
        }
    }
    NodeValue::Array(items)
}

fn build_unit(kind: UnitKind, data: &Value, pointer: &str, ctx: &mut Ctx) -> NodeValue {
    match decode_unit(kind, data) {
        Ok(Some(unit)) => NodeValue::Unit(unit),
        Ok(None) => scalar_value(data),
        Err(message) => {
            ctx.push(pointer, DataErrorKind::Unit, message);
            scalar_value(data)
        }
    }
}

fn build_id(data: &Value, pointer: &str, ctx: &mut Ctx) -> NodeValue {
    match data.as_str().and_then(|s| Uuid::parse_str(s).ok()) {
        Some(id) => NodeValue::Id(id),
        None => {
            ctx.push(pointer, DataErrorKind::Id, "expected a UUID string");
            scalar_value(data)
        }
    }
}

fn build_ref(target: Option<String>, data: &Value, pointer: &str, ctx: &mut Ctx) -> NodeValue {
    match data.as_str().and_then(|s| Uuid::parse_str(s).ok()) {
        Some(raw) => NodeValue::Ref(Reference {
            raw,
            target,
            state: RefState::Unresolved,
        }),
        None => {
            ctx.push(pointer, DataErrorKind::Reference, "expected a UUID reference string");
            scalar_value(data)
        }
    }
}

/// Maps a JSON scalar to a [`NodeValue`] (arrays/objects fall back to generic).
fn scalar_value(value: &Value) -> NodeValue {
    match value {
        Value::Null => NodeValue::Null,
        Value::Bool(b) => NodeValue::Bool(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                NodeValue::Int(i)
            } else {
                NodeValue::Float(n.as_f64().unwrap_or(0.0))
            }
        }
        Value::String(s) => NodeValue::Str(s.clone()),
        Value::Array(items) => NodeValue::Array(
            items
                .iter()
                .map(|v| generic_node("", v, ""))
                .collect(),
        ),
        Value::Object(_) => generic_node("", value, "").value,
    }
}

/// Builds an unschema'd node straight from JSON (for extra keys / fallbacks).
fn generic_node(name: &str, value: &Value, pointer: &str) -> Node {
    let (node_value, kind) = match value {
        Value::Object(obj) => {
            let mut map = indexmap::IndexMap::new();
            for (key, child) in obj {
                let child_pointer = join_pointer(pointer, key);
                map.insert(key.clone(), generic_node(key, child, &child_pointer));
            }
            (NodeValue::Object(map), FieldKind::Object)
        }
        Value::Array(items) => {
            let nodes = items
                .iter()
                .enumerate()
                .map(|(idx, child)| generic_node("", child, &format!("{pointer}/{idx}")))
                .collect();
            (NodeValue::Array(nodes), FieldKind::Array)
        }
        _ => (scalar_value(value), FieldKind::Scalar),
    };
    Node::new(node_value, Meta::bare(name, kind))
}

/// Computes the schema default for an (absent) property, following `$ref` and
/// `anyOf`, and synthesizing an object only when it has defaulted children.
fn compute_default(set: &SchemaSet, base_id: &str, node: &Value) -> Option<Value> {
    if let Some(default) = node.get("default") {
        return Some(default.clone());
    }
    if let Some(constant) = node.get("const") {
        return Some(constant.clone());
    }

    let eff = pick_effective(node);
    if !std::ptr::eq(eff, node) {
        if let Some(default) = eff.get("default") {
            return Some(default.clone());
        }
        if let Some(constant) = eff.get("const") {
            return Some(constant.clone());
        }
    }

    if let Some(ref_str) = eff.get("$ref").and_then(Value::as_str) {
        match ref_target(ref_str) {
            RefTarget::Units(_) | RefTarget::Id => return None,
            RefTarget::Structural => {
                let resolved = set.resolve_ref(base_id, ref_str)?;
                return compute_default(set, &resolved.base, resolved.value);
            }
        }
    }

    if eff.get("type").and_then(Value::as_str) == Some("object") {
        if let Some(Value::Object(properties)) = eff.get("properties") {
            let mut obj = serde_json::Map::new();
            for (key, prop_schema) in properties {
                if let Some(default) = compute_default(set, base_id, prop_schema) {
                    obj.insert(key.clone(), default);
                }
            }
            if !obj.is_empty() {
                return Some(Value::Object(obj));
            }
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Factory: build fresh instances from a schema
// ---------------------------------------------------------------------------

/// Builds a fresh instance of `schema_id`'s root: schema defaults applied,
/// identity fields assigned new UUIDv7s, references and required-without-default
/// fields left absent (so the node is `Incomplete` until filled by the caller).
pub(crate) fn instantiate(set: &SchemaSet, schema_id: &str) -> Option<Node> {
    let root_schema = set.root(schema_id)?;
    let value = factory_value(set, schema_id, root_schema)?;
    Some(annotate(set, schema_id, &None, schema_id, root_schema, &value).0)
}

/// Builds a fresh item for the array at `array_pointer` within `schema_id`.
pub(crate) fn instantiate_item(
    set: &SchemaSet,
    schema_id: &str,
    array_pointer: &str,
) -> Option<Node> {
    let (array_schema, base) = schema_node_at(set, schema_id, array_pointer)?;
    let items = pick_effective(array_schema).get("items")?;
    let value = factory_value(set, &base, items)?;
    Some(annotate(set, schema_id, &None, &base, items, &value).0)
}

/// Resolves the schema node governing a data JSON Pointer, following
/// `properties`/`items` and crossing `$ref`s (array index tokens select `items`).
pub(crate) fn schema_node_at<'a>(
    set: &'a SchemaSet,
    schema_id: &str,
    pointer: &str,
) -> Option<(&'a Value, String)> {
    let mut base = schema_id.to_string();
    let mut node = set.root(schema_id)?;

    for token in pointer.split('/').filter(|s| !s.is_empty()) {
        let cls = classify(set, &base, node);
        let structural = cls.structural.as_ref()?;
        match &cls.kind {
            FieldKind::Object => {
                let props = structural.node.get("properties").and_then(Value::as_object)?;
                let next = props.get(token)?;
                base = structural.base.clone();
                node = next;
            }
            FieldKind::Array => {
                let items = structural.node.get("items")?;
                base = structural.base.clone();
                node = items;
            }
            _ => return None,
        }
    }
    Some((node, base))
}

/// Synthesizes a default JSON value for a schema node: defaults/consts applied,
/// identity fields freshly generated, objects recursed, references omitted.
/// Returns `None` when there is nothing to emit (an absent optional field).
fn factory_value(set: &SchemaSet, base_id: &str, schema_node: &Value) -> Option<Value> {
    let cls = classify(set, base_id, schema_node);
    match &cls.kind {
        FieldKind::Ref { .. } => None,
        FieldKind::Id => Some(Value::String(Uuid::now_v7().to_string())),
        FieldKind::Object => {
            let structural = cls.structural.as_ref()?;
            let base = structural.base.as_str();
            let mut map = serde_json::Map::new();
            if let Some(properties) = structural.node.get("properties").and_then(Value::as_object) {
                for (name, prop_schema) in properties {
                    if let Some(value) = factory_value(set, base, prop_schema) {
                        map.insert(name.clone(), value);
                    }
                }
            }
            Some(Value::Object(map))
        }
        FieldKind::Array => {
            compute_default(set, base_id, schema_node).or_else(|| Some(Value::Array(Vec::new())))
        }
        FieldKind::Unit(_) | FieldKind::Enum(_) | FieldKind::Scalar => {
            compute_default(set, base_id, schema_node)
        }
    }
}

/// Extracts UI constraints from a schema node.
fn constraints_from(node: &Value) -> Constraints {
    Constraints {
        const_value: node.get("const").cloned(),
        enum_values: node
            .get("enum")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        minimum: node.get("minimum").and_then(Value::as_f64),
        maximum: node.get("maximum").and_then(Value::as_f64),
        min_length: node.get("minLength").and_then(Value::as_u64),
        max_length: node.get("maxLength").and_then(Value::as_u64),
        pattern: string_field(node, "pattern"),
    }
}

fn required_names(schema_node: &Value) -> Vec<String> {
    schema_node
        .get("required")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

fn string_field(node: &Value, key: &str) -> Option<String> {
    node.get(key).and_then(Value::as_str).map(String::from)
}

/// Appends a (pointer-escaped) key to a JSON Pointer.
fn join_pointer(pointer: &str, key: &str) -> String {
    let escaped = key.replace('~', "~0").replace('/', "~1");
    format!("{pointer}/{escaped}")
}

/// Aggregates a subtree's statuses into a document-level status.
pub(crate) fn aggregate_status(root: &Node) -> Status {
    let mut reasons = Vec::new();
    collect_reasons(root, &mut reasons);
    if reasons.is_empty() {
        Status::Complete
    } else {
        Status::Incomplete(reasons)
    }
}

fn collect_reasons(node: &Node, out: &mut Vec<Reason>) {
    if let Status::Incomplete(reasons) = &node.status {
        for reason in reasons {
            if !out.contains(reason) {
                out.push(reason.clone());
            }
        }
    }
    match &node.value {
        NodeValue::Object(map) => {
            for child in map.values() {
                collect_reasons(child, out);
            }
        }
        NodeValue::Array(items) => {
            for child in items {
                collect_reasons(child, out);
            }
        }
        _ => {}
    }
}

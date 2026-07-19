//! The annotated document model.
//!
//! Parsing produces a tree of [`Node`]s that mirrors the data, but where every
//! node also carries the schema-derived [`Meta`] a UI needs to render it — its
//! title, description, whether it is required, its kind, and any constraints.
//! This is the "generic data type, enriched" the crate is built around: the
//! *value* is a serde-like union ([`NodeValue`]), and the *shape/rules* come from
//! the schema.
//!
//! Unit-bearing fields are decoded into typed [`UnitValue`]s (via the `units`
//! crate); identity fields become [`NodeValue::Id`]; reference fields become
//! [`NodeValue::Ref`] and stay unresolved until [`crate::DataStore::resolve`].

use std::path::PathBuf;

use indexmap::IndexMap;
use serde_json::{Number, Value};
use uuid::Uuid;

use crate::error::Reason;
use crate::units_bridge::UnitKind;

/// A registered schema's identity (its `$id`, e.g. `"cnc.yaml"`).
pub type SchemaId = String;

/// One parsed data file as an annotated tree.
#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    /// Root schema this document was parsed against.
    pub schema_id: SchemaId,
    /// Originating file, when the caller supplied one.
    pub source: Option<PathBuf>,
    /// The root node (typically an object).
    pub root: Node,
    /// Aggregate completeness, set during [`crate::DataStore::resolve`].
    pub status: Status,
}

impl Document {
    /// Re-emits the document as a plain `serde_json::Value`, losslessly, so the
    /// caller can serialize it back to YAML/JSON and save it.
    pub fn to_value(&self) -> Value {
        self.root.to_value()
    }
}

/// A single value in the document, plus its schema metadata and status.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// The decoded value.
    pub value: NodeValue,
    /// Schema-derived metadata describing this node.
    pub meta: Meta,
    /// Per-node completeness (missing-required / unresolved-ref).
    pub status: Status,
}

impl Node {
    pub(crate) fn new(value: NodeValue, meta: Meta) -> Self {
        Self {
            value,
            meta,
            status: Status::Complete,
        }
    }

    /// Navigates to a descendant by JSON Pointer (e.g. `/tools/0/ref`).
    pub fn get_pointer(&self, pointer: &str) -> Option<&Node> {
        let mut current = self;
        for raw in pointer.split('/').filter(|s| !s.is_empty()) {
            let token = unescape_pointer_token(raw);
            current = match &current.value {
                NodeValue::Object(map) => map.get(token.as_ref())?,
                NodeValue::Array(items) => items.get(token.parse::<usize>().ok()?)?,
                _ => return None,
            };
        }
        Some(current)
    }

    /// Mutable variant of [`Node::get_pointer`], for editing a field in place.
    pub fn get_pointer_mut(&mut self, pointer: &str) -> Option<&mut Node> {
        let mut current = self;
        for raw in pointer.split('/').filter(|s| !s.is_empty()) {
            let token = unescape_pointer_token(raw);
            current = match &mut current.value {
                NodeValue::Object(map) => map.get_mut(token.as_ref())?,
                NodeValue::Array(items) => items.get_mut(token.parse::<usize>().ok()?)?,
                _ => return None,
            };
        }
        Some(current)
    }

    /// The identity (`id`) of this object node, if it has one.
    pub fn identity(&self) -> Option<Uuid> {
        if let NodeValue::Object(map) = &self.value {
            for child in map.values() {
                if let NodeValue::Id(id) = &child.value {
                    return Some(*id);
                }
            }
        }
        None
    }

    /// Deep-clones this subtree, assigning every identity (`id`) a fresh UUIDv7.
    /// References that pointed *inside* the subtree are remapped to the new ids
    /// (and reset to unresolved); references to outside objects are preserved.
    pub fn clone_with_new_ids(&self) -> Node {
        let mut remap: std::collections::HashMap<Uuid, Uuid> = std::collections::HashMap::new();
        collect_identities(self, &mut remap);
        clone_remapped(self, &remap)
    }

    /// Re-emits this node's value as a plain `serde_json::Value`.
    pub fn to_value(&self) -> Value {
        match &self.value {
            NodeValue::Null => Value::Null,
            NodeValue::Bool(b) => Value::Bool(*b),
            NodeValue::Int(i) => Value::Number((*i).into()),
            NodeValue::Float(f) => Number::from_f64(*f).map(Value::Number).unwrap_or(Value::Null),
            NodeValue::Str(s) => Value::String(s.clone()),
            NodeValue::Unit(u) => Value::String(u.to_source_string()),
            NodeValue::Id(id) => Value::String(id.to_string()),
            NodeValue::Ref(r) => Value::String(r.raw.to_string()),
            NodeValue::Array(items) => Value::Array(items.iter().map(Node::to_value).collect()),
            NodeValue::Object(map) => {
                let mut out = serde_json::Map::new();
                for (key, child) in map {
                    out.insert(key.clone(), child.to_value());
                }
                Value::Object(out)
            }
        }
    }
}

/// The value carried by a [`Node`].
#[derive(Debug, Clone, PartialEq)]
pub enum NodeValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    /// A unit-bearing value decoded via the `units` crate.
    Unit(UnitValue),
    /// This object's own identity.
    Id(Uuid),
    /// A reference to another object (stays unresolved until `resolve`).
    Ref(Reference),
    Array(Vec<Node>),
    /// Object fields, kept in schema-declared order (drives UI ordering).
    Object(IndexMap<String, Node>),
}

/// A typed unit value. Only the four kinds the `units` crate models appear here;
/// other unit kinds keep their raw scalar form (see [`UnitKind::is_typed`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnitValue {
    Length(units::Length),
    Feed(units::FeedRate),
    Angle(units::Angle),
    Rpm(units::RotationalSpeed),
}

impl UnitValue {
    /// The canonical source string for this value (round-trips to the file form).
    pub fn to_source_string(self) -> String {
        match self {
            Self::Length(v) => v.to_string(),
            Self::Feed(v) => v.to_string(),
            Self::Angle(v) => v.to_string(),
            Self::Rpm(v) => v.to_string(),
        }
    }
}

/// A reference to another object, identified by UUID.
#[derive(Debug, Clone, PartialEq)]
pub struct Reference {
    /// The raw UUID as written in the file.
    pub raw: Uuid,
    /// The schema id the reference is expected to point at (from `x-ref`).
    pub target: Option<SchemaId>,
    /// Whether the reference has been resolved.
    pub state: RefState,
}

/// Resolution state of a [`Reference`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefState {
    /// Not yet resolved, or no matching object was found.
    Unresolved,
    /// Resolved to an object addressed by [`Handle`].
    Resolved(Handle),
}

/// Opaque handle into a [`crate::ResolvedStore`], addressing a referenced object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Handle(pub(crate) usize);

/// Schema-derived metadata attached to every node — the UI's source of truth.
#[derive(Debug, Clone, PartialEq)]
pub struct Meta {
    /// Property key (empty for the document root and array items).
    pub name: String,
    /// Schema `title`.
    pub title: Option<String>,
    /// Schema `description` (the field's "comments").
    pub description: Option<String>,
    /// Whether the parent schema lists this property as required.
    pub required: bool,
    /// What kind of field this is.
    pub kind: FieldKind,
    /// Whether the value came from a schema `default`/`const` rather than the file.
    pub default_applied: bool,
    /// Validation constraints, surfaced for UI hints.
    pub constraints: Constraints,
}

impl Meta {
    pub(crate) fn bare(name: impl Into<String>, kind: FieldKind) -> Self {
        Self {
            name: name.into(),
            title: None,
            description: None,
            required: false,
            kind,
            default_applied: false,
            constraints: Constraints::default(),
        }
    }
}

/// The classified kind of a field, derived from its schema.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldKind {
    /// A plain scalar (`bool`/`integer`/`number`/`string`/`null`).
    Scalar,
    /// A string constrained to a fixed set of values.
    Enum(Vec<String>),
    /// A unit-bearing value.
    Unit(UnitKind),
    /// This object's own identity (a UUID).
    Id,
    /// A reference to another object, optionally naming the target schema.
    Ref { target: Option<SchemaId> },
    /// A nested object.
    Object,
    /// An array.
    Array,
}

/// Validation constraints copied from the schema for UI use. All optional.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Constraints {
    pub const_value: Option<Value>,
    pub enum_values: Vec<Value>,
    pub minimum: Option<f64>,
    pub maximum: Option<f64>,
    pub min_length: Option<u64>,
    pub max_length: Option<u64>,
    pub pattern: Option<String>,
}

/// Completeness of a node or document.
#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    /// Fully populated and (where applicable) resolved.
    Complete,
    /// One or more required fields missing, or references unresolved.
    Incomplete(Vec<Reason>),
}

impl Status {
    /// Whether this status is [`Status::Complete`].
    pub fn is_complete(&self) -> bool {
        matches!(self, Status::Complete)
    }

    /// Adds a reason, transitioning to (or extending) [`Status::Incomplete`].
    pub(crate) fn add_reason(&mut self, reason: Reason) {
        match self {
            Status::Complete => *self = Status::Incomplete(vec![reason]),
            Status::Incomplete(reasons) => {
                if !reasons.contains(&reason) {
                    reasons.push(reason);
                }
            }
        }
    }
}

/// JSON Pointer tokens escape `~` as `~0` and `/` as `~1`.
fn unescape_pointer_token(token: &str) -> std::borrow::Cow<'_, str> {
    if token.contains('~') {
        std::borrow::Cow::Owned(token.replace("~1", "/").replace("~0", "~"))
    } else {
        std::borrow::Cow::Borrowed(token)
    }
}

/// Collects every identity in a subtree, mapping each to a fresh UUIDv7.
fn collect_identities(node: &Node, remap: &mut std::collections::HashMap<Uuid, Uuid>) {
    match &node.value {
        NodeValue::Id(id) => {
            remap.entry(*id).or_insert_with(Uuid::now_v7);
        }
        NodeValue::Object(map) => map.values().for_each(|c| collect_identities(c, remap)),
        NodeValue::Array(items) => items.iter().for_each(|c| collect_identities(c, remap)),
        _ => {}
    }
}

/// Clones a subtree, applying an identity remap to `Id`s and to internal `Ref`s.
fn clone_remapped(node: &Node, remap: &std::collections::HashMap<Uuid, Uuid>) -> Node {
    let value = match &node.value {
        NodeValue::Id(id) => NodeValue::Id(remap.get(id).copied().unwrap_or(*id)),
        NodeValue::Ref(reference) => match remap.get(&reference.raw) {
            // Internal reference: point at the clone's new id, unresolved again.
            Some(new_id) => NodeValue::Ref(Reference {
                raw: *new_id,
                target: reference.target.clone(),
                state: RefState::Unresolved,
            }),
            // External reference: keep as-is.
            None => NodeValue::Ref(reference.clone()),
        },
        NodeValue::Object(map) => NodeValue::Object(
            map.iter()
                .map(|(key, child)| (key.clone(), clone_remapped(child, remap)))
                .collect(),
        ),
        NodeValue::Array(items) => {
            NodeValue::Array(items.iter().map(|c| clone_remapped(c, remap)).collect())
        }
        other => other.clone(),
    };
    Node {
        value,
        meta: node.meta.clone(),
        status: node.status.clone(),
    }
}

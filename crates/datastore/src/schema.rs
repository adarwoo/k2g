//! Schema registration, compilation, and introspection.
//!
//! A [`SchemaSet`] holds every schema the caller supplied (keyed by `$id`),
//! compiles a reusable JSON Schema [`Validator`] per root schema — registering
//! all the others as resources so cross-file `$ref`s resolve — and offers the
//! `$ref` navigation the parser needs to classify fields and gather defaults.
//!
//! Building a set is *runtime-trusting*: it compiles validators but does not
//! re-check that each schema is itself a valid JSON Schema. That check is a
//! build-time concern, exposed separately as [`SchemaSet::validate`].

use std::collections::HashMap;

use indexmap::IndexMap;
use jsonschema::{options, Resource, Validator};
use serde_json::Value;

use crate::error::SchemaError;

/// One registered schema: its id, original text (for export), and parsed value.
struct RawSchema {
    id: String,
    text: String,
    value: Value,
}

/// A compiled, queryable collection of schemas.
pub struct SchemaSet {
    schemas: IndexMap<String, RawSchema>,
    validators: HashMap<String, Validator>,
}

/// What a `$ref` string points at, for field classification.
pub(crate) enum RefTarget<'a> {
    /// `units.yaml#/$defs/<name>` — a unit-bearing value.
    Units(&'a str),
    /// `id.yaml#/$defs/uuid_v7[_or_null]` — an identity.
    Id,
    /// Anything else — resolve with [`SchemaSet::resolve_ref`] and recurse.
    Structural,
}

/// A `$ref` resolved to its target schema node, plus the base id it now lives in.
pub(crate) struct ResolvedRef<'a> {
    pub value: &'a Value,
    pub base: String,
}

impl SchemaSet {
    /// Builds a set from `(id, yaml_text)` pairs, compiling one validator per
    /// schema. Trusts that each schema is well-formed (see [`Self::validate`]).
    pub fn from_sources(sources: &[(String, String)]) -> Result<Self, SchemaError> {
        let mut schemas: IndexMap<String, RawSchema> = IndexMap::new();
        for (key, text) in sources {
            let value = yaml_to_json(text).map_err(|message| SchemaError::Parse {
                id: key.clone(),
                message,
            })?;
            let id = schema_id(&value, key);
            schemas.insert(id.clone(), RawSchema { id, text: text.clone(), value });
        }

        let mut validators = HashMap::new();
        for (id, raw) in &schemas {
            let compiled = compile_with_resources(&schemas, &raw.value)
                .map_err(|message| SchemaError::Compile { id: id.clone(), message })?;
            validators.insert(id.clone(), compiled);
        }

        Ok(Self { schemas, validators })
    }

    /// Build-time validation: parses and compiles every schema and checks that
    /// each `x-ref` names a registered schema, returning *all* problems found.
    pub fn validate(sources: &[(String, String)]) -> Result<(), Vec<SchemaError>> {
        let mut errors = Vec::new();
        let mut schemas: IndexMap<String, RawSchema> = IndexMap::new();

        for (key, text) in sources {
            match yaml_to_json(text) {
                Ok(value) => {
                    let id = schema_id(&value, key);
                    schemas.insert(id.clone(), RawSchema { id, text: text.clone(), value });
                }
                Err(message) => errors.push(SchemaError::Parse {
                    id: key.clone(),
                    message,
                }),
            }
        }

        for (id, raw) in &schemas {
            if let Err(message) = compile_with_resources(&schemas, &raw.value) {
                errors.push(SchemaError::Compile {
                    id: id.clone(),
                    message,
                });
            }
        }

        for (id, raw) in &schemas {
            for (pointer, target) in collect_x_refs(&raw.value) {
                if !schemas.contains_key(&target) {
                    errors.push(SchemaError::UnknownRefTarget {
                        id: id.clone(),
                        target,
                        pointer,
                    });
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// The compiled validator for a root schema id.
    pub(crate) fn validator(&self, id: &str) -> Option<&Validator> {
        self.validators.get(id)
    }

    /// The parsed root value of a schema.
    pub(crate) fn root(&self, id: &str) -> Option<&Value> {
        self.schemas.get(id).map(|raw| &raw.value)
    }

    /// Iterates `(id, original_text)` for every registered schema — used by
    /// [`crate::DataStore::export_schemas`].
    pub(crate) fn iter_texts(&self) -> impl Iterator<Item = (&str, &str)> {
        self.schemas.values().map(|raw| (raw.id.as_str(), raw.text.as_str()))
    }

    /// Resolves a `$ref` string (relative to `base_id`) to its target node.
    pub(crate) fn resolve_ref(&self, base_id: &str, ref_str: &str) -> Option<ResolvedRef<'_>> {
        let (file, pointer) = split_ref(ref_str);
        let (base, root) = if file.is_empty() {
            (base_id.to_string(), self.root(base_id)?)
        } else {
            (file.to_string(), self.root(file)?)
        };
        let value = if pointer.is_empty() {
            root
        } else {
            root.pointer(pointer)?
        };
        Some(ResolvedRef { value, base })
    }
}

/// Classifies what a `$ref` string points at.
pub(crate) fn ref_target(ref_str: &str) -> RefTarget<'_> {
    let (file, pointer) = split_ref(ref_str);
    if file == "units.yaml" {
        if let Some(name) = pointer.strip_prefix("/$defs/") {
            return RefTarget::Units(name);
        }
    }
    if file == "id.yaml" && (pointer == "/$defs/uuid_v7" || pointer == "/$defs/uuid_v7_or_null") {
        return RefTarget::Id;
    }
    RefTarget::Structural
}

/// Splits a `$ref` into `(file, json_pointer)`. `file` is empty for a local
/// (`#/...`) ref; `pointer` is empty when the ref addresses a whole document.
fn split_ref(ref_str: &str) -> (&str, &str) {
    match ref_str.split_once('#') {
        Some((file, fragment)) => (file, fragment),
        None => (ref_str, ""),
    }
}

/// Determines a schema's id: its `$id` if present, else the caller's key.
fn schema_id(value: &Value, key: &str) -> String {
    value
        .get("$id")
        .and_then(Value::as_str)
        .unwrap_or(key)
        .to_string()
}

/// Compiles a schema with every schema in the set registered as a resource, so
/// cross-file `$ref`s (`units.yaml#/...`) resolve without external retrieval.
fn compile_with_resources(schemas: &IndexMap<String, RawSchema>, root: &Value) -> Result<Validator, String> {
    let mut opts = options();
    for (id, raw) in schemas {
        let resource = Resource::from_contents(raw.value.clone()).map_err(|e| e.to_string())?;
        opts.with_resource(id.clone(), resource.clone());
        opts.with_resource(format!("json-schema:///{id}"), resource);
    }
    opts.build(root).map_err(|e| e.to_string())
}

/// Parses YAML (BOM-tolerant) into a JSON value.
pub(crate) fn yaml_to_json(text: &str) -> Result<Value, String> {
    let stripped = text.strip_prefix('\u{feff}').unwrap_or(text);
    let yaml: serde_yaml::Value = serde_yaml::from_str(stripped).map_err(|e| e.to_string())?;
    serde_json::to_value(yaml).map_err(|e| e.to_string())
}

/// Walks a schema value collecting every `x-ref` string with its JSON Pointer.
fn collect_x_refs(value: &Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    collect_x_refs_into(value, String::new(), &mut out);
    out
}

fn collect_x_refs_into(value: &Value, pointer: String, out: &mut Vec<(String, String)>) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(target)) = map.get("x-ref") {
                out.push((pointer.clone(), target.clone()));
            }
            for (key, child) in map {
                let escaped = key.replace('~', "~0").replace('/', "~1");
                collect_x_refs_into(child, format!("{pointer}/{escaped}"), out);
            }
        }
        Value::Array(items) => {
            for (idx, child) in items.iter().enumerate() {
                collect_x_refs_into(child, format!("{pointer}/{idx}"), out);
            }
        }
        _ => {}
    }
}

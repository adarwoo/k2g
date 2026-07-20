//! Tests for the structural-edit + normalized-load primitives used by the
//! machining migration: [`ResolvedStore::replace_document_from_value`] and
//! [`ResolvedStore::parse_texts`].

use std::path::PathBuf;

use datastore::{DataStore, NodeValue};
use tempfile::tempdir;

const ID_SCHEMA: &str = r#"{ "$id": "id.yaml", "$defs": { "uuid_v7": { "type": "string" } } }"#;
const THING_SCHEMA: &str = r#"
$schema: "https://json-schema.org/draft/2020-12/schema"
$id: "thing.yaml"
type: object
required: [id, items]
properties:
  id: { $ref: "id.yaml#/$defs/uuid_v7" }
  items: { type: array, items: { type: string }, default: [] }
  extra:
    type: object
    properties:
      note: { type: string, default: "hi" }
    additionalProperties: false
  schema_version: { type: integer, const: 1 }
additionalProperties: false
"#;

fn schemas() -> DataStore {
    DataStore::builder()
        .schema("id.yaml", ID_SCHEMA)
        .schema("thing.yaml", THING_SCHEMA)
        .build()
        .expect("schemas compile")
}

#[test]
fn replace_document_from_value_adds_a_property_and_array_items() {
    let dir = tempdir().unwrap();
    let mut store = schemas().open();
    store.set_data_dir(dir.path());
    let id = store.create_document("thing.yaml").expect("create");

    // Structural edit at the value level: append array items and add the
    // optional `extra` object (which was absent).
    let mut value = store.document_by_id(id).unwrap().to_value();
    let obj = value.as_object_mut().unwrap();
    obj.insert("items".into(), serde_json::json!(["a", "b"]));
    obj.insert("extra".into(), serde_json::json!({ "note": "x" }));

    let problems = store
        .replace_document_from_value(id, &value)
        .expect("known id");
    assert!(problems.is_empty(), "unexpected parse problems: {problems:?}");

    let doc = store.document_by_id(id).unwrap();
    let items = doc.root.get_pointer("/items").unwrap();
    assert!(matches!(&items.value, NodeValue::Array(a) if a.len() == 2));
    let note = doc.root.get_pointer("/extra/note").unwrap();
    assert!(matches!(&note.value, NodeValue::Str(s) if s == "x"));

    // The id is preserved (not regenerated).
    assert_eq!(doc.root.identity(), Some(id));

    // And it persisted to the same file (collection dir uses the schema *stem*).
    store.flush();
    let path = dir.path().join("thing").join(format!("{id}.yaml"));
    let saved = std::fs::read_to_string(&path).expect("file written");
    assert!(saved.contains("note: x"), "saved:\n{saved}");
}

#[test]
fn replace_can_shrink_an_array() {
    let dir = tempdir().unwrap();
    let mut store = schemas().open();
    store.set_data_dir(dir.path());
    let id = store.create_document("thing.yaml").expect("create");

    let mut value = store.document_by_id(id).unwrap().to_value();
    value.as_object_mut().unwrap().insert("items".into(), serde_json::json!(["a", "b", "c"]));
    store.replace_document_from_value(id, &value).unwrap();
    assert!(matches!(&store.document_by_id(id).unwrap().root.get_pointer("/items").unwrap().value, NodeValue::Array(a) if a.len() == 3));

    let mut value = store.document_by_id(id).unwrap().to_value();
    value.as_object_mut().unwrap().insert("items".into(), serde_json::json!(["a"]));
    store.replace_document_from_value(id, &value).unwrap();
    assert!(matches!(&store.document_by_id(id).unwrap().root.get_pointer("/items").unwrap().value, NodeValue::Array(a) if a.len() == 1));
}

/// Documents a decisive re-parse behavior: an *optional* object property whose
/// children carry schema `default`s is re-materialized on parse even when the
/// caller omits it from the value. (This is why `machining.yaml` must not gate
/// op-config objects behind `allOf` conditional-presence rules — they are always
/// present; the `operations` array is the sole enablement signal.)
#[test]
fn replace_re_materializes_defaulted_optional_objects() {
    let dir = tempdir().unwrap();
    let mut store = schemas().open();
    store.set_data_dir(dir.path());
    let id = store.create_document("thing.yaml").expect("create");

    // Even omitting `extra` entirely, parse re-adds it with its child defaults.
    let mut value = store.document_by_id(id).unwrap().to_value();
    value.as_object_mut().unwrap().remove("extra");
    store.replace_document_from_value(id, &value).unwrap();

    let note = store.document_by_id(id).unwrap().root.get_pointer("/extra/note");
    assert!(matches!(note.map(|n| &n.value), Some(NodeValue::Str(s)) if s == "hi"));
}

/// The path-addressed twin used for singletons (stock, settings) that have no
/// root identity and are located by their source file. Mirrors the id-based
/// path: re-parse, swap, schedule write — but keyed on the source path.
#[test]
fn replace_document_from_value_at_edits_a_singleton_by_path() {
    let dir = tempdir().unwrap();
    let collection = dir.path().join("things");
    let mut store = schemas().open();

    let id = uuid::Uuid::now_v7();
    let path = collection.join(format!("{id}.yaml"));
    let text = format!("id: \"{id}\"\nitems: [\"a\"]\nschema_version: 1\n");
    store.parse_texts("thing.yaml", &collection, &[(path.clone(), text)]);

    // Structural edit addressed purely by source path (no root-identity lookup).
    let mut value = store.document_by_id(id).unwrap().to_value();
    value
        .as_object_mut()
        .unwrap()
        .insert("items".into(), serde_json::json!(["a", "b", "c"]));

    let problems = store
        .replace_document_from_value_at(&path, &value)
        .expect("known source path");
    assert!(problems.is_empty(), "unexpected parse problems: {problems:?}");

    let items = store.document_by_id(id).unwrap().root.get_pointer("/items").unwrap();
    assert!(matches!(&items.value, NodeValue::Array(a) if a.len() == 3));

    // An unknown source path is a `None`, not a panic.
    let missing = collection.join("nope.yaml");
    assert!(store.replace_document_from_value_at(&missing, &value).is_none());

    // The edit scheduled a write back to the same file.
    store.flush();
    assert!(path.exists(), "singleton edit should persist to its source file");
}

#[test]
fn replace_document_from_value_unknown_id_is_none() {
    let dir = tempdir().unwrap();
    let mut store = schemas().open();
    store.set_data_dir(dir.path());
    let value = serde_json::json!({ "id": uuid::Uuid::now_v7().to_string(), "items": [] });
    assert!(store.replace_document_from_value(uuid::Uuid::now_v7(), &value).is_none());
}

#[test]
fn parse_texts_loads_pre_read_content_and_registers_the_collection() {
    let dir = tempdir().unwrap();
    let collection = dir.path().join("things");
    let mut store = schemas().open();

    let id = uuid::Uuid::now_v7();
    let text = format!("id: \"{id}\"\nitems: [\"z\"]\nschema_version: 1\n");
    let items = vec![(collection.join(format!("{id}.yaml")), text)];

    let problems = store.parse_texts("thing.yaml", &collection, &items);
    assert!(problems.is_empty(), "unexpected problems: {problems:?}");

    let doc = store.document_by_id(id).expect("loaded");
    assert!(matches!(&doc.root.get_pointer("/items/0").unwrap().value, NodeValue::Str(s) if s == "z"));

    // The collection is registered, so a new document lands in `collection`.
    let new_id = store.create_document("thing.yaml").expect("create in registered dir");
    store.flush();
    let path: PathBuf = collection.join(format!("{new_id}.yaml"));
    assert!(path.exists(), "new doc should be written under the registered collection dir");
}

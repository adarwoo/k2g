//! Tests for the schema-aware string write path used by the UI:
//! [`ResolvedStore::set_value_str_by_id`].

use datastore::{DataStore, NodeValue, UnitValue};
use tempfile::tempdir;

const ID_SCHEMA: &str = r#"{ "$id": "id.yaml", "$defs": { "uuid_v7": { "type": "string" } } }"#;
const UNITS_SCHEMA: &str = r#"{ "$id": "units.yaml", "$defs": { "size": { "type": "string" } } }"#;
const THING_SCHEMA: &str = r#"
$schema: "https://json-schema.org/draft/2020-12/schema"
$id: "thing.yaml"
type: object
required: [id]
properties:
  id: { $ref: "id.yaml#/$defs/uuid_v7" }
  width: { $ref: "units.yaml#/$defs/size", default: "1mm" }
  count: { type: integer, default: 0 }
  ratio: { type: number, default: 1.0 }
  mode: { type: string, enum: [a, b, c], default: a }
  schema_version: { type: integer, const: 1 }
"#;

fn store(dir: &std::path::Path) -> datastore::ResolvedStore {
    let schemas = DataStore::builder()
        .schema("id.yaml", ID_SCHEMA)
        .schema("units.yaml", UNITS_SCHEMA)
        .schema("thing.yaml", THING_SCHEMA)
        .build()
        .expect("schemas compile");
    let mut store = schemas.open();
    store.set_data_dir(dir);
    store
}

#[test]
fn decodes_units_ints_numbers_and_enums_from_strings() {
    let dir = tempdir().unwrap();
    let mut store = store(dir.path());
    let id = store.create_document("thing.yaml").expect("create");

    // Unit field: "5mm" → typed Length.
    assert_eq!(store.set_value_str_by_id(id, "/width", "5mm"), Some(true));
    let width = store.document_by_id(id).unwrap().root.get_pointer("/width").unwrap();
    match &width.value {
        NodeValue::Unit(u @ UnitValue::Length(_)) => assert_eq!(u.to_source_string(), "5mm"),
        other => panic!("expected length, got {other:?}"),
    }

    // Integer field.
    assert_eq!(store.set_value_str_by_id(id, "/count", "42"), Some(true));
    assert!(matches!(
        store.document_by_id(id).unwrap().root.get_pointer("/count").unwrap().value,
        NodeValue::Int(42)
    ));

    // Number field.
    assert_eq!(store.set_value_str_by_id(id, "/ratio", "2.5"), Some(true));
    assert!(matches!(
        store.document_by_id(id).unwrap().root.get_pointer("/ratio").unwrap().value,
        NodeValue::Float(f) if (f - 2.5).abs() < 1e-9
    ));

    // Enum / string field stays a string.
    assert_eq!(store.set_value_str_by_id(id, "/mode", "b"), Some(true));
    assert!(matches!(
        &store.document_by_id(id).unwrap().root.get_pointer("/mode").unwrap().value,
        NodeValue::Str(s) if s == "b"
    ));
}

#[test]
fn rejects_undecodable_values_without_changing_the_field() {
    let dir = tempdir().unwrap();
    let mut store = store(dir.path());
    let id = store.create_document("thing.yaml").expect("create");

    // Non-numeric into an integer field → rejected, value stays at default 0.
    assert_eq!(store.set_value_str_by_id(id, "/count", "not-a-number"), Some(false));
    assert!(matches!(
        store.document_by_id(id).unwrap().root.get_pointer("/count").unwrap().value,
        NodeValue::Int(0)
    ));
}

#[test]
fn unknown_id_or_pointer_returns_none_or_false() {
    let dir = tempdir().unwrap();
    let mut store = store(dir.path());
    let id = store.create_document("thing.yaml").expect("create");

    // Unknown id → None.
    assert_eq!(store.set_value_str_by_id(uuid::Uuid::now_v7(), "/count", "1"), None);
    // Unknown pointer → decode returns None → Some(false).
    assert_eq!(store.set_value_str_by_id(id, "/nope", "1"), Some(false));
}

//! Tests for the *from a source document* factory path:
//! [`DataStore::instantiate_from`] and [`ResolvedStore::create_document_from`].

use datastore::{DataStore, NodeValue, UnitValue};
use serde_json::json;
use tempfile::tempdir;

// Minimal stand-ins for the shared schemas (classification is by `$ref` string,
// so the target bodies need only be resolvable for validation).
const ID_SCHEMA: &str = r#"{ "$id": "id.yaml", "$defs": { "uuid_v7": { "type": "string" } } }"#;
const UNITS_SCHEMA: &str = r#"{ "$id": "units.yaml", "$defs": { "size": { "type": "string" } } }"#;
const THING_SCHEMA: &str = r#"
$schema: "https://json-schema.org/draft/2020-12/schema"
$id: "thing.yaml"
type: object
required: [id, name]
properties:
  id: { $ref: "id.yaml#/$defs/uuid_v7" }
  name: { type: string, default: "default name" }
  width: { $ref: "units.yaml#/$defs/size", default: "1mm" }
  schema_version: { type: integer, const: 1 }
"#;

fn schemas() -> DataStore {
    DataStore::builder()
        .schema("id.yaml", ID_SCHEMA)
        .schema("units.yaml", UNITS_SCHEMA)
        .schema("thing.yaml", THING_SCHEMA)
        .build()
        .expect("schemas compile")
}

#[test]
fn instantiate_from_overlays_seed_over_defaults_and_assigns_fresh_id() {
    let store = schemas();
    // Seed carries no id and no schema_version — both must be supplied for us.
    let seed = json!({ "name": "Seeded", "width": "5mm" });
    let node = store.instantiate_from("thing.yaml", &seed).expect("instantiate");

    // A fresh identity was assigned even though the seed had none.
    assert!(node.identity().is_some(), "expected a generated id");

    // Seed value wins over the schema default.
    let name = node.get_pointer("/name").unwrap();
    assert!(matches!(&name.value, NodeValue::Str(s) if s == "Seeded"));

    // Unit-bearing field decoded from the seed's value.
    let width = node.get_pointer("/width").unwrap();
    assert!(matches!(width.value, NodeValue::Unit(UnitValue::Length(_))));

    // `const` materialised from the schema even though the seed omitted it.
    let version = node.get_pointer("/schema_version").unwrap();
    assert!(matches!(version.value, NodeValue::Int(1)));

    assert!(node.status.is_complete(), "status: {:?}", node.status);
}

#[test]
fn instantiate_from_falls_back_to_defaults_for_absent_seed_fields() {
    let store = schemas();
    let seed = json!({ "name": "Only name" });
    let node = store.instantiate_from("thing.yaml", &seed).unwrap();

    // `width` was not in the seed, so it takes the schema default value (1mm).
    // Note: like `instantiate`, the factory materialises defaults into the value
    // before annotation, so the `default_applied` *flag* is not set here — the
    // default *value* is what matters.
    let width = node.get_pointer("/width").unwrap();
    match &width.value {
        NodeValue::Unit(unit @ UnitValue::Length(_)) => {
            assert_eq!(unit.to_source_string(), "1mm");
        }
        other => panic!("expected a length, got {other:?}"),
    }
}

#[test]
fn instantiate_from_ignores_any_id_in_the_seed() {
    let store = schemas();
    let seed = json!({ "id": "0194fd2c-5f2e-7a9d-9a76-3b2b9fbc3f11", "name": "X" });
    let node = store.instantiate_from("thing.yaml", &seed).unwrap();

    let id = node.identity().expect("has id");
    assert_ne!(
        id.to_string(),
        "0194fd2c-5f2e-7a9d-9a76-3b2b9fbc3f11",
        "seed id must be regenerated, not reused"
    );
}

#[test]
fn instantiate_from_unknown_schema_is_none() {
    let store = schemas();
    assert!(store.instantiate_from("nope.yaml", &json!({})).is_none());
}

#[test]
fn create_document_from_stores_a_new_file_and_loads_it() {
    let dir = tempdir().unwrap();
    let mut store = schemas().open();
    store.set_data_dir(dir.path());

    let seed = json!({ "name": "From Template", "width": "3mm" });
    let id = store.create_document_from("thing.yaml", &seed).expect("create");
    store.flush();

    // Written one-file-per-instance at <data_dir>/thing/<id>.yaml.
    let path = dir.path().join("thing").join(format!("{id}.yaml"));
    assert!(path.exists(), "expected file at {}", path.display());

    // Loaded, complete, and carrying the seeded name with a fresh id.
    let doc = store.document_by_id(id).expect("document present");
    let name = doc.root.get_pointer("/name").unwrap();
    assert!(matches!(&name.value, NodeValue::Str(s) if s == "From Template"));
    assert!(doc.status.is_complete(), "status: {:?}", doc.status);

    // The persisted file re-stamps `$schema` and is valid YAML.
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(written.contains("$schema: thing.yaml"), "written:\n{written}");
}

//! Build-time schema validation and schema export.

mod common;

use datastore::{validate_schemas, DataStore, SchemaError};

#[test]
fn accepts_valid_schema_set() {
    validate_schemas(&[
        ("id.yaml", common::ID),
        ("units.yaml", common::UNITS),
        ("gadget.yaml", common::GADGET),
        ("widget.yaml", common::WIDGET),
    ])
    .expect("the fixture schemas should be valid");
}

#[test]
fn rejects_unknown_x_ref_target() {
    // widget.yaml x-refs gadget.yaml; leave gadget.yaml out of the set.
    let errors = validate_schemas(&[
        ("id.yaml", common::ID),
        ("units.yaml", common::UNITS),
        ("widget.yaml", common::WIDGET),
    ])
    .expect_err("a dangling x-ref target should be rejected");

    assert!(
        errors.iter().any(|e| matches!(
            e,
            SchemaError::UnknownRefTarget { target, .. } if target == "gadget.yaml"
        )),
        "errors: {errors:?}"
    );
}

#[test]
fn rejects_unresolvable_ref() {
    let broken = r#"
$schema: "https://json-schema.org/draft/2020-12/schema"
$id: "broken.yaml"
type: object
properties:
  x:
    $ref: "missing.yaml#/$defs/nope"
"#;

    let errors = validate_schemas(&[("broken.yaml", broken)])
        .expect_err("an unresolvable $ref should be rejected");

    assert!(
        errors
            .iter()
            .any(|e| matches!(e, SchemaError::Compile { .. })),
        "errors: {errors:?}"
    );
}

#[test]
fn exports_schemas_to_folder() {
    let store = DataStore::builder()
        .schema("id.yaml", common::ID)
        .schema("units.yaml", common::UNITS)
        .schema("gadget.yaml", common::GADGET)
        .schema("widget.yaml", common::WIDGET)
        .build()
        .unwrap();

    let dir = tempfile::tempdir().unwrap();
    store.export_schemas(dir.path()).unwrap();

    for name in ["id.yaml", "units.yaml", "gadget.yaml", "widget.yaml"] {
        let written = std::fs::read_to_string(dir.path().join(name)).unwrap();
        assert!(written.contains(&format!("$id: \"{name}\"")), "{name} content");
    }
}

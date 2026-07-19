//! Parsing: unit decode, defaults, metadata, and non-fatal error collection.

mod common;

use datastore::{DataErrorKind, FieldKind, NodeValue, ParseInput, Reason, Status, UnitValue};

#[test]
fn parses_units_defaults_and_metadata() {
    let store = common::store();
    let text = r#"
id: "0194fd2c-5f2e-7a9d-9a76-3b2b9fbc3f11"
kind: alpha
width: "10mm"
gadget_ref: "0194fd2c-5f2e-7a9d-9a76-3b2b9fbc3f22"
spindle: "8000rpm"
"#;

    let out = store.parse(&[ParseInput {
        schema_id: "widget.yaml",
        source: None,
        text,
    }]);
    assert!(out.errors.is_empty(), "unexpected errors: {:?}", out.errors);
    let doc = &out.documents[0];

    // Identity decoded to a UUID.
    let id = doc.root.get_pointer("/id").unwrap();
    assert!(matches!(id.value, NodeValue::Id(_)));

    // Unit field decoded to a typed Length.
    let width = doc.root.get_pointer("/width").unwrap();
    assert!(matches!(width.value, NodeValue::Unit(UnitValue::Length(_))));

    // Optional unit (via anyOf) still decodes.
    let spindle = doc.root.get_pointer("/spindle").unwrap();
    assert!(matches!(spindle.value, NodeValue::Unit(UnitValue::Rpm(_))));

    // Reference recorded, unresolved, carrying its declared target.
    let gref = doc.root.get_pointer("/gadget_ref").unwrap();
    match &gref.value {
        NodeValue::Ref(reference) => {
            assert!(matches!(reference.state, datastore::RefState::Unresolved));
            assert_eq!(reference.target.as_deref(), Some("gadget.yaml"));
        }
        other => panic!("expected a reference, got {other:?}"),
    }
    assert!(matches!(gref.meta.kind, FieldKind::Ref { .. }));

    // Enum + descriptive metadata carried from the schema.
    let kind = doc.root.get_pointer("/kind").unwrap();
    match &kind.meta.kind {
        FieldKind::Enum(variants) => assert_eq!(variants, &["alpha", "beta", "gamma"]),
        other => panic!("expected enum, got {other:?}"),
    }
    assert_eq!(kind.meta.title.as_deref(), Some("Kind"));
    assert_eq!(kind.meta.description.as_deref(), Some("Widget kind."));
    assert!(kind.meta.required);
    assert!(!width.meta.required);

    // Absent scalar picked up its schema default.
    let note = doc.root.get_pointer("/note").unwrap();
    assert!(note.meta.default_applied);
    assert!(matches!(&note.value, NodeValue::Str(s) if s == "n/a"));

    // Absent nested object was synthesized from its children's defaults.
    let settings = doc.root.get_pointer("/settings").unwrap();
    assert!(settings.meta.default_applied);
    let speed = doc.root.get_pointer("/settings/speed").unwrap();
    assert!(matches!(speed.value, NodeValue::Unit(UnitValue::Rpm(_))));
    let label = doc.root.get_pointer("/settings/label").unwrap();
    assert!(matches!(&label.value, NodeValue::Str(s) if s == "unnamed"));

    assert!(doc.status.is_complete());
}

#[test]
fn flags_missing_required_field() {
    let store = common::store();
    let text = r#"
id: "0194fd2c-5f2e-7a9d-9a76-3b2b9fbc3f11"
"#;

    let out = store.parse(&[ParseInput {
        schema_id: "widget.yaml",
        source: None,
        text,
    }]);

    assert!(
        out.errors
            .iter()
            .any(|e| e.kind == DataErrorKind::MissingRequired && e.pointer == "/kind"),
        "errors: {:?}",
        out.errors
    );

    let doc = &out.documents[0];
    assert!(!doc.status.is_complete());
    match &doc.root.status {
        Status::Incomplete(reasons) => assert!(reasons
            .iter()
            .any(|r| matches!(r, Reason::MissingRequired(p) if p == "/kind"))),
        Status::Complete => panic!("root should be incomplete"),
    }
}

#[test]
fn collects_unit_decode_error_without_losing_data() {
    let store = common::store();
    let text = r#"
id: "0194fd2c-5f2e-7a9d-9a76-3b2b9fbc3f11"
kind: beta
width: "10 furlongs"
"#;

    let out = store.parse(&[ParseInput {
        schema_id: "widget.yaml",
        source: None,
        text,
    }]);

    assert!(
        out.errors
            .iter()
            .any(|e| e.kind == DataErrorKind::Unit && e.pointer == "/width"),
        "errors: {:?}",
        out.errors
    );

    // The un-decodable value is preserved verbatim for round-tripping.
    let width = out.documents[0].root.get_pointer("/width").unwrap();
    assert!(matches!(&width.value, NodeValue::Str(s) if s == "10 furlongs"));
}

#[test]
fn round_trips_to_value() {
    let store = common::store();
    let text = r#"
id: "0194fd2c-5f2e-7a9d-9a76-3b2b9fbc3f11"
kind: gamma
width: "1/8in"
"#;

    let out = store.parse(&[ParseInput {
        schema_id: "widget.yaml",
        source: None,
        text,
    }]);
    let value = out.documents[0].to_value();
    // The fractional inch length survives as its canonical source string.
    assert_eq!(value["width"], serde_json::json!("1/8in"));
    assert_eq!(value["kind"], serde_json::json!("gamma"));
}

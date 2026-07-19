//! Reference resolution across documents, with incomplete/unresolved flags.

mod common;

use datastore::{NodeValue, ParseInput, RefState};

#[test]
fn resolves_reference_across_documents() {
    let store = common::store();

    let widget = r#"
id: "0194fd2c-5f2e-7a9d-9a76-3b2b9fbc3f11"
kind: alpha
gadget_ref: "0194fd2c-5f2e-7a9d-9a76-3b2b9fbc3faa"
"#;
    let gadget = r#"
gadgets:
  - id: "0194fd2c-5f2e-7a9d-9a76-3b2b9fbc3faa"
    name: "Widget Mount"
"#;

    let out = store.parse(&[
        ParseInput {
            schema_id: "widget.yaml",
            source: None,
            text: widget,
        },
        ParseInput {
            schema_id: "gadget.yaml",
            source: None,
            text: gadget,
        },
    ]);
    assert!(out.errors.is_empty(), "errors: {:?}", out.errors);

    let resolved = store.resolve(out.documents);
    let widget_doc = &resolved.documents()[0];

    let gref = widget_doc.root.get_pointer("/gadget_ref").unwrap();
    let handle = match &gref.value {
        NodeValue::Ref(reference) => match reference.state {
            RefState::Resolved(handle) => handle,
            RefState::Unresolved => panic!("reference should have resolved"),
        },
        other => panic!("expected a reference, got {other:?}"),
    };

    // The handle addresses the gadget object in the other document.
    let target = resolved.get(handle).expect("handle resolves to a node");
    let name = target.get_pointer("/name").unwrap();
    assert!(matches!(&name.value, NodeValue::Str(s) if s == "Widget Mount"));

    assert!(widget_doc.status.is_complete());
}

#[test]
fn flags_dangling_reference_as_incomplete() {
    let store = common::store();

    let widget = r#"
id: "0194fd2c-5f2e-7a9d-9a76-3b2b9fbc3f11"
kind: alpha
gadget_ref: "0194fd2c-5f2e-7a9d-9a76-3b2b9fbc3fff"
"#;

    let out = store.parse(&[ParseInput {
        schema_id: "widget.yaml",
        source: None,
        text: widget,
    }]);

    let resolved = store.resolve(out.documents);
    let doc = &resolved.documents()[0];

    let gref = doc.root.get_pointer("/gadget_ref").unwrap();
    assert!(matches!(&gref.value, NodeValue::Ref(r) if r.state == RefState::Unresolved));
    assert!(!doc.status.is_complete());
}

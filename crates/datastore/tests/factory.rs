//! Factories: instantiate, create-as-file, add item, clone, export, versioning.

mod common;

use datastore::{DataErrorKind, NodeValue, ParseInput, UnitValue};

#[test]
fn instantiate_applies_defaults_and_generates_ids() {
    let store = common::store();
    let node = store.instantiate("toolset.yaml").expect("known schema");

    // Identity generated.
    assert!(matches!(node.get_pointer("/id").unwrap().value, NodeValue::Id(_)));
    // Const default carried.
    assert!(matches!(node.get_pointer("/schema_version").unwrap().value, NodeValue::Int(2)));
    // Array default carried.
    assert!(matches!(&node.get_pointer("/items").unwrap().value, NodeValue::Array(a) if a.is_empty()));
    // Required-without-default absent → incomplete.
    assert!(node.get_pointer("/name").is_none());
    assert!(!node.status.is_complete());
}

#[test]
fn create_document_writes_one_file_per_instance() {
    let store = common::store();
    let mut resolved = store.resolve(store.parse(&[]).documents);
    let dir = tempfile::tempdir().unwrap();
    resolved.set_data_dir(dir.path());

    let id = resolved.create_document("toolset.yaml").expect("create");
    let path = resolved.document_by_id(id).unwrap().source.clone().unwrap();
    assert_eq!(path.parent().unwrap(), dir.path().join("toolset"));
    assert_eq!(path.extension().unwrap(), "yaml");
    assert_eq!(path.file_stem().unwrap().to_str().unwrap(), id.to_string());

    resolved.flush();
    assert!(resolved.write_errors().is_empty());
    assert!(path.exists());

    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("$schema"));
    assert!(text.contains("toolset.yaml"));
    assert!(text.contains("schema_version: 2"));
}

#[test]
fn add_item_appends_defaulted_item() {
    let store = common::store();
    let mut resolved = store.resolve(store.parse(&[]).documents);
    let dir = tempfile::tempdir().unwrap();
    resolved.set_data_dir(dir.path());
    let id = resolved.create_document("toolset.yaml").unwrap();
    let path = resolved.document_by_id(id).unwrap().source.clone().unwrap();

    let index = resolved.add_item(&path, "/items").expect("add item");
    assert_eq!(index, 0);

    let doc = resolved
        .documents()
        .iter()
        .find(|d| d.source.as_deref() == Some(path.as_path()))
        .unwrap();
    let item = doc.root.get_pointer("/items/0").unwrap();
    assert!(matches!(item.get_pointer("/id").unwrap().value, NodeValue::Id(_)));
    assert!(matches!(
        item.get_pointer("/width").unwrap().value,
        NodeValue::Unit(UnitValue::Length(_))
    ));
    assert!(item.get_pointer("/label").is_none()); // required, still to fill
}

#[test]
fn clone_document_regenerates_ids_and_keeps_external_refs() {
    let store = common::store();
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("orig.yaml");
    let text = r#"
$schema: "toolset.yaml"
schema_version: 2
id: "0194fd2c-5f2e-7a9d-9a76-3b2b9fbc3f11"
name: "Original"
gadget_ref: "0194fd2c-5f2e-7a9d-9a76-3b2b9fbc3faa"
items:
  - id: "0194fd2c-5f2e-7a9d-9a76-3b2b9fbc3f22"
    label: "bit"
    width: "2mm"
"#;

    let out = store.parse(&[ParseInput {
        schema_id: "toolset.yaml",
        source: Some(src.clone()),
        text,
    }]);
    assert!(out.errors.is_empty(), "errors: {:?}", out.errors);
    let mut resolved = store.resolve(out.documents);
    resolved.set_data_dir(dir.path());

    let orig_root_id = resolved.documents()[0].root.get_pointer("/id").unwrap().value.clone();
    let orig_item_id = resolved.documents()[0].root.get_pointer("/items/0/id").unwrap().value.clone();

    let clone_id = resolved.clone_document(&src).expect("clone");
    resolved.flush();

    let clone = resolved.document_by_id(clone_id).unwrap();

    // Identities regenerated.
    assert_ne!(clone.root.get_pointer("/id").unwrap().value, orig_root_id);
    assert_ne!(clone.root.get_pointer("/items/0/id").unwrap().value, orig_item_id);

    // External reference preserved verbatim.
    match &clone.root.get_pointer("/gadget_ref").unwrap().value {
        NodeValue::Ref(reference) => {
            assert_eq!(reference.raw.to_string(), "0194fd2c-5f2e-7a9d-9a76-3b2b9fbc3faa")
        }
        other => panic!("expected a reference, got {other:?}"),
    }
}

#[test]
fn rejects_newer_older_and_missing_versions() {
    let store = common::store();
    let body = "id: \"0194fd2c-5f2e-7a9d-9a76-3b2b9fbc3f11\"\nname: X\n";

    for (version_line, needle) in [
        ("schema_version: 3\n", "newer"),
        ("schema_version: 1\n", "older"),
        ("", "missing"),
    ] {
        let text = format!("{version_line}{body}");
        let out = store.parse(&[ParseInput {
            schema_id: "toolset.yaml",
            source: None,
            text: &text,
        }]);
        assert!(out.documents.is_empty(), "should reject ({needle})");
        assert!(
            out.errors.iter().any(|e| e.kind == DataErrorKind::SchemaVersion
                && e.message.contains(needle)),
            "expected a {needle} version error, got {:?}",
            out.errors
        );
    }

    // Matching version parses cleanly.
    let ok = format!("schema_version: 2\n{body}");
    let out = store.parse(&[ParseInput {
        schema_id: "toolset.yaml",
        source: None,
        text: &ok,
    }]);
    assert!(out.errors.is_empty(), "errors: {:?}", out.errors);
    assert_eq!(out.documents.len(), 1);
}

#[test]
fn export_writes_a_copy_to_target() {
    let store = common::store();
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("orig.yaml");
    let text = "schema_version: 2\nid: \"0194fd2c-5f2e-7a9d-9a76-3b2b9fbc3f11\"\nname: X\n";

    let out = store.parse(&[ParseInput {
        schema_id: "toolset.yaml",
        source: Some(src.clone()),
        text,
    }]);
    assert!(out.errors.is_empty(), "errors: {:?}", out.errors);
    let mut resolved = store.resolve(out.documents);

    let target = dir.path().join("exported.yaml");
    resolved.export(&src, &target).expect("export");
    resolved.flush();

    assert!(target.exists());
    let content = std::fs::read_to_string(&target).unwrap();
    assert!(content.contains("$schema"));
    assert!(content.contains("name: X"));
}

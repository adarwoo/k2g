//! Directory-oriented loading and path-free create/edit.

mod common;

use datastore::{NodeValue, RefState, RemoveError};

const A: &str = "0194fd2c-5f2e-7a9d-9a76-3b2b9fbc3f11";
const B: &str = "0194fd2c-5f2e-7a9d-9a76-3b2b9fbc3f22";

/// A minimal valid toolset file (schema_version 2), optionally referencing another id.
fn toolset_yaml(id: &str, name: &str, gadget_ref: Option<&str>) -> String {
    let reference = gadget_ref
        .map(|g| format!("gadget_ref: \"{g}\"\n"))
        .unwrap_or_default();
    format!("$schema: \"toolset.yaml\"\nschema_version: 2\nid: \"{id}\"\nname: \"{name}\"\n{reference}")
}

#[test]
fn parse_directory_loads_all_lists_ids_and_resolves_across_files() {
    let store = common::store();
    let mut resolved = store.open();
    let dir = tempfile::tempdir().unwrap();

    // Files are named <uuid>.yaml. A references B.
    std::fs::write(
        dir.path().join(format!("{A}.yaml")),
        toolset_yaml(A, "Alpha", Some(B)),
    )
    .unwrap();
    std::fs::write(
        dir.path().join(format!("{B}.yaml")),
        toolset_yaml(B, "Beta", None),
    )
    .unwrap();

    let errors = resolved.parse_directory("toolset.yaml", dir.path());
    assert!(errors.is_empty(), "errors: {errors:?}");

    // The datastore now lists the ids of this "type".
    let mut ids: Vec<String> = resolved
        .document_ids("toolset.yaml")
        .iter()
        .map(|u| u.to_string())
        .collect();
    ids.sort();
    assert_eq!(ids, vec![A.to_string(), B.to_string()]);

    // A's reference to B resolved as part of the directory load.
    let a_id = *resolved
        .document_ids("toolset.yaml")
        .iter()
        .find(|u| u.to_string() == A)
        .unwrap();
    let doc_a = resolved.document_by_id(a_id).unwrap();
    let gref = doc_a.root.get_pointer("/gadget_ref").unwrap();
    assert!(matches!(&gref.value, NodeValue::Ref(r) if matches!(r.state, RefState::Resolved(_))));
}

#[test]
fn set_value_by_id_persists_without_a_path() {
    let store = common::store();
    let mut resolved = store.open();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(format!("{A}.yaml")),
        toolset_yaml(A, "Alpha", None),
    )
    .unwrap();
    resolved.parse_directory("toolset.yaml", dir.path());

    let id = *resolved.document_ids("toolset.yaml").first().unwrap();
    assert_eq!(
        resolved.set_value_by_id(id, "/name", NodeValue::Str("Renamed".into())),
        Some(true)
    );
    resolved.flush();

    let text = std::fs::read_to_string(dir.path().join(format!("{A}.yaml"))).unwrap();
    assert!(text.contains("name: Renamed"), "file was: {text}");
}

#[test]
fn create_document_lands_in_the_parsed_directory() {
    let store = common::store();
    let mut resolved = store.open();
    let dir = tempfile::tempdir().unwrap();

    // Associating the directory (even empty) is enough — no path needed to create.
    resolved.parse_directory("toolset.yaml", dir.path());
    let id = resolved.create_document("toolset.yaml").expect("create");
    resolved.flush();

    let path = resolved.document_by_id(id).unwrap().source.clone().unwrap();
    assert_eq!(path.parent().unwrap(), dir.path());
    assert_eq!(path.file_stem().unwrap().to_str().unwrap(), id.to_string());
    assert!(path.exists());
}

#[test]
fn parse_collection_uses_data_dir_convention() {
    let store = common::store();
    let mut resolved = store.open();
    let base = tempfile::tempdir().unwrap();
    let collection = base.path().join("toolset");
    std::fs::create_dir_all(&collection).unwrap();
    std::fs::write(
        collection.join(format!("{A}.yaml")),
        toolset_yaml(A, "Alpha", None),
    )
    .unwrap();

    resolved.set_data_dir(base.path());
    let errors = resolved
        .parse_collection("toolset.yaml")
        .expect("collection directory is known");
    assert!(errors.is_empty(), "errors: {errors:?}");
    assert_eq!(resolved.document_ids("toolset.yaml").len(), 1);
}

#[test]
fn remove_document_deletes_file_and_drops_it() {
    let store = common::store();
    let mut resolved = store.open();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(format!("{A}.yaml")),
        toolset_yaml(A, "Alpha", None),
    )
    .unwrap();
    resolved.parse_directory("toolset.yaml", dir.path());

    let id = *resolved.document_ids("toolset.yaml").first().unwrap();
    resolved.remove_document(id).expect("remove");
    resolved.flush();

    assert!(resolved.document_by_id(id).is_none());
    assert!(resolved.document_ids("toolset.yaml").is_empty());
    assert!(!dir.path().join(format!("{A}.yaml")).exists());
}

#[test]
fn remove_document_blocked_when_referenced_points_at_the_dependant() {
    let store = common::store();
    let mut resolved = store.open();
    let dir = tempfile::tempdir().unwrap();
    // A references B.
    std::fs::write(
        dir.path().join(format!("{A}.yaml")),
        toolset_yaml(A, "Alpha", Some(B)),
    )
    .unwrap();
    std::fs::write(
        dir.path().join(format!("{B}.yaml")),
        toolset_yaml(B, "Beta", None),
    )
    .unwrap();
    resolved.parse_directory("toolset.yaml", dir.path());

    let b_id = *resolved
        .document_ids("toolset.yaml")
        .iter()
        .find(|u| u.to_string() == B)
        .unwrap();

    match resolved.remove_document(b_id).expect_err("should be blocked") {
        RemoveError::InUse { id, referrers } => {
            assert_eq!(id, b_id);
            assert_eq!(referrers.len(), 1);
            let referrer = &referrers[0];
            assert_eq!(referrer.target_id, b_id);
            assert_eq!(referrer.pointer, "/gadget_ref");
            assert_eq!(referrer.document_id.map(|u| u.to_string()).as_deref(), Some(A));
        }
        other => panic!("expected InUse, got {other:?}"),
    }

    // B is untouched.
    assert!(resolved.document_by_id(b_id).is_some());
    assert!(dir.path().join(format!("{B}.yaml")).exists());
}

#[test]
fn remove_unknown_id_is_not_found() {
    let store = common::store();
    let mut resolved = store.open();
    let dir = tempfile::tempdir().unwrap();
    resolved.parse_directory("toolset.yaml", dir.path());

    let id = resolved.create_document("toolset.yaml").unwrap();
    resolved.remove_document(id).expect("first remove");
    assert!(matches!(
        resolved.remove_document(id).expect_err("already gone"),
        RemoveError::NotFound(_)
    ));
}

//! Auto-persistence: editing a document writes it back to its source file.

mod common;

use std::path::Path;

use datastore::{DataStore, NodeValue, ParseInput, ResolvedStore};

/// Writes `text` to `path`, parses it as a widget, and resolves.
fn load(store: &DataStore, path: &Path, text: &str) -> ResolvedStore {
    std::fs::write(path, text).unwrap();
    let out = store.parse(&[ParseInput {
        schema_id: "widget.yaml",
        source: Some(path.to_path_buf()),
        text,
    }]);
    assert!(out.errors.is_empty(), "parse errors: {:?}", out.errors);
    store.resolve(out.documents)
}

/// Re-reads and re-parses `path`, returning the string value at `pointer`.
fn read_back(store: &DataStore, path: &Path, pointer: &str) -> String {
    let text = std::fs::read_to_string(path).unwrap();
    let out = store.parse(&[ParseInput {
        schema_id: "widget.yaml",
        source: None,
        text: &text,
    }]);
    assert!(out.errors.is_empty(), "re-parse errors: {:?}", out.errors);
    match &out.documents[0].root.get_pointer(pointer).unwrap().value {
        NodeValue::Str(s) => s.clone(),
        other => panic!("expected string at {pointer}, got {other:?}"),
    }
}

const SEED: &str = "id: \"0194fd2c-5f2e-7a9d-9a76-3b2b9fbc3f11\"\nkind: alpha\n";

#[test]
fn set_value_persists_to_source_file() {
    let store = common::store();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("widget.yaml");
    let mut resolved = load(&store, &path, SEED);

    let changed = resolved.set_value(&path, "/kind", NodeValue::Str("beta".into()));
    assert_eq!(changed, Some(true));

    resolved.flush();
    assert!(resolved.write_errors().is_empty(), "unexpected write errors");

    assert_eq!(read_back(&store, &path, "/kind"), "beta");
}

#[test]
fn coalesced_burst_persists_latest_value() {
    let store = common::store();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("widget.yaml");
    let mut resolved = load(&store, &path, SEED);

    for kind in ["beta", "gamma", "alpha", "beta", "gamma"] {
        resolved.set_value(&path, "/kind", NodeValue::Str(kind.into()));
    }
    resolved.flush();

    // Whatever the coalescing did, the last value written wins.
    assert_eq!(read_back(&store, &path, "/kind"), "gamma");
}

#[test]
fn dropping_store_drains_pending_writes() {
    let store = common::store();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("widget.yaml");

    {
        let mut resolved = load(&store, &path, SEED);
        resolved.set_value(&path, "/kind", NodeValue::Str("gamma".into()));
        // No flush: dropping the store must still drain the queue.
    }

    assert_eq!(read_back(&store, &path, "/kind"), "gamma");
}

#[test]
fn edit_closure_persists_and_returns_value() {
    let store = common::store();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("widget.yaml");
    let mut resolved = load(&store, &path, SEED);

    let previous = resolved.edit(&path, |doc| {
        let node = doc.root.get_pointer_mut("/kind").unwrap();
        let old = match &node.value {
            NodeValue::Str(s) => s.clone(),
            _ => String::new(),
        };
        node.value = NodeValue::Str("beta".into());
        old
    });
    assert_eq!(previous.as_deref(), Some("alpha"));

    resolved.flush();
    assert_eq!(read_back(&store, &path, "/kind"), "beta");
}

#[test]
fn edit_unknown_source_returns_none() {
    let store = common::store();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("widget.yaml");
    let mut resolved = load(&store, &path, SEED);

    let missing = dir.path().join("does-not-exist.yaml");
    assert!(resolved
        .set_value(&missing, "/kind", NodeValue::Str("x".into()))
        .is_none());
}

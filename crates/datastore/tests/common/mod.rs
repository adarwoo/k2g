//! Shared fixtures for the integration tests.
#![allow(dead_code)]

use datastore::DataStore;

pub const ID: &str = include_str!("../fixtures/id.yaml");
pub const UNITS: &str = include_str!("../fixtures/units.yaml");
pub const GADGET: &str = include_str!("../fixtures/gadget.yaml");
pub const WIDGET: &str = include_str!("../fixtures/widget.yaml");
pub const TOOLSET: &str = include_str!("../fixtures/toolset.yaml");

/// A store compiled from all fixture schemas.
pub fn store() -> DataStore {
    DataStore::builder()
        .schema("id.yaml", ID)
        .schema("units.yaml", UNITS)
        .schema("gadget.yaml", GADGET)
        .schema("widget.yaml", WIDGET)
        .schema("toolset.yaml", TOOLSET)
        .build()
        .expect("fixtures should compile")
}

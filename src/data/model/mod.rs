pub mod job;
pub mod catalog;
pub mod profiles;
pub mod state;
pub mod stock;
pub mod tool_core;

// Flat re-exports of the shared model types. This replaces the former
// `ui::model` compatibility facade: the runtime context and the UI both import
// these from `crate::data::model`, so the model lives below the UI layer.
// (The navigation/shell types — Screen, Theme, etc. — now live in
// `crate::ui::navigation`, since they describe UI state rather than the domain.)
// The operator's display unit system is owned by the `units` crate (the single
// unit-display layer); re-exported here so runtime/UI keep one import path.
pub use units::UserUnitSystem;
pub use catalog::{CatalogStockCatalog, CatalogStockSection, CatalogStockTool};
pub use job::{CutDepthStrategy, JobConfig, ProductionOperation, Side};
pub use profiles::{
    CascadeDeleteImpact, FixtureProfile, JobProfile, MachineProfile, ToolsetGenerationPolicy,
    ToolsetProfile,
};
pub use stock::{Tool, ToolPreference, ToolStatus};

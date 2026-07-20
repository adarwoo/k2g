pub mod app_shell;
pub mod job;
pub mod catalog;
pub mod profiles;
pub mod state;
pub mod stock;
pub mod tool_core;

// Flat re-exports of the shared model types. This replaces the former
// `ui::model` compatibility facade: the runtime context and the UI both import
// these from `crate::domain`, so the model lives below the UI layer.
pub use app_shell::{
    GenerationState, JobCenterView, PersistRealm, Screen, Theme, UiLaunchData, UnitSystem,
};
pub use catalog::{CatalogStockCatalog, CatalogStockSection, CatalogStockTool};
pub use job::{CutDepthStrategy, JobConfig, ProductionOperation, Side};
pub use profiles::{
    CascadeDeleteImpact, FixtureProfile, JobProfile, MachineProfile, ToolsetGenerationPolicy,
    ToolsetProfile,
};
pub use stock::{Tool, ToolPreference, ToolStatus};

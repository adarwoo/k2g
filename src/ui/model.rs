mod ui_shell;
mod profiles;
mod stock;
mod job;

pub use ui_shell::{GenerationState, JobCenterView, Screen, Theme, UiLaunchData, UnitSystem};
pub use profiles::{
    CascadeDeleteImpact, FixtureProfile, JobProfile, MachineProfile, ToolsetGenerationPolicy,
    ToolsetProfile,
};
pub use stock::{
    CatalogStockCatalog, CatalogStockSection, CatalogStockTool, Tool, ToolPreference, ToolStatus,
};
pub use job::{
    CutDepthStrategy, JobConfig, ProductionOperation, Side,
};
pub use crate::app_state_impl::AppError;

// =============================================================================
// MODEL HIERARCHY MAP
// =============================================================================
// 1) UI shell models
//    - UiLaunchData, Screen, JobCenterView, UnitSystem, Theme, GenerationState
//
// 2) Schema-bound profile models
//    - CNC profile                -> resources/schemas/cnc.yaml
//    - Fixture profile            -> resources/schemas/fixture.yaml
//    - Machining profile          -> resources/schemas/processing.yaml
//    - Toolset profile            -> resources/schemas/toolset.yaml
//
// 3) Schema-bound stock/catalog models
//    - Tool stock                -> resources/schemas/stock.yaml
//    - Catalog index snapshot    -> resources/schemas/catalog.yaml
//
// 4) Job runtime configuration
//    - JobConfig and operation strategy enums
//
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PersistRealm {
    GlobalSettings,
    CncProfiles,
    FixtureProfiles,
    ProcessingProfiles,
    ToolsetProfiles,
    Stock,
}

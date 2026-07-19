use units::{Angle, FeedRate, Length, RotationalSpeed};

// Compatibility facade while domain ownership is moved out of UI.
pub use crate::domain::stock::{Tool, ToolPreference, ToolStatus};

/// Runtime catalog tool item shown in stock import UI.
#[allow(dead_code)]
#[derive(Clone)]
pub struct CatalogStockTool {
    pub key: String,
    pub catalog_name: String,
    pub section_name: String,
    pub display_name: String,
    pub kind: String,
    pub diameter: Length,
    pub point_angle: Angle,
    pub feed_rate: Option<FeedRate>,
    pub spindle_speed: Option<RotationalSpeed>,
    pub sku: Option<String>,
}

/// Runtime catalog section shown in stock import UI.
#[derive(Clone)]
pub struct CatalogStockSection {
    pub key: String,
    pub name: String,
    pub tools: Vec<CatalogStockTool>,
}

/// Runtime catalog index node shown in stock import UI.
#[derive(Clone)]
pub struct CatalogStockCatalog {
    pub key: String,
    pub name: String,
    pub built_in: bool,
    pub sections: Vec<CatalogStockSection>,
}

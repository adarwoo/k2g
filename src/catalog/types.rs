use serde::{Deserialize, Serialize};

use crate::units::{Angle, FeedRate, Length, RotationalSpeed};

/// Discriminates between the two supported tool categories.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolType {
    Drillbit,
    Routerbit,
}

/// Linear units supported by catalog dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinearUnit {
    #[serde(rename = "mm")]
    Mm,
    #[serde(rename = "in")]
    In,
}

impl LinearUnit {
    pub fn suffix(self) -> &'static str {
        match self {
            Self::Mm => "mm",
            Self::In => "in",
        }
    }
}

/// Feed-rate units supported by catalog machining parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FeedUnit {
    #[serde(rename = "mm_min")]
    MmMin,
    #[serde(rename = "ipm")]
    Ipm,
}

/// A single tool entry within a catalog section.
///
/// Dimensions and feed rates use native section units:
///   - `diameter`    preserved as a native `Length`
///   - `flute_len`   preserved as a native `Length`
///   - `spindle_rpm` preserved as a native `RotationalSpeed`
///   - `z_feed`      preserved as a native `FeedRate`
///   - `table_feed`  preserved as a native `FeedRate`, router bits only
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolEntry {
    #[serde(rename = "type")]
    pub tool_type: ToolType,

    /// Nominal tool diameter in native units.
    pub diameter: Length,

    /// Flute length in native units.
    pub flute_length: Option<Length>,

    /// Tool identifier (SKU or vendor/name string).
    pub sku_name: String,

    /// Tool point angle in degrees.
    ///
    /// Router bits should use `180.0` to represent a flat end.
    pub point_angle: Angle,

    /// Minimum depth to exit the hole cleanly.
    ///
    /// For drill bits, this can be automatically suggested based on
    /// point angle and diameter during editing, but catalogs must provide a value.
    pub z_min_depth: Length,

    /// Recommended spindle speed in RPM.
    pub spindle_rpm: Option<RotationalSpeed>,

    /// Recommended Z-axis (plunge) feed rate in native section units.
    pub z_feed: Option<FeedRate>,

    /// Recommended XY table feed rate in native section units — router bits only.
    pub table_feed: Option<FeedRate>,

    /// Maximum recommended hit count before tool change.
    pub max_hits: Option<u32>,

    /// Free-text note (substrate, coating, caveats, …).
    pub notes: Option<String>,
}

impl ToolEntry {
    /// Short human-readable identifier using the diameter and unit,
    /// e.g. `"0.2mm"` or `"1/8in"`.
    pub fn diameter_label(&self) -> String {
        self.diameter.to_string()
    }
}

/// A named group of tools within a catalog (e.g. a product series).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogSection {
    /// Section name (e.g. `"Series 100"`, `"Router bits"`).
    pub name: String,

    /// Optional default unit used for tool flute lengths in this section.
    pub default_flute_length_unit: Option<LinearUnit>,

    /// Optional free-text description.
    pub description: Option<String>,

    /// Tools belonging to this section.
    pub tools: Vec<ToolEntry>,
}

/// A complete tool catalog as deserialized from a YAML file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Catalog {
    /// Vendor or catalog name (e.g. `"Kyocera"`, `"Generic"`).
    pub name: String,

    /// Optional free-text description.
    pub description: Option<String>,

    /// Ordered list of tool sections.
    pub sections: Vec<CatalogSection>,
}

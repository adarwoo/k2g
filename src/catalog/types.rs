use serde::{Deserialize, Serialize};

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
///   - `diameter`    mm|in (per section `default_diameter_unit`)
///   - `flute_len`   mm|in (per section `default_flute_length_unit`)
///   - `spindle_rpm` rpm (always)
///   - `z_feed`      per section `default_feed_unit`
///   - `table_feed`  per section `default_feed_unit`, router bits only
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolEntry {
    #[serde(rename = "type")]
    pub tool_type: ToolType,

    /// Nominal tool diameter in section default units unless overridden.
    pub diameter: f64,

    /// Optional per-tool diameter unit override.
    pub diameter_unit: Option<LinearUnit>,

    /// Flute length in section default units unless overridden.
    pub flute_length: Option<f64>,

    /// Optional per-tool flute length unit override.
    pub flute_length_unit: Option<LinearUnit>,

    /// Tool identifier (SKU or vendor/name string).
    pub sku_name: String,

    /// Tool point angle in degrees.
    ///
    /// Router bits should use `180.0` to represent a flat end.
    pub point_angle: f64,

    /// Recommended spindle speed in RPM.
    pub spindle_rpm: Option<u32>,

    /// Recommended Z-axis (plunge) feed rate in native section units.
    pub z_feed: Option<f64>,

    /// Recommended XY table feed rate in native section units — router bits only.
    pub table_feed: Option<f64>,

    /// Maximum recommended hit count before tool change.
    pub max_hits: Option<u32>,

    /// Free-text note (substrate, coating, caveats, …).
    pub notes: Option<String>,
}

impl ToolEntry {
    /// Short human-readable identifier using the diameter and unit,
    /// e.g. `"0.20mm"` or `"0.0200in"`.
    pub fn diameter_label(&self, default_unit: LinearUnit) -> String {
        let unit = self.diameter_unit.unwrap_or(default_unit);
        if unit == LinearUnit::In {
            format!("{:.4}{}", self.diameter, unit.suffix())
        } else {
            format!("{:.3}{}", self.diameter, unit.suffix())
        }
    }
}

/// A named group of tools within a catalog (e.g. a product series).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogSection {
    /// Section name (e.g. `"Series 100"`, `"Router bits"`).
    pub name: String,

    /// Default unit used for tool diameters in this section.
    pub default_diameter_unit: LinearUnit,

    /// Default unit used for z/table feed rates in this section.
    pub default_feed_unit: FeedUnit,

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

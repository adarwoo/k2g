use units::{Angle, FeedRate, Length, RotationalSpeed, UserUnitDisplay, UserUnitSystem};

/// Canonical tool kinds shared across catalog and stock conversion flows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Drillbit,
    Routerbit,
    Engraver,
    Vbit,
    Endmill,
}

impl ToolKind {
    /// Label used for catalog-derived display items.
    pub fn catalog_label(self) -> &'static str {
        match self {
            Self::Drillbit => "Drill",
            Self::Routerbit => "Router",
            Self::Engraver => "Engraver",
            Self::Vbit => "V-bit",
            Self::Endmill => "Endmill",
        }
    }

    /// Label used for stock/runtime tool kind text.
    pub fn stock_label(self) -> &'static str {
        match self {
            Self::Drillbit => "Drill",
            Self::Routerbit => "Router",
            Self::Engraver => "Engraver",
            Self::Vbit => "V-Bit",
            Self::Endmill => "End Mill",
        }
    }

    pub fn as_storage_key(self) -> &'static str {
        match self {
            Self::Drillbit => "drillbit",
            Self::Routerbit => "routerbit",
            Self::Engraver => "engraver",
            Self::Vbit => "vbit",
            Self::Endmill => "endmill",
        }
    }

    pub fn from_storage_key(value: &str) -> Self {
        match value {
            "drillbit" => Self::Drillbit,
            "routerbit" => Self::Routerbit,
            "engraver" => Self::Engraver,
            "vbit" => Self::Vbit,
            "endmill" => Self::Endmill,
            _ => Self::Endmill,
        }
    }

    pub fn from_kind_label(value: &str) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "drill" | "drillbit" => Self::Drillbit,
            "router" | "routerbit" => Self::Routerbit,
            "engraver" => Self::Engraver,
            "v-bit" | "vbit" => Self::Vbit,
            _ => Self::Endmill,
        }
    }
}

/// Canonical normalized tool descriptor used by catalog/stock adapters.
#[derive(Clone)]
pub struct ToolCore {
    pub kind: ToolKind,
    pub diameter: Length,
    pub point_angle: Angle,
    pub feed_rate: Option<FeedRate>,
    pub spindle_speed: Option<RotationalSpeed>,
    pub sku: Option<String>,
}

impl ToolCore {
    pub fn display_name(&self) -> String {
        let sku_name = self.sku.clone().unwrap_or_default();
        if sku_name.trim().is_empty() {
            format!(
                "{} {}",
                self.kind.catalog_label(),
                self.diameter.unit_display(UserUnitSystem::Metric).user
            )
        } else {
            sku_name
        }
    }
}
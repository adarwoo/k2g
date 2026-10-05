use units::{Angle, FeedRate, Length, RotationalSpeed, UserUnitDisplay, UserUnitSystem};

/// Canonical tool kinds shared across catalog and stock conversion flows.
///
/// `Milling` is the family shown to the operator as "Milling" — stock's small,
/// precision bits for isolation-scale copper work (the isolation V-bit's own
/// siblings), as opposed to `Endmill`/`Routerbit`, which cut through the board itself
/// and belong to the router family (see `is_router_tool`/`is_engraver_tool` in
/// `runtime::tooling`). The variant was named `Engraver` before the family was
/// renamed to "Milling"; **the on-disk `kind` string stays `"engraver"`** (see
/// `as_storage_key`/`from_storage_key`/`from_kind_label` below) so every stock.yaml
/// already on an operator's disk keeps loading unchanged — this is a display rename,
/// not a schema change, and doesn't need the version bump or migration a real wire
/// format change would.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Drillbit,
    Routerbit,
    Milling,
    Vbit,
    Endmill,
}

impl ToolKind {
    /// Label used for catalog-derived display items.
    pub fn catalog_label(self) -> &'static str {
        match self {
            Self::Drillbit => "Drill",
            Self::Routerbit => "Router",
            Self::Milling => "Milling",
            Self::Vbit => "V-bit",
            Self::Endmill => "Endmill",
        }
    }

    /// Label used for stock/runtime tool kind text.
    pub fn stock_label(self) -> &'static str {
        match self {
            Self::Drillbit => "Drill",
            Self::Routerbit => "Router",
            Self::Milling => "Milling",
            Self::Vbit => "V-Bit",
            Self::Endmill => "End Mill",
        }
    }

    /// The wire value stored in `kind`, unchanged as `"engraver"` — see the type doc.
    pub fn as_storage_key(self) -> &'static str {
        match self {
            Self::Drillbit => "drillbit",
            Self::Routerbit => "routerbit",
            Self::Milling => "engraver",
            Self::Vbit => "vbit",
            Self::Endmill => "endmill",
        }
    }

    pub fn from_storage_key(value: &str) -> Self {
        match value {
            "drillbit" => Self::Drillbit,
            "routerbit" => Self::Routerbit,
            "engraver" => Self::Milling,
            "vbit" => Self::Vbit,
            "endmill" => Self::Endmill,
            _ => Self::Endmill,
        }
    }

    pub fn from_kind_label(value: &str) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "drill" | "drillbit" => Self::Drillbit,
            "router" | "routerbit" => Self::Routerbit,
            "engraver" | "milling" => Self::Milling,
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
    /// The **lateral** cutting feed: what a G1 in XY runs at.
    pub table_feed: Option<FeedRate>,
    /// The **plunge** feed: what a G1 in Z alone runs at.
    ///
    /// A separate rating, not a fraction of [`Self::table_feed`]. A straight plunge
    /// engages the tool's weak end-cutting geometry over its full diameter at once, and
    /// how much slower that has to be is a property of the tool, which the catalogue
    /// states. `None` only when the catalogue is silent.
    pub z_feed: Option<FeedRate>,
    pub spindle_speed: Option<RotationalSpeed>,
    pub sku: Option<String>,
    /// The shallowest cut the tool is rated to hold.
    pub z_min_depth: Option<Length>,
    /// Usable cutting length, for the check that a cutter can reach through the board.
    pub flute_length: Option<Length>,
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
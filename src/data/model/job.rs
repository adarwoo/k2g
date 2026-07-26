use serde_json::{Map, Value};
use units::Length;

/// Which stitched outline a retaining tab holds.
///
/// A board's own boundary is the common case; an interior cutout needs tabs too once
/// the slug it frees is big enough to move under the cutter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TabContour {
    #[default]
    Outer,
    Cutout,
}

impl TabContour {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Outer => "outer",
            Self::Cutout => "cutout",
        }
    }

    /// Parses the schema key, defaulting to the board boundary for anything
    /// unrecognised — a tab in the wrong place is recoverable, a lost tab is not.
    pub fn from_key(key: &str) -> Self {
        match key {
            "cutout" => Self::Cutout,
            _ => Self::Outer,
        }
    }
}

/// One retaining tab's placement on the board outline (`job.yaml#/edge_tabs`).
///
/// Stored as a position **along** a contour rather than as an XY point: `at` is the
/// fraction of that contour's length from its start, so moving the board in the KiCad
/// layout leaves every tab where the operator put it. Width and retention style are not
/// here — those are profile policy, reusable across boards.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeTab {
    pub contour: TabContour,
    /// Which contour of that kind, in the stitcher's order.
    pub index: usize,
    /// Fraction along the contour, in `[0, 1)`.
    pub at: f64,
}

impl EdgeTab {
    /// Reads one `edge_tabs` entry. `at` is required (a tab with no position is not a
    /// tab); the rest fall back to the schema defaults.
    pub fn from_value(value: &Value) -> Option<Self> {
        let at = value.get("at").and_then(Value::as_f64)?;
        Some(Self {
            contour: value
                .get("contour")
                .and_then(Value::as_str)
                .map(TabContour::from_key)
                .unwrap_or_default(),
            index: value.get("index").and_then(Value::as_u64).unwrap_or(0) as usize,
            // The contour is a loop, so a position is only ever meaningful modulo 1.
            at: at.rem_euclid(1.0),
        })
    }

    pub fn to_value(&self) -> Value {
        let mut obj = Map::new();
        obj.insert("contour".into(), Value::from(self.contour.as_str()));
        obj.insert("index".into(), Value::from(self.index as u64));
        obj.insert("at".into(), Value::from(self.at.rem_euclid(1.0)));
        Value::Object(obj)
    }
}

/// Job side selection.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Top,
    Bottom,
}

impl Side {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Bottom => "bottom",
        }
    }
}

/// Supported production operations in a job.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ProductionOperation {
    DrillLocatingPins,
    DrillPth,
    DrillNpth,
    RouteBoard,
    MillBoard,
}

impl ProductionOperation {
    pub fn all() -> [Self; 5] {
        [
            Self::DrillLocatingPins,
            Self::DrillPth,
            Self::DrillNpth,
            Self::RouteBoard,
            Self::MillBoard,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::DrillLocatingPins => "Drill Locating Pins",
            Self::DrillPth => "Drill Plated Through Holes (PTH)",
            Self::DrillNpth => "Drill Non-Plated Through Holes (NPTH)",
            Self::RouteBoard => "Route Board Outline",
            Self::MillBoard => "Mill Board Outline",
        }
    }
}

/// Aggregate job configuration used by the job screen and generation context.
#[allow(dead_code)]
#[derive(Clone)]
pub struct JobConfig {
    pub selected_operations: Vec<ProductionOperation>,
    pub rotation_angle: i32,
    pub tab_count: u8,
    pub tab_width: Length,
    pub tab_width_baseline: Length,
    pub allow_routing_holes: bool,
    pub drill_then_route: bool,
    pub pilot_hole_fallback: bool,
    pub outline_router_tool_id: Option<String>,
    pub mouse_bites_enabled: bool,
    pub mouse_bite_pitch: Length,
    pub mouse_bite_drill_tool_id: Option<String>,
}

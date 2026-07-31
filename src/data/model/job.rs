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

/// One retaining tab's **nudge** away from where the application put it
/// (`job.yaml#/edge_tabs`).
///
/// Positions are computed, not stored — the profile says how many tabs, and
/// `crate::gcode::outline::distribute_tabs` shares them over the outline's straight
/// sides. What the job records is the operator's disagreement with that: a signed
/// distance along the contour. Storing the difference is what lets a tab survive the
/// board changing shape, since the computed home moves with the geometry and the nudge
/// still means the same thing.
///
/// Width and retention style are not here — those are profile policy, reusable across
/// boards.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeTab {
    pub contour: TabContour,
    /// Which contour of that kind, in the stitcher's order.
    pub index: usize,
    /// Which computed tab on that contour, in contour order.
    pub tab: usize,
    /// How far along the contour to move it, signed; positive runs with the traversal.
    pub offset: Length,
}

impl EdgeTab {
    /// Reads one `edge_tabs` entry. `tab` and `offset` are required — an entry naming
    /// neither a tab nor a displacement records nothing. The rest fall back to the
    /// schema defaults.
    pub fn from_value(value: &Value) -> Option<Self> {
        let tab = value.get("tab").and_then(Value::as_u64)? as usize;
        let offset = value.get("offset").and_then(Value::as_str)?;
        Some(Self {
            contour: value
                .get("contour")
                .and_then(Value::as_str)
                .map(TabContour::from_key)
                .unwrap_or_default(),
            index: value.get("index").and_then(Value::as_u64).unwrap_or(0) as usize,
            tab,
            offset: Length::from_string(offset, Some(units::LengthUnit::Mm)).ok()?,
        })
    }

    pub fn to_value(self) -> Value {
        let mut obj = Map::new();
        obj.insert("contour".into(), Value::from(self.contour.as_str()));
        obj.insert("index".into(), Value::from(self.index as u64));
        obj.insert("tab".into(), Value::from(self.tab as u64));
        // Authored in millimetres: the stored form is canonical, and the UI converts
        // for display like every other length.
        obj.insert("offset".into(), Value::from(format!("{}mm", self.offset.as_mm())));
        Value::Object(obj)
    }
}

/// Which face of the PCB a job's first step cuts.
///
/// `Front` is the component side, `Back` the solder side — named for the **board**, not
/// for the machine. "Top" and "bottom" meant both at once, and they part company the
/// moment a step machines the back: the board is turned over, so the PCB's bottom is what
/// faces up. The bed keeps its own words (`near`/`far`, `left`/`right`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BoardFace {
    Front,
    Back,
}

impl BoardFace {
    /// The schema's own word for it (`machining.yaml#/$defs/step/board_face`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Front => "front",
            Self::Back => "back",
        }
    }

    /// How it is named to the operator.
    pub fn label(self) -> &'static str {
        match self {
            Self::Front => "Front",
            Self::Back => "Back",
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
    /// In the same "most likely first" order the UI and `operation_key` use.
    pub fn all() -> [Self; 5] {
        [
            Self::DrillPth,
            Self::DrillNpth,
            Self::RouteBoard,
            Self::DrillLocatingPins,
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

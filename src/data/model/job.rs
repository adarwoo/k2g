use units::Length;

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

/// Cut depth strategy for board machining.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CutDepthStrategy {
    Automatic,
    SinglePass,
    MultiPass,
}

impl CutDepthStrategy {
    #[allow(dead_code)]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Automatic => "automatic",
            Self::SinglePass => "single_pass",
            Self::MultiPass => "multi_pass",
        }
    }

    #[allow(dead_code)]
    pub fn label(self) -> &'static str {
        match self {
            Self::Automatic => "Automatic (recommended)",
            Self::SinglePass => "Single Pass",
            Self::MultiPass => "Multi-pass",
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

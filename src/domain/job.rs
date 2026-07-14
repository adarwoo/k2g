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

/// Rotation strategy used by job setup.
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RotationMode {
    Auto,
    Manual,
}

impl RotationMode {
    #[allow(dead_code)]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Manual => "manual",
        }
    }
}

/// Rack strategy used for ATC generation.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AtcRackStrategy {
    Off,
    Reuse,
    Overwrite,
}

impl AtcRackStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Reuse => "reuse",
            Self::Overwrite => "overwrite",
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

/// Board thickness sourcing mode.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BoardThicknessMode {
    Automatic,
    Preset,
    UserDefined,
    Probe,
}

impl BoardThicknessMode {
    #[allow(dead_code)]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Automatic => "automatic",
            Self::Preset => "preset",
            Self::UserDefined => "user_defined",
            Self::Probe => "probe",
        }
    }
}

/// Z0 reference mode.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Z0DeterminationMode {
    ManualAdjustZ0,
    TouchProbe,
}

impl Z0DeterminationMode {
    #[allow(dead_code)]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ManualAdjustZ0 => "manual_adjust_z0",
            Self::TouchProbe => "touch_probe",
        }
    }
}

/// Touch probe source mode.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TouchProbeSource {
    ManualInstallation,
    AtcSlot,
}

impl TouchProbeSource {
    #[allow(dead_code)]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ManualInstallation => "manual_installation",
            Self::AtcSlot => "atc_slot",
        }
    }
}

/// Board orientation options.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BoardOrientation {
    Automatic,
    NoRotation,
    Rotate90,
    Rotate180,
    Rotate270,
    RotateCustom,
}

impl BoardOrientation {
    #[allow(dead_code)]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Automatic => "automatic",
            Self::NoRotation => "no_rotation",
            Self::Rotate90 => "rotate_90",
            Self::Rotate180 => "rotate_180",
            Self::Rotate270 => "rotate_270",
            Self::RotateCustom => "rotate_custom",
        }
    }
}

/// Aggregate job configuration used by the job screen and generation context.
#[allow(dead_code)]
#[derive(Clone)]
pub struct JobConfig {
    pub selected_operations: Vec<ProductionOperation>,
    pub side: Side,
    pub rotation_mode: RotationMode,
    pub rotation_angle: i32,
    pub atc_strategy: AtcRackStrategy,
    pub tab_count: u8,
    pub tab_width_mm: f32,
    pub tab_width_baseline_mm: f32,
    pub allow_routing_holes: bool,
    pub drill_then_route: bool,
    pub pilot_hole_fallback: bool,
    pub cut_depth_strategy: CutDepthStrategy,
    pub multi_pass_max_depth_mm: f32,
    pub outline_router_tool_id: Option<String>,
    pub board_thickness_mode: BoardThicknessMode,
    pub board_thickness_preset_mm: f32,
    pub board_thickness_user_value: f32,
    pub z0_determination_mode: Z0DeterminationMode,
    pub touch_probe_source: TouchProbeSource,
    pub touch_probe_atc_slot: u8,
    pub mouse_bites_enabled: bool,
    pub mouse_bite_pitch_mm: f32,
    pub mouse_bite_drill_tool_id: Option<String>,
    pub board_orientation: BoardOrientation,
    pub board_orientation_custom_degrees: f32,
}

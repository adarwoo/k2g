//! App-shell model types shared by the runtime context and the UI: top-level
//! screen navigation, the Job screen's sub-views, user unit/theme preferences,
//! generation status, the UI launch payload, and the persistence realm marker.
//! (Relocated out of the former `ui::model` facade — these belong below the UI.)

use pcb::BoardSnapshot;
use units::UserUnitSystem;

/// Boot payload received when launching the UI layer.
#[derive(Clone, PartialEq)]
pub struct UiLaunchData {
    pub env_vars: Vec<(String, String)>,
    pub cli_args: Vec<String>,
    pub kicad_status: String,
    pub board_snapshot: Option<BoardSnapshot>,
    pub save_filename_override: Option<String>,
}

/// Top-level screens available in the application shell.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Job,
    CncProfiles,
    FixtureProfiles,
    MachiningProfiles,
    ToolsetProfiles,
    Stock,
    Catalog,
}

impl Screen {
    pub fn label(self) -> &'static str {
        match self {
            Self::Job => "Job",
            Self::CncProfiles => "CNC",
            Self::FixtureProfiles => "Fixtures",
            Self::MachiningProfiles => "Machining",
            Self::ToolsetProfiles => "Toolset",
            Self::Stock => "Stock",
            Self::Catalog => "Catalog",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Job => "job",
            Self::CncProfiles => "cnc-profiles",
            Self::FixtureProfiles => "fixture-profiles",
            Self::MachiningProfiles => "machining-profiles",
            Self::ToolsetProfiles => "toolset-profiles",
            Self::Stock => "stock",
            Self::Catalog => "catalog",
        }
    }
}

/// Sub-views inside the Job screen.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum JobCenterView {
    Board,
    Machining,
    Code,
    Rack,
}

impl JobCenterView {
    pub fn label(self) -> &'static str {
        match self {
            Self::Board => "Board",
            Self::Machining => "Machining",
            Self::Code => "Code",
            Self::Rack => "Rack",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Board => "board",
            Self::Machining => "machining",
            Self::Code => "code",
            Self::Rack => "rack",
        }
    }
}

/// User-facing measurement display system.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UnitSystem {
    Metric,
    Imperial,
    Mil,
}

impl UnitSystem {
    #[allow(dead_code)]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Metric => "metric",
            Self::Imperial => "imperial",
            Self::Mil => "mil",
        }
    }

    #[allow(dead_code)]
    pub fn user_unit_system(self) -> UserUnitSystem {
        match self {
            Self::Metric => UserUnitSystem::Metric,
            Self::Imperial | Self::Mil => UserUnitSystem::Imperial,
        }
    }

    #[allow(dead_code)]
    pub fn length_unit_label(self) -> &'static str {
        match self {
            Self::Metric => "mm",
            Self::Imperial => "\"",
            Self::Mil => "mil",
        }
    }

    #[allow(dead_code)]
    pub fn feed_unit_label(self) -> &'static str {
        match self {
            Self::Metric => "mm/min",
            Self::Imperial | Self::Mil => "in/min",
        }
    }
}

/// Application visual theme.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Light,
    Dark,
}

impl Theme {
    #[allow(dead_code)]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    #[allow(dead_code)]
    pub fn from_str(value: &str) -> Self {
        match value {
            "light" => Self::Light,
            _ => Self::Dark,
        }
    }
}

/// GCode generation status for UI feedback.
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GenerationState {
    Idle,
    Generating,
    Failed,
}

/// A persistable realm — the dispatch tag the legacy context uses to mirror a
/// mutation down into the AppData datastore (the sole writer). Global settings and
/// stock are the only realms still driven through this legacy funnel; the profile
/// realms are edited on AppData directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PersistRealm {
    GlobalSettings,
    Stock,
}

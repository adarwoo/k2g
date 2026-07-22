//! UI navigation and shell state: top-level screen selection, the Job screen's
//! sub-views, the visual theme, the launch payload, the generation status, and
//! the persistence-realm dispatch marker. These describe *where the user is* and
//! how the shell is framed, so they live under the UI layer.
//!
//! Note that `GenerationState` and `PersistRealm` are not navigation as such —
//! they ride along here as small shell-level status/dispatch markers that the
//! runtime keeps on `AppState` alongside the navigation fields.

use pcb::BoardSnapshot;

/// Boot payload received when launching the UI layer.
#[derive(Clone, PartialEq)]
pub struct UiLaunchData {
    /// A short, clean KiCad connection status for display (no raw error dump).
    pub kicad_status: String,
    /// The board collected at startup (the reachable KiCad's open PCB), if any.
    pub board_snapshot: Option<BoardSnapshot>,
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

/// Application visual theme.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Light,
    Dark,
}

impl Theme {
    #[allow(dead_code)]
    pub fn from_str(value: &str) -> Self {
        match value {
            "light" => Self::Light,
            _ => Self::Dark,
        }
    }
}

/// GCode generation status for UI feedback (see `docs/gcode-generation.md` §8).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GenerationState {
    /// Nothing running; the last program (if any) is current.
    Idle,
    /// The worker is generating; the displayed program is stale/greyed.
    Running,
    /// The last run failed; outputs are cleared and diagnostics surfaced.
    Failed,
}

/// A persistable realm — the dispatch tag the legacy context uses to mirror a
/// mutation down into the AppData datastore (the sole writer). Global settings and
/// stock are the only realms still driven through this legacy funnel; the profile
/// realms are edited on AppData directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PersistRealm {
    GlobalSettings,
}

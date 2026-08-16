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
    /// Its outer copper, read on the same connection.
    pub copper: crate::runtime::BoardCopper,
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
    Manual,
    Logs,
    About,
}

impl Screen {
    /// Every screen, in rail order.
    ///
    /// Exists so [`Self::from_key`] can be written once and proved total by a test,
    /// rather than as a second `match` that has to be read beside [`Self::key`] to see
    /// whether the two still agree.
    pub const ALL: [Self; 10] = [
        Self::Job,
        Self::CncProfiles,
        Self::FixtureProfiles,
        Self::MachiningProfiles,
        Self::ToolsetProfiles,
        Self::Stock,
        Self::Catalog,
        Self::Manual,
        Self::Logs,
        Self::About,
    ];

    /// The inverse of [`Self::key`], for the screen the last session was left on.
    ///
    /// `None` for a key this build does not know — a settings file written by a newer
    /// version, or edited by hand. The caller supplies the fallback, because "open on
    /// the Job screen" is a decision about launch rather than about decoding.
    pub fn from_key(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|screen| screen.key() == value)
    }

    /// Whether the docked Job view may appear beside this screen.
    ///
    /// The profile and inventory screens only: those are what feed the plan, so
    /// seeing the Code/Tooling/Rack result react while editing them is the point.
    /// `Job` already *is* the view, and `Manual`/`Logs`/`About` change nothing it shows.
    pub fn shows_pinned_job(self) -> bool {
        matches!(
            self,
            Self::CncProfiles
                | Self::FixtureProfiles
                | Self::MachiningProfiles
                | Self::ToolsetProfiles
                | Self::Stock
                | Self::Catalog
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Job => "Job",
            Self::CncProfiles => "CNC",
            Self::FixtureProfiles => "Fixtures",
            Self::MachiningProfiles => "Machining",
            Self::ToolsetProfiles => "Toolset",
            Self::Stock => "Stock",
            Self::Catalog => "Catalog",
            Self::Manual => "Manual",
            Self::Logs => "Logs",
            Self::About => "About",
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
            Self::Manual => "manual",
            Self::Logs => "logs",
            Self::About => "about",
        }
    }
}

/// Sub-views inside the Job screen.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum JobCenterView {
    Board,
    Machining,
    Code,
    Tooling,
    Rack,
}

impl JobCenterView {
    /// Every tab, in tab-bar order. See [`Screen::ALL`].
    pub const ALL: [Self; 5] = [
        Self::Board,
        Self::Machining,
        Self::Code,
        Self::Tooling,
        Self::Rack,
    ];

    /// The inverse of [`Self::key`]. See [`Screen::from_key`].
    pub fn from_key(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|view| view.key() == value)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Board => "Board",
            Self::Machining => "Machining",
            Self::Code => "Code",
            Self::Tooling => "Tooling",
            Self::Rack => "Rack",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Board => "board",
            Self::Machining => "machining",
            Self::Code => "code",
            Self::Tooling => "tooling",
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

#[cfg(test)]
mod key_codec_tests {
    use super::*;

    /// The keys are what the settings file stores, so a variant whose key does not come
    /// back reopens the application somewhere the user never was. Neither enum derives
    /// `Debug`, so failures name the key.
    #[test]
    fn every_screen_and_job_view_key_round_trips() {
        for screen in Screen::ALL {
            assert!(
                Screen::from_key(screen.key()) == Some(screen),
                "Screen::from_key did not return the screen that produced {:?}",
                screen.key()
            );
        }
        for view in JobCenterView::ALL {
            assert!(
                JobCenterView::from_key(view.key()) == Some(view),
                "JobCenterView::from_key did not return the view that produced {:?}",
                view.key()
            );
        }

        assert!(
            Screen::from_key("teleporter").is_none(),
            "an unknown key must decline rather than guess — the caller opens on Job"
        );
        assert!(JobCenterView::from_key("").is_none());
    }

    /// The schema's `enum` and these keys must be the same set. If a screen is added
    /// without extending the schema, every settings write made from that screen fails
    /// validation — and `persist_global_settings` only *logs* that, so the first symptom
    /// would be a user's settings quietly ceasing to save.
    #[test]
    fn the_schema_enumerates_exactly_the_keys_the_codecs_produce() {
        let schema: serde_yaml::Value =
            serde_yaml::from_str(crate::data::settings_schema_text()).expect("schema parses");

        let listed = |property: &str| -> Vec<String> {
            schema["properties"][property]["enum"]
                .as_sequence()
                .unwrap_or_else(|| panic!("{property} declares an enum"))
                .iter()
                .map(|value| value.as_str().expect("enum entries are strings").to_string())
                .collect()
        };

        let mut schema_screens = listed("selected_screen");
        let mut code_screens: Vec<String> =
            Screen::ALL.iter().map(|s| s.key().to_string()).collect();
        schema_screens.sort();
        code_screens.sort();
        assert_eq!(
            schema_screens, code_screens,
            "schemas/settings.yaml#/properties/selected_screen/enum has drifted from Screen::key"
        );

        let mut schema_views = listed("selected_job_view");
        let mut code_views: Vec<String> = JobCenterView::ALL
            .iter()
            .map(|v| v.key().to_string())
            .collect();
        schema_views.sort();
        code_views.sort();
        assert_eq!(
            schema_views, code_views,
            "schemas/settings.yaml#/properties/selected_job_view/enum has drifted from \
             JobCenterView::key"
        );
    }
}

use dioxus::prelude::*;

use super::profile_manager::{FieldGroup, ProfileManager};
use super::profiles_common::format_impact_warning;
use crate::data::Profile;
use crate::ui::bindings::{data_revision, refresh_legacy_fixtures};

/// Fixture profile screen — a thin wrapper over the shared [`ProfileManager`].
/// Supplies the fixture field layout and a transitional delete guard that blocks
/// removal while a legacy machining profile still references the fixture (the
/// machining screen has not been migrated to the datastore yet).
#[component]
pub fn FixtureProfilesScreen(state: Signal<crate::runtime::AppCtx>) -> Element {
    // Keep the legacy `fixtures` projection in sync with AppData on every store
    // mutation while this screen is mounted, then push a fresh snapshot so sibling
    // screens (and the current-job reference check) observe the same fixtures.
    // Without this a fixture created here never reaches the runtime, so a machining
    // profile referencing it is wrongly flagged as a broken reference.
    use_effect(move || {
        let _ = data_revision();
        refresh_legacy_fixtures();
        state.set(crate::runtime::ctx_snapshot());
    });

    let delete_guard = use_callback(move |id: String| {
        let impact = state.read().impact_delete_fixture_profile(&id);
        if impact.dependent_process_profiles.is_empty() {
            None
        } else {
            Some(format_impact_warning(
                "Cannot delete fixture profile because it is referenced by machining profiles:",
                &impact,
            ))
        }
    });

    rsx! {
        ProfileManager {
            kind: Profile::Fixture,
            type_label: "Fixture".to_string(),
            file_kind: "fixture-profile".to_string(),
            groups: FieldGroup::flat(&[
                "/name",
                "/board_holding_method",
                "/origin/x0",
                "/origin/y0",
                "/backboard_thickness",
                "/bed_clearance",
                "/breakthrough",
                "/z_retract",
                "/z_safe",
            ]),
            templates: Vec::new(),
            delete_guard: Some(delete_guard),
        }
    }
}

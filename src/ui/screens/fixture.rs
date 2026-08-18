use dioxus::prelude::*;

use super::profile_manager::{FieldGroup, ProfileManager};
use super::profiles_common::format_impact_warning;
use crate::data::Profile;

/// Fixture profile screen — a thin wrapper over the shared [`ProfileManager`].
/// Supplies the fixture field layout and a transitional delete guard that blocks
/// removal while a legacy machining profile still references the fixture (the
/// machining screen has not been migrated to the datastore yet).
///
/// The legacy `fixtures` projection is mirrored from AppData by the root's single bridge
/// ([`crate::ui::bindings::refresh_legacy_projections`]), not from here.
#[component]
pub fn FixtureProfilesScreen(state: Signal<crate::runtime::AppCtx>) -> Element {
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
                // Beside `origin` because both answer "where is zero" — that one for
                // the board's corner, this one for which of the machine's stored zeros
                // the fixture sits in.
                "/origin_reference",
                // Which axis a bottom-side step mirrors about, and where the locating
                // pins go. Only the operator knows it — it follows from where the
                // registration physically is — and getting it wrong mirrors the solder
                // side along the wrong axis.
                "/board_flip_axis",
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

use dioxus::prelude::*;

use super::profile_manager::{FieldGroup, ProfileManager};
use super::profiles_common::format_impact_warning;
use crate::data::Profile;
use crate::ui::data_bind::{data_revision, refresh_legacy_cnc, use_cnc_templates};

/// CNC profile screen — a thin wrapper over the shared [`ProfileManager`].
///
/// CNC profiles are owned by the `AppData` datastore (`crate::data`). Because the
/// legacy GCode generator, the setup screen, and the active machine selection
/// still read the in-memory `machines` list, this wrapper mirrors every AppData
/// change back into that legacy projection (see [`refresh_legacy_cnc`]) so a
/// session stays coherent. The delete guard blocks removal while a legacy
/// machining profile still references the CNC profile (machining is not migrated
/// to the datastore yet).
#[component]
pub fn CncScreen(state: Signal<crate::app_state_impl::AppCtx>) -> Element {
    // Keep the legacy `machines` projection in sync with AppData on every store
    // mutation while this screen is mounted, then refresh the legacy snapshot so
    // sibling screens observe the same machines. The effect re-runs whenever the
    // store revision changes; the follow-up `state.set` does not (it only writes),
    // so there is no feedback loop.
    use_effect(move || {
        let _ = data_revision();
        refresh_legacy_cnc();
        state.set(crate::app_state_impl::ctx_snapshot());
    });

    let templates = use_cnc_templates();

    let delete_guard = use_callback(move |id: String| {
        let impact = state.read().impact_delete_cnc_profile(&id);
        if impact.dependent_process_profiles.is_empty() {
            None
        } else {
            Some(format_impact_warning(
                "Cannot delete CNC profile because it is referenced by machining profiles:",
                &impact,
            ))
        }
    });

    rsx! {
        ProfileManager {
            kind: Profile::Cnc,
            type_label: "CNC".to_string(),
            file_kind: "cnc-profile".to_string(),
            groups: cnc_field_groups(),
            templates,
            delete_guard: Some(delete_guard),
        }
    }
}

/// The CNC detail-editor layout: schema field pointers grouped into sections that
/// mirror the shape of `cnc.yaml` (machine parameters first, then the RHAI
/// primitive templates). Each pointer is rendered by a `SchemaField`, so widgets,
/// labels, units, and validation all come from the schema.
fn cnc_field_groups() -> Vec<FieldGroup> {
    let group = |title: &str, fields: &[&str]| FieldGroup {
        title: title.to_string(),
        fields: fields.iter().map(|f| f.to_string()).collect(),
    };

    vec![
        group("", &["/name", "/machine/atc_slot_count"]),
        group(
            "Feed & spindle",
            &[
                "/machine/max_feed_rate",
                "/machine/spindle_rpm_min",
                "/machine/spindle_rpm_max",
            ],
        ),
        group("Axis scaling", &["/machine/scaling/x", "/machine/scaling/y"]),
        group("Line numbering", &["/machine/line_numbering_increment"]),
        group(
            "Program lifecycle",
            &["/primitives/initialise", "/primitives/conclude"],
        ),
        group(
            "Motion / spindle / drilling",
            &[
                "/primitives/rapid_move",
                "/primitives/linear_cut",
                "/primitives/start_spindle",
                "/primitives/stop_spindle",
                "/primitives/drill",
                "/primitives/peck_drill",
            ],
        ),
        group(
            "Arc / bezier & optional",
            &[
                "/primitives/cut_arc",
                "/primitives/cut_bezier",
                "/primitives/pause",
                "/primitives/banner",
            ],
        ),
        group("Tool change", &["/primitives/change_tool"]),
        group(
            "Unit switching",
            &["/primitives/use_metric", "/primitives/use_imperial"],
        ),
    ]
}

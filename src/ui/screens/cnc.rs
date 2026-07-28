use dioxus::prelude::*;

use super::profile_manager::{FieldGroup, ProfileManager};
use super::profiles_common::format_impact_warning;
use crate::data::Profile;
use crate::ui::bindings::{data_revision, refresh_legacy_cnc, use_cnc_templates};

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
pub fn CncScreen(state: Signal<crate::runtime::AppCtx>) -> Element {
    // Keep the legacy `machines` projection in sync with AppData on every store
    // mutation while this screen is mounted, then refresh the legacy snapshot so
    // sibling screens observe the same machines. The effect re-runs whenever the
    // store revision changes; the follow-up `state.set` does not (it only writes),
    // so there is no feedback loop.
    use_effect(move || {
        let _ = data_revision();
        refresh_legacy_cnc();
        state.set(crate::runtime::ctx_snapshot());
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
            help: Some(crate::ui::help::GTL),
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
        group(
            "",
            &[
                "/name",
                "/machine/output_file_extension",
                "/machine/atc_slot_count",
            ],
        ),
        group(
            "Spindle",
            &["/machine/spindle_rpm_min", "/machine/spindle_rpm_max"],
        ),
        // Axis feed ceilings sit beside the spindle range because the two are
        // solved together: a tool rated faster than the machine can feed is run at
        // a lower RPM to preserve its chip load (see `gcode::feeds::resolve`).
        group(
            "Feed limits",
            &["/machine/max_feed_xy", "/machine/max_feed_z"],
        ),
        group("Axis scaling", &["/machine/scaling/x", "/machine/scaling/y"]),
        group(
            "Zeroing & tool length",
            &[
                "/machine/has_repeatable_home",
                "/machine/work_coordinate_systems",
                "/machine/tool_length_measurement",
            ],
        ),
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
            ],
        ),
        group(
            "Arc / bezier & optional",
            &[
                "/primitives/cut_arc",
                "/primitives/cut_bezier",
                "/primitives/pause",
                "/primitives/banner",
                "/primitives/line_number",
            ],
        ),
        group("Tool change", &["/primitives/change_tool"]),
    ]
}

#[cfg(test)]
mod tests {
    use super::cnc_field_groups;

    /// Every field the schema requires must be reachable in the editor.
    ///
    /// The group list is a hand-written allow-list of pointers, so adding a
    /// property to `cnc.yaml` does *not* make it editable — `max_feed_xy`,
    /// `max_feed_z`, and `output_file_extension` were all required by the schema
    /// and invisible in the UI until this test existed. A defaulted field is the
    /// easiest kind to lose this way: the backfill keeps profiles loading, so
    /// nothing complains and the operator simply cannot reach the setting.
    #[test]
    fn every_required_field_is_editable() {
        const CNC_SCHEMA: &str = include_str!("../../../schemas/cnc.yaml");
        let schema: serde_yaml::Value = serde_yaml::from_str(CNC_SCHEMA).expect("cnc.yaml parses");

        let pointers: Vec<String> = cnc_field_groups()
            .into_iter()
            .flat_map(|g| g.fields)
            .collect();

        for section in ["machine", "primitives"] {
            let required = schema["properties"][section]["required"]
                .as_sequence()
                .unwrap_or_else(|| panic!("{section} declares required fields"));

            for field in required {
                let name = field.as_str().expect("required entries are strings");
                // A nested object (`scaling`) is covered by its leaf pointers, so
                // match on the prefix rather than on equality.
                let prefix = format!("/{section}/{name}");
                assert!(
                    pointers
                        .iter()
                        .any(|p| p == &prefix || p.starts_with(&format!("{prefix}/"))),
                    "cnc.yaml requires {section}.{name} but the CNC editor has no field for it"
                );
            }
        }
    }
}

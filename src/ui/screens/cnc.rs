use dioxus::prelude::*;

use super::profile_manager::{FieldGroup, ProfileManager};
use super::profiles_common::format_impact_warning;
use crate::data::Profile;
use crate::ui::bindings::use_cnc_templates;

/// CNC profile screen — a thin wrapper over the shared [`ProfileManager`].
///
/// CNC profiles are owned by the `AppData` datastore (`crate::data`). The legacy GCode
/// generator, the setup screen and the active machine selection still read the in-memory
/// `machines` list; it is mirrored from AppData by the root's single bridge
/// ([`crate::ui::bindings::refresh_legacy_projections`]) rather than by this screen, so
/// an edit made here reaches the Job views whether or not this screen is on show. The
/// delete guard blocks removal while a legacy machining profile still references the CNC
/// profile (machining is not migrated to the datastore yet).
#[component]
pub fn CncScreen(state: Signal<crate::runtime::AppCtx>) -> Element {
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
            &["/machine/has_repeatable_home", "/machine/tool_length_measurement"],
        ),
    ]
    .into_iter()
    .chain(primitive_groups())
    .collect()
}

/// The primitive sections, built from the schema's own `x-category` rather than from a
/// hand-written list.
///
/// The list used to be hardcoded here in groups like "Arc / bezier & optional", which had
/// two costs: adding a primitive to `cnc.yaml` did not make it editable until someone
/// remembered to add it here too, and the groupings mixed things the application emits with
/// things a template has to call — so the editor implied that filling in `set_origin` was
/// enough to select an origin, when nothing happens unless `program_begin` calls it. The
/// per-field *kind* badge is what actually says which is which; see
/// [`PrimitiveCategory`](crate::gcode::primitive_vars::PrimitiveCategory).
fn primitive_groups() -> Vec<FieldGroup> {
    use crate::gcode::primitive_vars::{primitives_in, PrimitiveCategory};

    PrimitiveCategory::ORDER
        .iter()
        .filter_map(|category| {
            let fields: Vec<String> = primitives_in(*category)
                .into_iter()
                .map(|p| format!("/primitives/{}", p.name))
                .collect();
            // A category the schema declares nothing for is simply not shown, rather than
            // rendered as an empty heading.
            (!fields.is_empty())
                .then(|| FieldGroup { title: category.title().to_string(), fields })
        })
        .collect()
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

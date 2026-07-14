use dioxus::prelude::*;
use std::collections::BTreeSet;

use crate::units::{FeedRate, Length, RotationalSpeed};
use crate::ui::unit_service;
use crate::ctx::sync_ctx_from_ui_state_and_persist_realms;

use super::super::model::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum StockSortMode {
    RecentFirst,
    Type,
}

impl StockSortMode {
    fn from_value(value: &str) -> Self {
        match value {
            "type" => Self::Type,
            _ => Self::RecentFirst,
        }
    }

    fn value(self) -> &'static str {
        match self {
            Self::RecentFirst => "recent",
            Self::Type => "type",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StockTypeFilter {
    All,
    Drill,
    Router,
    VBit,
    Engraving,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StockDetailField {
    Diameter,
    PointAngle,
    FeedRate,
    SpindleSpeed,
}

impl StockTypeFilter {
    fn from_value(value: &str) -> Self {
        match value {
            "drill" => Self::Drill,
            "router" => Self::Router,
            "vbit" => Self::VBit,
            "engraving" => Self::Engraving,
            _ => Self::All,
        }
    }

    fn value(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Drill => "drill",
            Self::Router => "router",
            Self::VBit => "vbit",
            Self::Engraving => "engraving",
        }
    }

    fn matches(self, kind: &str) -> bool {
        match self {
            Self::All => true,
            Self::Drill => stock_tool_type_label(kind) == "Drill",
            Self::Router => stock_tool_type_label(kind) == "Router",
            Self::VBit => stock_tool_type_label(kind) == "V-bit",
            Self::Engraving => stock_tool_type_label(kind) == "Engraving",
        }
    }
}

#[component]
pub fn StockScreen(state: Signal<crate::ctx::AppCtx>) -> Element {
    let mut stock_persist_armed = use_signal(|| false);
    let mut last_stock_fingerprint = use_signal(String::new);

    use_effect(move || {
        state.with_mut(|s| s.ensure_catalogs_loaded());
    });

    // Persist stock when tool content changes (skip initial mount snapshot).
    use_effect(move || {
        let tools = state.read().ui.tools.clone();
        let fingerprint = stock_fingerprint(&tools);

        if !*stock_persist_armed.read() {
            stock_persist_armed.set(true);
            last_stock_fingerprint.set(fingerprint);
            return;
        }

        if *last_stock_fingerprint.read() == fingerprint {
            return;
        }

        last_stock_fingerprint.set(fingerprint);
        let snapshot = state.read().clone();
        sync_ctx_from_ui_state_and_persist_realms(&snapshot.ui, &[PersistRealm::Stock]);
    });

    let snapshot = state.read().clone().ui;
    let has_atc = snapshot.selected_machine_has_atc();
    let unit_system = snapshot.unit_system;

    let mut show_catalog_picker = use_signal(|| false);
    let mut selected_catalog_tool_keys = use_signal(|| BTreeSet::<String>::new());
    let mut selected_stock_tool_ids = use_signal(|| BTreeSet::<String>::new());
    let mut show_delete_confirm = use_signal(|| false);
    let mut stock_feedback = use_signal(String::new);
    let mut stock_filter = use_signal(String::new);
    let mut stock_type_filter = use_signal(|| StockTypeFilter::All);
    let mut stock_sort_mode = use_signal(|| StockSortMode::RecentFirst);

    let mut detail_tool_id = use_signal(|| None::<String>);
    let detail_composite_name = use_signal(String::new);
    let mut detail_custom_name = use_signal(String::new);
    let detail_kind = use_signal(String::new);
    let mut detail_diameter_mm = use_signal(String::new);
    let mut detail_diameter_is_editing = use_signal(|| false);
    let mut detail_diameter_draft = use_signal(String::new);
    let mut detail_point_angle_degrees = use_signal(String::new);
    let mut detail_point_angle_is_editing = use_signal(|| false);
    let mut detail_point_angle_draft = use_signal(String::new);
    let mut detail_feed_rate_mm_per_min = use_signal(String::new);
    let mut detail_feed_rate_is_editing = use_signal(|| false);
    let mut detail_feed_rate_draft = use_signal(String::new);
    let mut detail_spindle_speed_rpm = use_signal(String::new);
    let mut detail_spindle_speed_is_editing = use_signal(|| false);
    let mut detail_spindle_speed_draft = use_signal(String::new);
    let mut detail_pending_focus_field = use_signal(|| None::<StockDetailField>);
    let detail_source_catalog = use_signal(String::new);
    let detail_manufacturer = use_signal(String::new);
    let detail_sku = use_signal(String::new);
    let mut detail_status = use_signal(|| tool_status_value(ToolStatus::InStock).to_string());
    let mut detail_preference = use_signal(|| tool_preference_value(ToolPreference::Neutral).to_string());
    let mut detail_field_popup_message = use_signal(|| None::<String>);

    let selected_catalog_count = selected_catalog_tool_keys.read().len();
    let selected_stock_count = selected_stock_tool_ids.read().len();
    let filter_value = stock_filter.read().clone();
    let filter_lower = filter_value.to_ascii_lowercase();
    let type_filter = *stock_type_filter.read();
    let sort_mode = *stock_sort_mode.read();

    let mut filtered_tools: Vec<(usize, &Tool)> = snapshot
        .tools
        .iter()
        .enumerate()
        .filter(|(_, tool)| {
            let display_name = tool.display_name().to_ascii_lowercase();

            type_filter.matches(&tool.kind)
                && (filter_lower.is_empty()
                    || display_name.contains(&filter_lower)
                    || tool.composite_name.to_ascii_lowercase().contains(&filter_lower)
                    || tool.name.to_ascii_lowercase().contains(&filter_lower)
                    || tool.kind.to_ascii_lowercase().contains(&filter_lower)
                    || stock_tool_type_label(&tool.kind).to_ascii_lowercase().contains(&filter_lower)
                    || tool.source_catalog.to_ascii_lowercase().contains(&filter_lower)
                    || tool.preference.label().to_ascii_lowercase().contains(&filter_lower)
                    || tool.status.label().to_ascii_lowercase().contains(&filter_lower))
        })
        .collect();

    match sort_mode {
        StockSortMode::RecentFirst => filtered_tools.sort_by(|left, right| right.0.cmp(&left.0)),
        StockSortMode::Type => filtered_tools.sort_by(|left, right| {
            stock_tool_type_rank(&left.1.kind)
                .cmp(&stock_tool_type_rank(&right.1.kind))
                .then_with(|| right.0.cmp(&left.0))
        }),
    }

    let filtered_tools_is_empty = filtered_tools.is_empty();
    let visible_tool_ids: Vec<String> = filtered_tools.iter().map(|(_, tool)| tool.id.clone()).collect();
    let selected_visible_count = visible_tool_ids
        .iter()
        .filter(|tool_id| selected_stock_tool_ids.read().contains(tool_id.as_str()))
        .count();
    let all_visible_selected = !visible_tool_ids.is_empty() && selected_visible_count == visible_tool_ids.len();

    let active_tool = detail_tool_id
        .read()
        .clone()
        .and_then(|tool_id| snapshot.tools.iter().find(|tool| tool.id == tool_id).cloned());

    let detail_diameter_display = if *detail_diameter_is_editing.read() {
        detail_diameter_draft.read().clone()
    } else if let Some(tool) = active_tool.as_ref() {
        format_length_for_user(tool.diameter, unit_system)
    } else {
        format_length_field_display(&detail_diameter_mm.read(), unit_system)
    };
    let detail_point_angle_display = if *detail_point_angle_is_editing.read() {
        detail_point_angle_draft.read().clone()
    } else if let Some(tool) = active_tool.as_ref() {
        unit_service::format_angle_display(tool.point_angle)
    } else {
        format_angle_field_display(&detail_point_angle_degrees.read())
    };
    let detail_feed_rate_display = if *detail_feed_rate_is_editing.read() {
        detail_feed_rate_draft.read().clone()
    } else if let Some(tool) = active_tool.as_ref() {
        tool.feed_rate
            .map(|value| format_feed_rate_for_user(value, unit_system))
            .unwrap_or_default()
    } else {
        format_feed_rate_field_display(&detail_feed_rate_mm_per_min.read(), unit_system)
    };
    let detail_spindle_speed_display = if *detail_spindle_speed_is_editing.read() {
        detail_spindle_speed_draft.read().clone()
    } else if let Some(tool) = active_tool.as_ref() {
        tool.spindle_speed
            .map(unit_service::format_rotational_speed_display)
            .unwrap_or_default()
    } else {
        format_rotational_speed_field_display(&detail_spindle_speed_rpm.read())
    };
    let diameter_edit_seed = active_tool
        .as_ref()
        .map(|tool| unit_service::format_length_edit_display(tool.diameter, unit_system))
        .unwrap_or_else(|| detail_diameter_mm.read().clone());
    let feed_rate_edit_seed = active_tool
        .as_ref()
        .and_then(|tool| tool.feed_rate)
        .map(|feed_rate| format_feed_rate_edit_display(feed_rate, unit_system))
        .unwrap_or_default();

    let detail_diameter_original_display = active_tool.as_ref().and_then(|tool| {
        tool.catalog_diameter
            .map(|value| unit_service::format_length_display(value, unit_system))
    });
    let detail_diameter_is_modified = active_tool
        .as_ref()
        .and_then(|tool| {
            tool.catalog_diameter
                .map(|original| (tool.diameter.as_mm() - original.as_mm()).abs() > 1e-9)
        })
        .unwrap_or(false);

    let detail_point_angle_original_display = active_tool.as_ref().and_then(|tool| {
        tool.catalog_point_angle
            .map(unit_service::format_angle_display)
    });
    let detail_point_angle_is_modified = active_tool
        .as_ref()
        .and_then(|tool| {
            tool.catalog_point_angle
                .map(|original| (tool.point_angle.as_degrees() - original.as_degrees()).abs() > 1e-9)
        })
        .unwrap_or(false);

    let detail_feed_rate_original_display = active_tool.as_ref().and_then(|tool| {
        tool.catalog_feed_rate
            .map(|value| format_feed_rate_for_user(value, unit_system))
    });
    let detail_feed_rate_is_modified = active_tool
        .as_ref()
        .map(|tool| option_feed_rate_changed(tool.feed_rate, tool.catalog_feed_rate))
        .unwrap_or(false);

    let detail_spindle_speed_original_display = active_tool.as_ref().and_then(|tool| {
        tool.catalog_spindle_speed
            .map(unit_service::format_rotational_speed_display)
    });
    let detail_spindle_speed_is_modified = active_tool
        .as_ref()
        .map(|tool| option_spindle_speed_changed(tool.spindle_speed, tool.catalog_spindle_speed))
        .unwrap_or(false);

    rsx! {
        div { class: "screen single stock-shell",
            div { class: "stock-toolbar",
                div {
                    h3 { "Stock" }
                    p { class: "diag-status",
                        "Manage installed tools and pull additional entries from your catalogs."
                    }
                }

                if active_tool.is_none() {
                    div { class: "stock-toolbar-actions",
                        input {
                            class: "stock-filter-input",
                            value: filter_value,
                            placeholder: "Filter by type, name, source, preference or status",
                            oninput: move |evt| stock_filter.set(evt.value()),
                        }
                        select {
                            class: "stock-toolbar-select",
                            value: type_filter.value(),
                            onchange: move |evt| stock_type_filter.set(StockTypeFilter::from_value(&evt.value())),
                            option { value: "all", "All types" }
                            option { value: "drill", "Drill" }
                            option { value: "router", "Router" }
                            option { value: "vbit", "V-bit" }
                            option { value: "engraving", "Engraving" }
                        }
                        select {
                            class: "stock-toolbar-select",
                            value: sort_mode.value(),
                            onchange: move |evt| stock_sort_mode.set(StockSortMode::from_value(&evt.value())),
                            option { value: "recent", "Latest first" }
                            option { value: "type", "Sort by type" }
                        }
                        if selected_stock_count > 0 {
                            button {
                                class: "btn btn-danger",
                                onclick: move |_| show_delete_confirm.set(true),
                                "Delete Selected ({selected_stock_count})"
                            }
                        }
                        button {
                            class: "btn btn-primary",
                            onclick: move |_| {
                                selected_catalog_tool_keys.set(BTreeSet::new());
                                show_catalog_picker.set(true);
                            },
                            "Add tools from catalog"
                        }
                    }
                } else {
                    div { class: "stock-toolbar-actions",
                        button {
                            class: "btn btn-secondary",
                            onclick: move |_| detail_tool_id.set(None),
                            "Back To Stock"
                        }
                    }
                }
            }

            if !stock_feedback.read().is_empty() {
                p { class: "diag-status", "{stock_feedback}" }
            }

            if *show_catalog_picker.read() {
                div { class: "wizard-overlay",
                    div { class: "catalog-picker-dialog",
                        div { class: "panel-header",
                            div {
                                h3 { "Add tools from catalog" }
                                p { "Select one or more tools from available catalogs." }
                            }
                        }

                        div { class: "catalog-picker-list",
                            for catalog in snapshot.catalogs.iter() {
                                details {
                                    key: "{catalog.key}",
                                    class: "catalog-node",
                                    summary { class: "catalog-node-summary",
                                        if catalog.built_in {
                                            "{catalog.name} (built-in)"
                                        } else {
                                            "{catalog.name}"
                                        }
                                    }

                                    for section in catalog.sections.iter() {
                                        details {
                                            key: "{section.key}",
                                            class: "catalog-node section-node",
                                            summary { class: "catalog-node-summary",
                                                "{section.name} ({section.tools.len()} tools)"
                                            }

                                            div { class: "catalog-tool-list",
                                                div { class: "catalog-tool-header",
                                                    span { class: "catalog-tool-col-label",
                                                        "Label / SKU"
                                                    }
                                                    span { class: "catalog-tool-col-type",
                                                        "Type"
                                                    }
                                                    span { class: "catalog-tool-col-diameter",
                                                        "Diameter"
                                                    }
                                                }
                                                for tool in section.tools.iter() {
                                                    label {
                                                        key: "{tool.key}",
                                                        class: "catalog-tool-row",
                                                        input {
                                                            r#type: "checkbox",
                                                            checked: selected_catalog_tool_keys.read().contains(&tool.key),
                                                            oninput: {
                                                                let tool_key = tool.key.clone();
                                                                move |evt: FormEvent| {
                                                                    let checked = evt.checked();
                                                                    selected_catalog_tool_keys
                                                                        .with_mut(|selected| {
                                                                            if checked {
                                                                                selected.insert(tool_key.clone());
                                                                            } else {
                                                                                selected.remove(&tool_key);
                                                                            }
                                                                        });
                                                                }
                                                            },
                                                        }
                                                        span { class: "catalog-tool-label",
                                                            "{tool.display_name}"
                                                        }
                                                        span { class: "catalog-tool-type",
                                                            "{catalog_tool_type(tool)}"
                                                        }
                                                        span { class: "catalog-tool-diameter",
                                                            "{catalog_tool_diameter(tool, unit_system)}"
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        div { class: "wizard-actions",
                            button {
                                class: "btn btn-secondary",
                                onclick: move |_| show_catalog_picker.set(false),
                                "Cancel"
                            }
                            button {
                                class: "btn btn-primary",
                                disabled: selected_catalog_count == 0,
                                onclick: move |_| {
                                    let selected: Vec<String> = selected_catalog_tool_keys
                                        .read()
                                        .iter()
                                        .cloned()
                                        .collect();
                                    let mut added = 0usize;
                                    state
                                        .with_mut(|s| {
                                            added = s.ui.add_tools_from_catalog_selection(&selected);
                                        });
                                    stock_feedback.set(format!("Added {} tool(s) from catalogs", added));
                                    selected_catalog_tool_keys.set(BTreeSet::new());
                                    show_catalog_picker.set(false);
                                },
                                "Add Selected ({selected_catalog_count})"
                            }
                        }
                    }
                }
            }

            if *show_delete_confirm.read() {
                div { class: "wizard-overlay",
                    div { class: "wizard-dialog",
                        h3 { "Delete tools" }
                        p {
                            "Delete {selected_stock_count} selected tool(s)? This also clears any rack assignment and project tool references."
                        }
                        div { class: "wizard-actions",
                            button {
                                class: "btn btn-secondary",
                                onclick: move |_| show_delete_confirm.set(false),
                                "Cancel"
                            }
                            button {
                                class: "btn btn-danger",
                                onclick: move |_| {
                                    let selected: Vec<String> = selected_stock_tool_ids
                                        .read()
                                        .iter()
                                        .cloned()
                                        .collect();
                                    let active_detail_tool_id = detail_tool_id.read().clone();
                                    let mut removed = 0usize;
                                    state
                                        .with_mut(|s| {
                                            removed = s.ui.remove_tools(&selected);
                                        });
                                    if active_detail_tool_id
                                        .as_ref()
                                        .map(|tool_id| selected.iter().any(|selected_id| selected_id == tool_id))
                                        .unwrap_or(false)
                                    {
                                        detail_tool_id.set(None);
                                    }
                                    selected_stock_tool_ids.set(BTreeSet::new());
                                    show_delete_confirm.set(false);
                                    stock_feedback.set(format!("Deleted {} tool(s)", removed));
                                },
                                "Delete"
                            }
                        }
                    }
                }
            }

            if let Some(tool) = active_tool.as_ref() {
                div {
                    class: "stock-detail-page",
                    tabindex: "0",
                    onmounted: move |evt| async move {
                        let _ = evt.set_focus(true).await;
                    },
                    onkeydown: move |evt| {
                        let key = evt.key().to_string().to_ascii_lowercase();
                        if (key == "escape" || key == "esc")
                            && !*detail_diameter_is_editing.read()
                            && !*detail_point_angle_is_editing.read()
                            && !*detail_feed_rate_is_editing.read()
                            && !*detail_spindle_speed_is_editing.read()
                        {
                            detail_tool_id.set(None);
                        }
                    },
                    div { class: "panel stock-detail-panel",
                        div { class: "panel-header",
                            div {
                                h3 { "Tool detail" }
                                p { "Edit the tool properties, add a custom name, or clone the tool." }
                            }
                            div { class: "actions",
                                button {
                                    class: "btn btn-secondary",
                                    onclick: move |_| detail_tool_id.set(None),
                                    "Back"
                                }
                                button {
                                    class: "btn btn-secondary",
                                    onclick: {
                                        let tool_id = tool.id.clone();
                                        move |_| {
                                            let mut cloned_tool = None::<Tool>;
                                            state
                                                .with_mut(|s| {
                                                    if let Some(new_id) = s.ui.clone_tool(&tool_id) {
                                                        cloned_tool = s
                                                            .ui
                                                            .tools
                                                            .iter()
                                                            .find(|entry| entry.id == new_id)
                                                            .cloned();
                                                    }
                                                });
                                            if let Some(clone) = cloned_tool {
                                                load_tool_editor(
                                                    &clone,
                                                    unit_system,
                                                    detail_tool_id,
                                                    detail_composite_name,
                                                    detail_custom_name,
                                                    detail_kind,
                                                    detail_diameter_mm,
                                                    detail_diameter_is_editing,
                                                    detail_diameter_draft,
                                                    detail_point_angle_degrees,
                                                    detail_point_angle_is_editing,
                                                    detail_point_angle_draft,
                                                    detail_feed_rate_mm_per_min,
                                                    detail_feed_rate_is_editing,
                                                    detail_feed_rate_draft,
                                                    detail_spindle_speed_rpm,
                                                    detail_spindle_speed_is_editing,
                                                    detail_spindle_speed_draft,
                                                    detail_source_catalog,
                                                    detail_manufacturer,
                                                    detail_sku,
                                                    detail_status,
                                                    detail_preference,
                                                );
                                                stock_feedback.set(format!("Cloned {}", clone.display_name()));
                                            }
                                        }
                                    },
                                    "Clone Tool"
                                }
                            }
                        }

                        if let Some(popup) = detail_field_popup_message.read().clone() {
                            div { class: "stock-field-popup", "{popup}" }
                        }

                        div { class: "stock-detail-form",
                            div { class: "stock-detail-row",
                                div { class: "stock-detail-label", "Label" }
                                div { class: "stock-detail-readonly", "{detail_composite_name.read()}" }
                            }
                            div { class: "stock-detail-row",
                                div { class: "stock-detail-label", "Custom name" }
                                input {
                                    class: "stock-detail-input",
                                    value: detail_custom_name.read().clone(),
                                    placeholder: "Optional nickname",
                                    oninput: move |evt| {
                                        let value = evt.value();
                                        detail_custom_name.set(value.clone());
                                        if let Some(tool_id) = detail_tool_id.read().clone() {
                                            state
                                                .with_mut(|ui_state| {
                                                    if let Some(target) = ui_state
                                                        .ui
                                                        .tools
                                                        .iter_mut()
                                                        .find(|entry| entry.id == tool_id)
                                                    {
                                                        target.name = value.clone();
                                                    }
                                                });
                                            persist_stock_realm_now(state);
                                        }
                                    },
                                }
                            }
                            div { class: "stock-detail-row",
                                div { class: "stock-detail-label", "Type" }
                                div { class: "stock-detail-readonly", "{detail_kind.read()}" }
                            }
                            div { class: "stock-detail-row",
                                div { class: "stock-detail-label", "Diameter" }
                                div { class: "stock-detail-field-value",
                                    if *detail_diameter_is_editing.read() {
                                        input {
                                            class: "stock-detail-input",
                                            value: detail_diameter_display,
                                            autofocus: true,
                                            onmounted: move |evt| async move {
                                                let _ = evt.set_focus(true).await;
                                            },
                                            onfocusin: move |_| detail_pending_focus_field.set(None),
                                            oninput: move |evt| {
                                                let value = evt.value();
                                                detail_diameter_draft.set(value);
                                            },
                                            onkeydown: move |evt| {
                                                let key = evt.key().to_string().to_ascii_lowercase();
                                                if key == "enter" || key == "numpadenter" {
                                                    let value = detail_diameter_draft.read().trim().to_string();
                                                    if value.is_empty() {
                                                        detail_field_popup_message
                                                            .set(Some("Diameter must be a valid length".to_string()));
                                                        return;
                                                    }

                                                    let normalized_input = if length_input_has_explicit_unit(&value) {
                                                        value
                                                    } else {
                                                        format!("{}{}", value, default_length_unit_suffix(unit_system))
                                                    };

                                                    match parse_length_for_display_input(&normalized_input, unit_system) {
                                                        Ok(length) if length.as_mm() > 0.0 => {
                                                            let normalized = unit_service::format_length_edit_display(
                                                                length,
                                                                unit_system,
                                                            );
                                                            detail_diameter_mm.set(normalized.clone());
                                                            detail_diameter_draft.set(normalized);
                                                            detail_diameter_is_editing.set(false);
                                                            detail_pending_focus_field.set(None);
                                                            detail_field_popup_message.set(None);
                                                            if let Some(tool_id) = detail_tool_id.read().clone() {
                                                                state
                                                                    .with_mut(|ui_state| {
                                                                        if let Some(target) = ui_state
                                                                            .ui
                                                                            .tools
                                                                            .iter_mut()
                                                                            .find(|entry| entry.id == tool_id)
                                                                        {
                                                                            target.diameter = length;
                                                                        }
                                                                    });
                                                                persist_stock_realm_now(state);
                                                            }
                                                        }
                                                        Ok(_) => {
                                                            detail_field_popup_message
                                                                .set(Some("Diameter must be greater than zero".to_string()));
                                                        }
                                                        Err(_) => {
                                                            detail_field_popup_message
                                                                .set(Some("Diameter must be a valid length".to_string()));
                                                        }
                                                    }
                                                } else if key == "escape" || key == "esc" {
                                                    evt.stop_propagation();
                                                    detail_diameter_draft.set(detail_diameter_mm.read().clone());
                                                    detail_diameter_is_editing.set(false);
                                                    detail_pending_focus_field.set(None);
                                                    detail_field_popup_message.set(None);
                                                }
                                            },
                                            onfocusout: {
                                                let feed_rate_edit_seed = feed_rate_edit_seed.clone();
                                                move |_| {
                                                    detail_diameter_draft.set(detail_diameter_mm.read().clone());
                                                    detail_diameter_is_editing.set(false);
                                                    if let Some(next) = *detail_pending_focus_field.read() {
                                                        if next != StockDetailField::Diameter {
                                                            match next {
                                                                StockDetailField::PointAngle => {
                                                                    detail_point_angle_is_editing.set(true);
                                                                    detail_point_angle_draft
                                                                        .set(detail_point_angle_degrees.read().clone());
                                                                }
                                                                StockDetailField::FeedRate => {
                                                                    detail_feed_rate_is_editing.set(true);
                                                                    detail_feed_rate_draft
                                                                        .set(feed_rate_edit_seed.clone());
                                                                }
                                                                StockDetailField::SpindleSpeed => {
                                                                    detail_spindle_speed_is_editing.set(true);
                                                                    detail_spindle_speed_draft
                                                                        .set(detail_spindle_speed_rpm.read().clone());
                                                                }
                                                                StockDetailField::Diameter => {}
                                                            }
                                                            detail_field_popup_message.set(None);
                                                        }
                                                    }
                                                    detail_pending_focus_field.set(None);
                                                }
                                            },
                                        }
                                    } else {
                                        button {
                                            r#type: "button",
                                            class: "stock-detail-input stock-detail-trigger",
                                            onmousedown: {
                                                let diameter_edit_seed = diameter_edit_seed.clone();
                                                move |_| {
                                                    detail_pending_focus_field.set(Some(StockDetailField::Diameter));
                                                    detail_diameter_is_editing.set(true);
                                                    detail_diameter_draft.set(diameter_edit_seed.clone());
                                                    detail_field_popup_message.set(None);
                                                }
                                            },
                                            onclick: {
                                                let diameter_edit_seed = diameter_edit_seed.clone();
                                                move |_| {
                                                    detail_pending_focus_field.set(None);
                                                    detail_diameter_is_editing.set(true);
                                                    detail_diameter_draft.set(diameter_edit_seed.clone());
                                                    detail_field_popup_message.set(None);
                                                }
                                            },
                                            "{detail_diameter_display}"
                                        }
                                    }

                                    if detail_diameter_is_modified {
                                        if let Some(original_value) = detail_diameter_original_display.clone() {
                                            div { class: "stock-detail-original-group",
                                                span { class: "stock-detail-original-value",
                                                    "{original_value}"
                                                }
                                                button {
                                                    r#type: "button",
                                                    class: "stock-detail-revert-btn",
                                                    title: "Revert to catalog value",
                                                    onclick: {
                                                        let tool_id = tool.id.clone();
                                                        let original_diameter = tool.catalog_diameter;
                                                        move |_| {
                                                            if let Some(original_diameter) = original_diameter {
                                                                let normalized = unit_service::format_length_edit_display(
                                                                    original_diameter,
                                                                    unit_system,
                                                                );
                                                                detail_diameter_mm.set(normalized.clone());
                                                                detail_diameter_draft.set(normalized);
                                                                detail_diameter_is_editing.set(false);
                                                                detail_pending_focus_field.set(None);
                                                                detail_field_popup_message.set(None);
                                                                state
                                                                    .with_mut(|ui_state| {
                                                                        if let Some(target) = ui_state
                                                                            .ui
                                                                            .tools
                                                                            .iter_mut()
                                                                            .find(|entry| entry.id == tool_id)
                                                                        {
                                                                            target.diameter = original_diameter;
                                                                        }
                                                                    });
                                                                persist_stock_realm_now(state);
                                                            }
                                                        }
                                                    },
                                                    "↺"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            div { class: "stock-detail-row",
                                div { class: "stock-detail-label", "Tip geometry" }
                                div { class: "stock-detail-field-value",
                                    if *detail_point_angle_is_editing.read() {
                                        input {
                                            class: "stock-detail-input",
                                            value: detail_point_angle_display,
                                            autofocus: true,
                                            onmounted: move |evt| async move {
                                                let _ = evt.set_focus(true).await;
                                            },
                                            onfocusin: move |_| detail_pending_focus_field.set(None),
                                            oninput: move |evt| detail_point_angle_draft.set(evt.value()),
                                            onkeydown: move |evt| {
                                                let key = evt.key().to_string().to_ascii_lowercase();
                                                if key == "enter" || key == "numpadenter" {
                                                    let draft_value = detail_point_angle_draft.read().clone();
                                                    match unit_service::parse_angle(&draft_value) {
                                                        Ok(angle) if angle.as_degrees() > 0.0 && angle.as_degrees() <= 180.0 => {
                                                            let normalized = unit_service::format_angle_edit_display(angle);
                                                            detail_point_angle_degrees.set(normalized.clone());
                                                            detail_point_angle_draft.set(normalized);
                                                            detail_point_angle_is_editing.set(false);
                                                            detail_pending_focus_field.set(None);
                                                            detail_field_popup_message.set(None);
                                                            if let Some(tool_id) = detail_tool_id.read().clone() {
                                                                state
                                                                    .with_mut(|ui_state| {
                                                                        if let Some(target) = ui_state
                                                                            .ui
                                                                            .tools
                                                                            .iter_mut()
                                                                            .find(|entry| entry.id == tool_id)
                                                                        {
                                                                            target.point_angle = angle;
                                                                        }
                                                                    });
                                                                persist_stock_realm_now(state);
                                                            }
                                                        }
                                                        Ok(_) => {
                                                            detail_field_popup_message
                                                                .set(
                                                                    Some(
                                                                        "Tip geometry must be greater than 0 and at most 180 degrees"
                                                                            .to_string(),
                                                                    ),
                                                                );
                                                        }
                                                        Err(_) => {
                                                            detail_field_popup_message
                                                                .set(Some("Tip geometry must be a valid angle".to_string()));
                                                        }
                                                    }
                                                } else if key == "escape" || key == "esc" {
                                                    evt.stop_propagation();
                                                    detail_point_angle_draft.set(detail_point_angle_degrees.read().clone());
                                                    detail_point_angle_is_editing.set(false);
                                                    detail_pending_focus_field.set(None);
                                                    detail_field_popup_message.set(None);
                                                }
                                            },
                                            onfocusout: {
                                                let diameter_edit_seed = diameter_edit_seed.clone();
                                                let feed_rate_edit_seed = feed_rate_edit_seed.clone();
                                                move |_| {
                                                    detail_point_angle_draft
                                                        .set(detail_point_angle_degrees.read().clone());
                                                    detail_point_angle_is_editing.set(false);
                                                    if let Some(next) = *detail_pending_focus_field.read() {
                                                        if next != StockDetailField::PointAngle {
                                                            match next {
                                                                StockDetailField::Diameter => {
                                                                    detail_diameter_is_editing.set(true);
                                                                    detail_diameter_draft
                                                                        .set(diameter_edit_seed.clone());
                                                                }
                                                                StockDetailField::FeedRate => {
                                                                    detail_feed_rate_is_editing.set(true);
                                                                    detail_feed_rate_draft
                                                                        .set(feed_rate_edit_seed.clone());
                                                                }
                                                                StockDetailField::SpindleSpeed => {
                                                                    detail_spindle_speed_is_editing.set(true);
                                                                    detail_spindle_speed_draft
                                                                        .set(detail_spindle_speed_rpm.read().clone());
                                                                }
                                                                StockDetailField::PointAngle => {}
                                                            }
                                                            detail_field_popup_message.set(None);
                                                        }
                                                    }
                                                    detail_pending_focus_field.set(None);
                                                }
                                            },
                                        }
                                    } else {
                                        button {
                                            r#type: "button",
                                            class: "stock-detail-input stock-detail-trigger",
                                            onmousedown: move |_| {
                                                detail_pending_focus_field.set(Some(StockDetailField::PointAngle));
                                                detail_point_angle_is_editing.set(true);
                                                detail_point_angle_draft
                                                    .set(detail_point_angle_degrees.read().clone());
                                                detail_field_popup_message.set(None);
                                            },
                                            onclick: move |_| {
                                                detail_pending_focus_field.set(None);
                                                detail_point_angle_is_editing.set(true);
                                                detail_point_angle_draft
                                                    .set(detail_point_angle_degrees.read().clone());
                                                detail_field_popup_message.set(None);
                                            },
                                            "{detail_point_angle_display}"
                                        }

                                        if detail_point_angle_is_modified {
                                            if let Some(original_value) = detail_point_angle_original_display.clone() {
                                                div { class: "stock-detail-original-group",
                                                    span { class: "stock-detail-original-value",
                                                        "{original_value}"
                                                    }
                                                    button {
                                                        r#type: "button",
                                                        class: "stock-detail-revert-btn",
                                                        title: "Revert to catalog value",
                                                        onclick: {
                                                            let tool_id = tool.id.clone();
                                                            let original_point_angle = tool.catalog_point_angle;
                                                            move |_| {
                                                                if let Some(original_point_angle) = original_point_angle {
                                                                    let normalized = unit_service::format_angle_edit_display(
                                                                        original_point_angle,
                                                                    );
                                                                    detail_point_angle_degrees.set(normalized.clone());
                                                                    detail_point_angle_draft.set(normalized);
                                                                    detail_point_angle_is_editing.set(false);
                                                                    detail_pending_focus_field.set(None);
                                                                    detail_field_popup_message.set(None);
                                                                    state
                                                                        .with_mut(|ui_state| {
                                                                            if let Some(target) = ui_state
                                                                                .ui
                                                                                .tools
                                                                                .iter_mut()
                                                                                .find(|entry| entry.id == tool_id)
                                                                            {
                                                                                target.point_angle = original_point_angle;
                                                                            }
                                                                        });
                                                                    persist_stock_realm_now(state);
                                                                }
                                                            }
                                                        },
                                                        "↺"
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            div { class: "stock-detail-row",
                                div { class: "stock-detail-label", "Feed rate" }
                                div { class: "stock-detail-field-value",
                                    if *detail_feed_rate_is_editing.read() {
                                        input {
                                            class: "stock-detail-input",
                                            value: detail_feed_rate_display,
                                            placeholder: "Optional",
                                            autofocus: true,
                                            onmounted: move |evt| async move {
                                                let _ = evt.set_focus(true).await;
                                            },
                                            onfocusin: move |_| detail_pending_focus_field.set(None),
                                            oninput: move |evt| detail_feed_rate_draft.set(evt.value()),
                                            onkeydown: move |evt| {
                                                let key = evt.key().to_string().to_ascii_lowercase();
                                                if key == "enter" || key == "numpadenter" {
                                                    let value = detail_feed_rate_draft.read().trim().to_string();
                                                    if value.is_empty() {
                                                        detail_feed_rate_mm_per_min.set(String::new());
                                                        detail_feed_rate_draft.set(String::new());
                                                        detail_feed_rate_is_editing.set(false);
                                                        detail_pending_focus_field.set(None);
                                                        detail_field_popup_message.set(None);
                                                        if let Some(tool_id) = detail_tool_id.read().clone() {
                                                            state
                                                                .with_mut(|ui_state| {
                                                                    if let Some(target) = ui_state
                                                                        .ui
                                                                        .tools
                                                                        .iter_mut()
                                                                        .find(|entry| entry.id == tool_id)
                                                                    {
                                                                        target.feed_rate = None;
                                                                    }
                                                                });
                                                            persist_stock_realm_now(state);
                                                        }
                                                        return;
                                                    }
                                                    let normalized_input = if feed_rate_input_has_explicit_unit(&value) {
                                                        value
                                                    } else {
                                                        format!("{}{}", value, default_feed_unit_suffix(unit_system))
                                                    };
                                                    match parse_optional_feed_rate(&normalized_input, unit_system, "Feed rate") {
                                                        Ok(Some(feed_rate)) => {
                                                            let normalized = format_feed_rate_edit_display(
                                                                feed_rate,
                                                                unit_system,
                                                            );
                                                            detail_feed_rate_mm_per_min.set(normalized.clone());
                                                            detail_feed_rate_draft.set(normalized);
                                                            detail_feed_rate_is_editing.set(false);
                                                            detail_pending_focus_field.set(None);
                                                            detail_field_popup_message.set(None);
                                                            if let Some(tool_id) = detail_tool_id.read().clone() {
                                                                state
                                                                    .with_mut(|ui_state| {
                                                                        if let Some(target) = ui_state
                                                                            .ui
                                                                            .tools
                                                                            .iter_mut()
                                                                            .find(|entry| entry.id == tool_id)
                                                                        {
                                                                            target.feed_rate = Some(feed_rate);
                                                                        }
                                                                    });
                                                                persist_stock_realm_now(state);
                                                            }
                                                        }
                                                        Ok(None) => {
                                                            detail_feed_rate_mm_per_min.set(String::new());
                                                            detail_feed_rate_draft.set(String::new());
                                                            detail_feed_rate_is_editing.set(false);
                                                            detail_pending_focus_field.set(None);
                                                            detail_field_popup_message.set(None);
                                                            if let Some(tool_id) = detail_tool_id.read().clone() {
                                                                state
                                                                    .with_mut(|ui_state| {
                                                                        if let Some(target) = ui_state
                                                                            .ui
                                                                            .tools
                                                                            .iter_mut()
                                                                            .find(|entry| entry.id == tool_id)
                                                                        {
                                                                            target.feed_rate = None;
                                                                        }
                                                                    });
                                                                persist_stock_realm_now(state);
                                                            }
                                                        }
                                                        Err(message) => {
                                                            detail_field_popup_message.set(Some(message));
                                                        }
                                                    }
                                                } else if key == "escape" || key == "esc" {
                                                    evt.stop_propagation();
                                                    detail_feed_rate_draft.set(detail_feed_rate_mm_per_min.read().clone());
                                                    detail_feed_rate_is_editing.set(false);
                                                    detail_pending_focus_field.set(None);
                                                    detail_field_popup_message.set(None);
                                                }
                                            },
                                            onfocusout: move |_| {
                                                detail_feed_rate_draft
                                                    .set(detail_feed_rate_mm_per_min.read().clone());
                                                detail_feed_rate_is_editing.set(false);
                                                if let Some(next) = *detail_pending_focus_field.read() {
                                                    if next != StockDetailField::FeedRate {
                                                        match next {
                                                            StockDetailField::Diameter => {
                                                                detail_diameter_is_editing.set(true);
                                                                detail_diameter_draft
                                                                    .set(detail_diameter_mm.read().clone());
                                                            }
                                                            StockDetailField::PointAngle => {
                                                                detail_point_angle_is_editing.set(true);
                                                                detail_point_angle_draft
                                                                    .set(detail_point_angle_degrees.read().clone());
                                                            }
                                                            StockDetailField::SpindleSpeed => {
                                                                detail_spindle_speed_is_editing.set(true);
                                                                detail_spindle_speed_draft
                                                                    .set(detail_spindle_speed_rpm.read().clone());
                                                            }
                                                            StockDetailField::FeedRate => {}
                                                        }
                                                        detail_field_popup_message.set(None);
                                                    }
                                                }
                                                detail_pending_focus_field.set(None);
                                            },
                                        }
                                    } else {
                                        button {
                                            r#type: "button",
                                            class: "stock-detail-input stock-detail-trigger",
                                            onmousedown: {
                                                let feed_rate_edit_seed = feed_rate_edit_seed.clone();
                                                move |_| {
                                                    detail_pending_focus_field.set(Some(StockDetailField::FeedRate));
                                                    detail_feed_rate_is_editing.set(true);
                                                    detail_feed_rate_draft.set(feed_rate_edit_seed.clone());
                                                    detail_field_popup_message.set(None);
                                                }
                                            },
                                            onclick: {
                                                let feed_rate_edit_seed = feed_rate_edit_seed.clone();
                                                move |_| {
                                                    detail_pending_focus_field.set(None);
                                                    detail_feed_rate_is_editing.set(true);
                                                    detail_feed_rate_draft.set(feed_rate_edit_seed.clone());
                                                    detail_field_popup_message.set(None);
                                                }
                                            },
                                            if detail_feed_rate_display.is_empty() {
                                                "Optional"
                                            } else {
                                                "{detail_feed_rate_display}"
                                            }
                                        }

                                        if detail_feed_rate_is_modified {
                                            if let Some(original_value) = detail_feed_rate_original_display.clone() {
                                                div { class: "stock-detail-original-group",
                                                    span { class: "stock-detail-original-value",
                                                        "{original_value}"
                                                    }
                                                    button {
                                                        r#type: "button",
                                                        class: "stock-detail-revert-btn",
                                                        title: "Revert to catalog value",
                                                        onclick: {
                                                            let tool_id = tool.id.clone();
                                                            let original_feed_rate = tool.catalog_feed_rate;
                                                            move |_| {
                                                                if let Some(original_feed_rate) = original_feed_rate {
                                                                    let normalized = format_feed_rate_edit_display(
                                                                        original_feed_rate,
                                                                        unit_system,
                                                                    );
                                                                    detail_feed_rate_mm_per_min.set(normalized.clone());
                                                                    detail_feed_rate_draft.set(normalized);
                                                                    detail_feed_rate_is_editing.set(false);
                                                                    detail_pending_focus_field.set(None);
                                                                    detail_field_popup_message.set(None);
                                                                    state
                                                                        .with_mut(|ui_state| {
                                                                            if let Some(target) = ui_state
                                                                                .ui
                                                                                .tools
                                                                                .iter_mut()
                                                                                .find(|entry| entry.id == tool_id)
                                                                            {
                                                                                target.feed_rate = Some(original_feed_rate);
                                                                            }
                                                                        });
                                                                    persist_stock_realm_now(state);
                                                                }
                                                            }
                                                        },
                                                        "↺"
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            div { class: "stock-detail-row",
                                div { class: "stock-detail-label", "Spindle speed" }
                                div { class: "stock-detail-field-value",
                                    if *detail_spindle_speed_is_editing.read() {
                                        input {
                                            class: "stock-detail-input",
                                            value: detail_spindle_speed_display,
                                            placeholder: "Optional",
                                            autofocus: true,
                                            onmounted: move |evt| async move {
                                                let _ = evt.set_focus(true).await;
                                            },
                                            onfocusin: move |_| detail_pending_focus_field.set(None),
                                            oninput: move |evt| detail_spindle_speed_draft.set(evt.value()),
                                            onkeydown: move |evt| {
                                                let key = evt.key().to_string().to_ascii_lowercase();
                                                if key == "enter" || key == "numpadenter" {
                                                    let draft_value = detail_spindle_speed_draft.read().clone();
                                                    match unit_service::parse_rotational_speed(&draft_value) {
                                                        Ok(value) => {
                                                            let normalized = unit_service::format_rotational_speed_edit_display(
                                                                value,
                                                            );
                                                            detail_spindle_speed_rpm.set(normalized.clone());
                                                            detail_spindle_speed_draft.set(normalized);
                                                            detail_spindle_speed_is_editing.set(false);
                                                            detail_pending_focus_field.set(None);
                                                            detail_field_popup_message.set(None);
                                                            if let Some(tool_id) = detail_tool_id.read().clone() {
                                                                state
                                                                    .with_mut(|ui_state| {
                                                                        if let Some(target) = ui_state
                                                                            .ui
                                                                            .tools
                                                                            .iter_mut()
                                                                            .find(|entry| entry.id == tool_id)
                                                                        {
                                                                            target.spindle_speed = Some(value);
                                                                        }
                                                                    });
                                                                persist_stock_realm_now(state);
                                                            }
                                                        }
                                                        Err(_) => {
                                                            detail_field_popup_message
                                                                .set(
                                                                    Some("Spindle speed must be a valid rpm value".to_string()),
                                                                );
                                                        }
                                                    }
                                                } else if key == "escape" || key == "esc" {
                                                    evt.stop_propagation();
                                                    detail_spindle_speed_draft.set(detail_spindle_speed_rpm.read().clone());
                                                    detail_spindle_speed_is_editing.set(false);
                                                    detail_pending_focus_field.set(None);
                                                    detail_field_popup_message.set(None);
                                                }
                                            },
                                            onfocusout: move |_| {
                                                detail_spindle_speed_draft
                                                    .set(detail_spindle_speed_rpm.read().clone());
                                                detail_spindle_speed_is_editing.set(false);
                                                if let Some(next) = *detail_pending_focus_field.read() {
                                                    if next != StockDetailField::SpindleSpeed {
                                                        match next {
                                                            StockDetailField::Diameter => {
                                                                detail_diameter_is_editing.set(true);
                                                                detail_diameter_draft
                                                                    .set(detail_diameter_mm.read().clone());
                                                            }
                                                            StockDetailField::PointAngle => {
                                                                detail_point_angle_is_editing.set(true);
                                                                detail_point_angle_draft
                                                                    .set(detail_point_angle_degrees.read().clone());
                                                            }
                                                            StockDetailField::FeedRate => {
                                                                detail_feed_rate_is_editing.set(true);
                                                                detail_feed_rate_draft
                                                                    .set(detail_feed_rate_mm_per_min.read().clone());
                                                            }
                                                            StockDetailField::SpindleSpeed => {}
                                                        }
                                                        detail_field_popup_message.set(None);
                                                    }
                                                }
                                                detail_pending_focus_field.set(None);
                                            },
                                        }
                                    } else {
                                        button {
                                            r#type: "button",
                                            class: "stock-detail-input stock-detail-trigger",
                                            onmousedown: move |_| {
                                                detail_pending_focus_field.set(Some(StockDetailField::SpindleSpeed));
                                                detail_spindle_speed_is_editing.set(true);
                                                detail_spindle_speed_draft
                                                    .set(detail_spindle_speed_rpm.read().clone());
                                                detail_field_popup_message.set(None);
                                            },
                                            onclick: move |_| {
                                                detail_pending_focus_field.set(None);
                                                detail_spindle_speed_is_editing.set(true);
                                                detail_spindle_speed_draft
                                                    .set(detail_spindle_speed_rpm.read().clone());
                                                detail_field_popup_message.set(None);
                                            },
                                            if detail_spindle_speed_display.is_empty() {
                                                "Optional"
                                            } else {
                                                "{detail_spindle_speed_display}"
                                            }
                                        }

                                        if detail_spindle_speed_is_modified {
                                            if let Some(original_value) = detail_spindle_speed_original_display.clone() {
                                                div { class: "stock-detail-original-group",
                                                    span { class: "stock-detail-original-value",
                                                        "{original_value}"
                                                    }
                                                    button {
                                                        r#type: "button",
                                                        class: "stock-detail-revert-btn",
                                                        title: "Revert to catalog value",
                                                        onclick: {
                                                            let tool_id = tool.id.clone();
                                                            let original_spindle_speed = tool.catalog_spindle_speed;
                                                            move |_| {
                                                                if let Some(original_spindle_speed) = original_spindle_speed {
                                                                    let normalized = unit_service::format_rotational_speed_edit_display(
                                                                        original_spindle_speed,
                                                                    );
                                                                    detail_spindle_speed_rpm.set(normalized.clone());
                                                                    detail_spindle_speed_draft.set(normalized);
                                                                    detail_spindle_speed_is_editing.set(false);
                                                                    detail_pending_focus_field.set(None);
                                                                    detail_field_popup_message.set(None);
                                                                    state
                                                                        .with_mut(|ui_state| {
                                                                            if let Some(target) = ui_state
                                                                                .ui
                                                                                .tools
                                                                                .iter_mut()
                                                                                .find(|entry| entry.id == tool_id)
                                                                            {
                                                                                target.spindle_speed = Some(original_spindle_speed);
                                                                            }
                                                                        });
                                                                    persist_stock_realm_now(state);
                                                                }
                                                            }
                                                        },
                                                        "↺"
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            div { class: "stock-detail-row",
                                div { class: "stock-detail-label", "Status" }
                                select {
                                    class: "stock-detail-input",
                                    value: detail_status.read().clone(),
                                    onchange: move |evt| {
                                        let value = evt.value();
                                        detail_status.set(value.clone());
                                        if let Some(tool_id) = detail_tool_id.read().clone() {
                                            state
                                                .with_mut(|ui_state| {
                                                    if let Some(target) = ui_state
                                                        .ui
                                                        .tools
                                                        .iter_mut()
                                                        .find(|entry| entry.id == tool_id)
                                                    {
                                                        target.status = tool_status_from_value(&value);
                                                    }
                                                });
                                            persist_stock_realm_now(state);
                                        }
                                    },
                                    option { value: "in-stock", "In stock" }
                                    option { value: "out-of-stock", "Out of stock" }
                                }
                            }
                            div { class: "stock-detail-row",
                                div { class: "stock-detail-label", "Preference" }
                                select {
                                    class: "stock-detail-input",
                                    value: detail_preference.read().clone(),
                                    onchange: move |evt| {
                                        let value = evt.value();
                                        detail_preference.set(value.clone());
                                        if let Some(tool_id) = detail_tool_id.read().clone() {
                                            state
                                                .with_mut(|ui_state| {
                                                    if let Some(target) = ui_state
                                                        .ui
                                                        .tools
                                                        .iter_mut()
                                                        .find(|entry| entry.id == tool_id)
                                                    {
                                                        target.preference = tool_preference_from_value(&value);
                                                    }
                                                });
                                            persist_stock_realm_now(state);
                                        }
                                    },
                                    option { value: "preferred", "Preferred" }
                                    option { value: "neutral", "Neutral" }
                                    option { value: "not-preferred", "Not preferred" }
                                }
                            }
                            div { class: "stock-detail-row",
                                div { class: "stock-detail-label", "Source catalog" }
                                div { class: "stock-detail-readonly", "{detail_source_catalog.read()}" }
                            }
                            div { class: "stock-detail-row",
                                div { class: "stock-detail-label", "Manufacturer" }
                                div { class: "stock-detail-readonly", "{detail_manufacturer.read()}" }
                            }
                            div { class: "stock-detail-row",
                                div { class: "stock-detail-label", "SKU" }
                                div { class: "stock-detail-readonly", "{detail_sku.read()}" }
                            }
                            div { class: "stock-detail-row",
                                div { class: "stock-detail-label", "Tool ID" }
                                div { class: "stock-detail-readonly", "{tool.id}" }
                            }
                        }
                    }
                }
            } else if snapshot.tools.is_empty() {
                div { class: "empty-state",
                    p { "No tools in stock." }
                    p { "Add tools from catalogs using the button above." }
                }
            } else {
                div { class: "table-wrap stock-table-wrap",
                    table {
                        thead {
                            tr {
                                th {
                                    input {
                                        r#type: "checkbox",
                                        checked: all_visible_selected,
                                        disabled: visible_tool_ids.is_empty(),
                                        oninput: {
                                            let visible_tool_ids = visible_tool_ids.clone();
                                            move |evt: FormEvent| {
                                                let checked = evt.checked();
                                                selected_stock_tool_ids
                                                    .with_mut(|selected| {
                                                        if checked {
                                                            for tool_id in &visible_tool_ids {
                                                                selected.insert(tool_id.clone());
                                                            }
                                                        } else {
                                                            for tool_id in &visible_tool_ids {
                                                                selected.remove(tool_id);
                                                            }
                                                        }
                                                    });
                                            }
                                        },
                                    }
                                }
                                th { "Type" }
                                th { "Diameter" }
                                th { "Name" }
                                th { "Source catalog" }
                                th { "Preference" }
                                if has_atc {
                                    th { "ATC" }
                                }
                                th { "Status" }
                            }
                        }
                        tbody {
                            {
                                filtered_tools
                                    .iter()
                                    .map(|(_, tool)| {
                                        let tool_id = tool.id.clone();
                                        let tool_for_detail = (*tool).clone();
                                        let is_selected = selected_stock_tool_ids
                                            .read()
                                            .contains(tool_id.as_str());
                                        let atc_slot = snapshot
                                            .rack_slots
                                            .iter()
                                            .find(|(_, slot)| slot.tool_id.as_ref() == Some(&tool_id))
                                            .map(|(slot_num, _)| *slot_num);
                                        rsx! {
                                            tr {
                                                key: "{tool_id}",
                                                class: if is_selected { "stock-row selected" } else { "stock-row" },
                                                ondoubleclick: {
                                                    let tool_for_detail = tool_for_detail.clone();
                                                    move |_| {
                                                        load_tool_editor(
                                                            &tool_for_detail,
                                                            unit_system,
                                                            detail_tool_id,
                                                            detail_composite_name,
                                                            detail_custom_name,
                                                            detail_kind,
                                                            detail_diameter_mm,
                                                            detail_diameter_is_editing,
                                                            detail_diameter_draft,
                                                            detail_point_angle_degrees,
                                                            detail_point_angle_is_editing,
                                                            detail_point_angle_draft,
                                                            detail_feed_rate_mm_per_min,
                                                            detail_feed_rate_is_editing,
                                                            detail_feed_rate_draft,
                                                            detail_spindle_speed_rpm,
                                                            detail_spindle_speed_is_editing,
                                                            detail_spindle_speed_draft,
                                                            detail_source_catalog,
                                                            detail_manufacturer,
                                                            detail_sku,
                                                            detail_status,
                                                            detail_preference,
                                                        );
                                                    }
                                                },
                                                td {
                                                    input {
                                                        r#type: "checkbox",
                                                        checked: is_selected,
                                                        oninput: {
                                                            let tool_id = tool_id.clone();
                                                            move |evt: FormEvent| {
                                                                let checked = evt.checked();
                                                                selected_stock_tool_ids
                                                                    .with_mut(|selected| {
                                                                        if checked {
                                                                            selected.insert(tool_id.clone());
                                                                        } else {
                                                                            selected.remove(&tool_id);
                                                                        }
                                                                    });
                                                            }
                                                        },
                                                    }
                                                }
                                                td {
                                                    span { class: "tool-type-chip {stock_tool_type_class(&tool.kind)}",
                                                        "{stock_tool_type_label(&tool.kind)}"
                                                    }
                                                }
                                                td { "{tool_diameter(tool, unit_system)}" }
                                                td { class: "stock-name-cell", "{tool.display_name()}" }
                                                td { "{tool.source_catalog}" }
                                                td {
                                                    span { class: "status-chip {tool.preference.class_name()}", "{tool.preference.label()}" }
                                                }
                                                if has_atc {
                                                    td {
                                                        if let Some(slot_num) = atc_slot {
                                                            span { class: "atc-indicator",
                                                                span { class: "atc-dot" }
                                                                span { "T{slot_num}" }
                                                            }
                                                        } else {
                                                            span { class: "atc-empty", "-" }
                                                        }
                                                    }
                                                }
                                                td {
                                                    select {
                                                        class: "stock-inline-select {tool.status.class_name()}",
                                                        value: tool_status_value(tool.status),
                                                        onchange: {
                                                            let tool_id = tool_id.clone();
                                                            move |evt| {
                                                                let value = evt.value();
                                                                state
                                                                    .with_mut(|s| {
                                                                        if let Some(target) = s
                                                                            .ui
                                                                            .tools
                                                                            .iter_mut()
                                                                            .find(|entry| entry.id == tool_id)
                                                                        {
                                                                            target.status = tool_status_from_value(&value);
                                                                        }
                                                                    });
                                                                persist_stock_realm_now(state);
                                                            }
                                                        },
                                                        option { value: "in-stock", "In stock" }
                                                        option { value: "out-of-stock", "Out of stock" }
                                                    }
                                                }
                                            }
                                        }
                                    })
                            }
                        }
                    }
                }

                if filtered_tools_is_empty {
                    div { class: "empty-state",
                        p { "No tools match the current filter." }
                        p { "Try a broader search term or clear the filter." }
                    }
                }
            }
        }
    }
}

fn load_tool_editor(
    tool: &Tool,
    unit_system: UnitSystem,
    mut detail_tool_id: Signal<Option<String>>,
    mut detail_composite_name: Signal<String>,
    mut detail_custom_name: Signal<String>,
    mut detail_kind: Signal<String>,
    mut detail_diameter_mm: Signal<String>,
    mut detail_diameter_is_editing: Signal<bool>,
    mut detail_diameter_draft: Signal<String>,
    mut detail_point_angle_degrees: Signal<String>,
    mut detail_point_angle_is_editing: Signal<bool>,
    mut detail_point_angle_draft: Signal<String>,
    mut detail_feed_rate_mm_per_min: Signal<String>,
    mut detail_feed_rate_is_editing: Signal<bool>,
    mut detail_feed_rate_draft: Signal<String>,
    mut detail_spindle_speed_rpm: Signal<String>,
    mut detail_spindle_speed_is_editing: Signal<bool>,
    mut detail_spindle_speed_draft: Signal<String>,
    mut detail_source_catalog: Signal<String>,
    mut detail_manufacturer: Signal<String>,
    mut detail_sku: Signal<String>,
    mut detail_status: Signal<String>,
    mut detail_preference: Signal<String>,
) {
    detail_tool_id.set(Some(tool.id.clone()));
    detail_composite_name.set(tool.composite_name.clone());
    detail_custom_name.set(tool.name.clone());
    detail_kind.set(tool.kind.clone());
    let diameter_display = unit_service::format_length_edit_display(tool.diameter, unit_system);
    detail_diameter_mm.set(diameter_display.clone());
    detail_diameter_draft.set(diameter_display);
    detail_diameter_is_editing.set(false);
    let point_angle_display = unit_service::format_angle_edit_display(tool.point_angle);
    detail_point_angle_degrees.set(point_angle_display.clone());
    detail_point_angle_draft.set(point_angle_display);
    detail_point_angle_is_editing.set(false);
    let feed_rate_display = tool
        .feed_rate
        .map(|value| format_feed_rate_edit_display(value, unit_system))
        .unwrap_or_default();
    detail_feed_rate_mm_per_min.set(feed_rate_display.clone());
    detail_feed_rate_draft.set(feed_rate_display);
    detail_feed_rate_is_editing.set(false);
    let spindle_speed_display = tool
        .spindle_speed
        .map(unit_service::format_rotational_speed_edit_display)
        .unwrap_or_default();
    detail_spindle_speed_rpm.set(spindle_speed_display.clone());
    detail_spindle_speed_draft.set(spindle_speed_display);
    detail_spindle_speed_is_editing.set(false);
    detail_source_catalog.set(tool.source_catalog.clone());
    detail_manufacturer.set(tool.manufacturer.clone().unwrap_or_default());
    detail_sku.set(tool.sku.clone().unwrap_or_default());
    detail_status.set(tool_status_value(tool.status).to_string());
    detail_preference.set(tool_preference_value(tool.preference).to_string());
}

fn parse_optional_feed_rate(
    value: &str,
    unit_system: UnitSystem,
    label: &str,
) -> Result<Option<FeedRate>, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    unit_service::parse_feed_with_preference(trimmed, unit_system)
        .map(Some)
        .map_err(|_| format!("{} must be a valid feed rate", label))
}

fn length_input_has_explicit_unit(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return false;
    }

    // Detect explicit unit-bearing values like "3/4in", "1 mm", or 1".
    trimmed.chars().any(|ch| ch.is_ascii_alphabetic() || ch == '"')
}

fn feed_rate_input_has_explicit_unit(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return false;
    }

    trimmed.chars().any(|ch| ch.is_ascii_alphabetic() || ch == '/')
}

fn parse_length_for_display_input(value: &str, unit_system: UnitSystem) -> Result<Length, String> {
    unit_service::parse_length_with_preference(value, unit_system)
        .map_err(|_| "Diameter must be a valid length".to_string())
}

fn default_length_unit_suffix(unit_system: UnitSystem) -> &'static str {
    unit_service::default_length_suffix(unit_system)
}

fn default_feed_unit_suffix(unit_system: UnitSystem) -> &'static str {
    unit_service::default_feed_suffix(unit_system)
}

fn option_feed_rate_changed(current: Option<FeedRate>, original: Option<FeedRate>) -> bool {
    match original {
        Some(original) => match current {
            Some(current) => (current.as_mm_per_min() - original.as_mm_per_min()).abs() > 1e-9,
            None => true,
        },
        None => false,
    }
}

fn option_spindle_speed_changed(current: Option<RotationalSpeed>, original: Option<RotationalSpeed>) -> bool {
    match original {
        Some(original) => match current {
            Some(current) => (current.as_rpm() - original.as_rpm()).abs() > 1e-9,
            None => true,
        },
        None => false,
    }
}

fn format_length_for_user(length: Length, unit_system: UnitSystem) -> String {
    unit_service::format_length_display(length, unit_system)
}

fn format_length_field_display(raw_value: &str, unit_system: UnitSystem) -> String {
    let trimmed = raw_value.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let Ok(length) = parse_length_for_display_input(trimmed, unit_system) else {
        return trimmed.to_string();
    };

    format_length_for_user(length, unit_system)
}

fn format_angle_field_display(raw_value: &str) -> String {
    let trimmed = raw_value.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let Ok(angle) = unit_service::parse_angle(trimmed) else {
        return trimmed.to_string();
    };

    unit_service::format_angle_display(angle)
}

fn format_rotational_speed_field_display(raw_value: &str) -> String {
    let trimmed = raw_value.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let Ok(speed) = unit_service::parse_rotational_speed(trimmed) else {
        return trimmed.to_string();
    };

    unit_service::format_rotational_speed_display(speed)
}

fn format_feed_rate_edit_display(feed_rate: FeedRate, unit_system: UnitSystem) -> String {
    unit_service::format_feed_edit_display(feed_rate, unit_system)
}

fn format_feed_rate_for_user(feed_rate: FeedRate, unit_system: UnitSystem) -> String {
    unit_service::format_feed_display(feed_rate, unit_system)
}

fn format_feed_rate_field_display(raw_value: &str, unit_system: UnitSystem) -> String {
    let trimmed = raw_value.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let Ok(Some(feed_rate)) = parse_optional_feed_rate(
        trimmed,
        unit_system,
        "Feed rate",
    ) else {
        return trimmed.to_string();
    };

    format_feed_rate_for_user(feed_rate, unit_system)
}

fn tool_preference_value(preference: ToolPreference) -> &'static str {
    match preference {
        ToolPreference::Preferred => "preferred",
        ToolPreference::Neutral => "neutral",
        ToolPreference::NotPreferred => "not-preferred",
    }
}

fn tool_status_value(status: ToolStatus) -> &'static str {
    match status {
        ToolStatus::InStock => "in-stock",
        ToolStatus::OutOfStock => "out-of-stock",
    }
}

fn tool_status_from_value(value: &str) -> ToolStatus {
    match value {
        "out-of-stock" => ToolStatus::OutOfStock,
        _ => ToolStatus::InStock,
    }
}

fn tool_preference_from_value(value: &str) -> ToolPreference {
    match value {
        "preferred" => ToolPreference::Preferred,
        "not-preferred" => ToolPreference::NotPreferred,
        _ => ToolPreference::Neutral,
    }
}

fn stock_tool_type_label(kind: &str) -> &'static str {
    let normalized = kind.trim().to_ascii_lowercase();

    if normalized.contains("drill") {
        "Drill"
    } else if normalized.contains("engrav") {
        "Engraving"
    } else if normalized.contains("v-bit") || normalized == "v" || normalized.starts_with('v') {
        "V-bit"
    } else {
        "Router"
    }
}

fn stock_tool_type_class(kind: &str) -> &'static str {
    match stock_tool_type_label(kind) {
        "Drill" => "tool-type-drill",
        "Router" => "tool-type-router",
        "V-bit" => "tool-type-vbit",
        "Engraving" => "tool-type-engraving",
        _ => "tool-type-router",
    }
}

fn stock_tool_type_rank(kind: &str) -> u8 {
    match stock_tool_type_label(kind) {
        "Drill" => 0,
        "Router" => 1,
        "V-bit" => 2,
        "Engraving" => 3,
        _ => 4,
    }
}

fn tool_diameter(tool: &Tool, unit_system: UnitSystem) -> String {
    unit_service::format_length_display(tool.diameter, unit_system)
}

fn catalog_tool_type(tool: &CatalogStockTool) -> &'static str {
    if tool.kind.eq_ignore_ascii_case("drill") {
        return "Drill";
    }

    let lower_name = tool.display_name.to_ascii_lowercase();
    if lower_name.contains("v-bit") || lower_name.starts_with('v') {
        "V-bit"
    } else if lower_name.contains("engrav") {
        "Engraving"
    } else if lower_name.contains("mill") || lower_name.contains("end") {
        "Router"
    } else {
        "Router"
    }
}

fn catalog_tool_diameter(tool: &CatalogStockTool, unit_system: UnitSystem) -> String {
    unit_service::format_length_display(tool.diameter, unit_system)
}

fn persist_stock_realm_now(state: Signal<crate::ctx::AppCtx>) {
    let snapshot = state.read().clone();
    sync_ctx_from_ui_state_and_persist_realms(&snapshot.ui, &[PersistRealm::Stock]);
}

fn stock_fingerprint(tools: &[Tool]) -> String {
    let mut out = String::new();
    for tool in tools {
        let feed_rate = tool
            .feed_rate
            .map(|value| value.as_mm_per_min().to_string())
            .unwrap_or_else(|| "null".to_string());
        let spindle_speed = tool
            .spindle_speed
            .map(|value| value.as_rpm().to_string())
            .unwrap_or_else(|| "null".to_string());
        let manufacturer = tool.manufacturer.as_deref().unwrap_or_default();
        let sku = tool.sku.as_deref().unwrap_or_default();

        out.push_str(&format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}\n",
            tool.id,
            tool.composite_name,
            tool.name,
            tool.kind,
            tool.diameter.as_mm(),
            tool.point_angle.as_degrees(),
            feed_rate,
            spindle_speed,
            tool.status.label(),
            tool.preference.label(),
            tool.source_catalog,
            manufacturer,
            sku,
        ));
    }
    out
}


use dioxus::prelude::*;
use std::collections::BTreeSet;

use crate::runtime::ctx_snapshot;
use crate::ui::bindings::{StockField, StockForm};
use units::user_format as unit_format;

use crate::data::model::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum StockSortMode {
    RecentFirst,
    Type,
    SizeAscending,
    SizeDescending,
    Status,
    Preference,
    SourceCatalog,
}

impl StockSortMode {
    fn from_value(value: &str) -> Self {
        match value {
            "type" => Self::Type,
            "size_asc" => Self::SizeAscending,
            "size_desc" => Self::SizeDescending,
            "status" => Self::Status,
            "preference" => Self::Preference,
            "source_catalog" => Self::SourceCatalog,
            _ => Self::RecentFirst,
        }
    }

    fn value(self) -> &'static str {
        match self {
            Self::RecentFirst => "recent",
            Self::Type => "type",
            Self::SizeAscending => "size_asc",
            Self::SizeDescending => "size_desc",
            Self::Status => "status",
            Self::Preference => "preference",
            Self::SourceCatalog => "source_catalog",
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
pub fn StockScreen(state: Signal<crate::runtime::AppCtx>) -> Element {
    use_effect(move || {
        super::mutate_ctx(state, |s| s.ensure_catalogs_loaded());
    });

    // AppData owns stock.yaml. The detail editor writes tool fields directly into
    // the datastore singleton via StockField/StockForm, bumping the store
    // revision; mirror those changes back into the legacy in-memory `tools` (the
    // table's source) so table and detail stay coherent. Structural ops
    // (add/clone/remove) persist through their own AppData path and update the
    // local signal directly, so no fingerprint watcher is needed.
    use_effect(move || {
        let _ = crate::ui::bindings::data_revision();
        crate::ui::bindings::refresh_legacy_stock();
        state.set(ctx_snapshot());
    });

    let snapshot = state.read().clone();
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

    // The stock detail panel edits the AppData singleton directly (StockForm /
    // StockField over `/tools/{i}/…`), so it needs only the selected tool's id;
    // the old ~15 buffered editing signals are gone.
    let mut detail_tool_id = use_signal(|| None::<String>);

    let selected_catalog_count = selected_catalog_tool_keys.read().len();
    let selected_stock_count = selected_stock_tool_ids.read().len();
    let selected_stock_tool_ids_vec = selected_stock_tool_ids
        .read()
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    let selected_has_any_reference = selected_stock_tool_ids_vec
        .iter()
        .any(|tool_id| snapshot.is_uuid_referenced(tool_id));
    let delete_current_job_reference_warnings = selected_stock_tool_ids_vec
        .iter()
        .flat_map(|tool_id| snapshot.current_job_reference_locations_for_uuid(tool_id))
        .collect::<Vec<_>>();
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
        StockSortMode::SizeAscending => filtered_tools.sort_by(|left, right| {
            left.1
                .diameter
                .as_mm()
                .partial_cmp(&right.1.diameter.as_mm())
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| right.0.cmp(&left.0))
        }),
        StockSortMode::SizeDescending => filtered_tools.sort_by(|left, right| {
            right.1
                .diameter
                .as_mm()
                .partial_cmp(&left.1.diameter.as_mm())
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| right.0.cmp(&left.0))
        }),
        StockSortMode::Status => filtered_tools.sort_by(|left, right| {
            stock_tool_status_rank(left.1.status)
                .cmp(&stock_tool_status_rank(right.1.status))
                .then_with(|| right.0.cmp(&left.0))
        }),
        StockSortMode::Preference => filtered_tools.sort_by(|left, right| {
            stock_tool_preference_rank(left.1.preference)
                .cmp(&stock_tool_preference_rank(right.1.preference))
                .then_with(|| right.0.cmp(&left.0))
        }),
        StockSortMode::SourceCatalog => filtered_tools.sort_by(|left, right| {
            left.1
                .source_catalog
                .to_ascii_lowercase()
                .cmp(&right.1.source_catalog.to_ascii_lowercase())
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

    // The selected tool and its position in the AppData `/tools` array (kept in
    // step with `snapshot.tools` by the refresh effect), used to address the
    // schema-driven detail form at `/tools/{active_index}/…`.
    let active_index = detail_tool_id
        .read()
        .clone()
        .and_then(|tool_id| snapshot.tools.iter().position(|tool| tool.id == tool_id));
    let active_tool = active_index.map(|index| snapshot.tools[index].clone());

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
                            option { value: "size_asc", "Size: small to large" }
                            option { value: "size_desc", "Size: large to small" }
                            option { value: "status", "Sort by stock status" }
                            option { value: "preference", "Sort by preference" }
                            option { value: "source_catalog", "Sort by source catalog" }
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
                                    let added = crate::ui::bindings::add_stock_from_catalog(&selected);
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
                            "Delete {selected_stock_count} selected tool(s)? Broken references are allowed and must be repaired in the active job."
                        }
                        if selected_has_any_reference {
                            p { class: "diag-status",
                                "Warning: one or more selected tools are referenced by existing profiles or job settings."
                            }
                        }
                        if !delete_current_job_reference_warnings.is_empty() {
                            p { class: "diag-status",
                                "Warning: one or more selected tools are used by the current job:"
                            }
                            ul { class: "diag-status",
                                for (idx , location) in delete_current_job_reference_warnings.iter().enumerate() {
                                    li { key: "delete-warning-{idx}", "{location}" }
                                }
                            }
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
                                    let removed = crate::ui::bindings::remove_stock_tools(&selected);
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

            if let (Some(index), Some(tool)) = (active_index, active_tool.as_ref()) {
                div { class: "stock-detail-page",
                    div { class: "panel stock-detail-panel",
                        div { class: "panel-header",
                            div {
                                h3 { "Tool detail" }
                                p { "Edit the tool properties directly, or clone the tool." }
                            }
                            div { class: "actions",
                                button {
                                    class: "btn btn-secondary",
                                    onclick: move |_| detail_tool_id.set(None),
                                    "Back"
                                }
                                button {
                                    class: "btn btn-secondary",
                                    onclick: move |_| {
                                        if let Some(new_id) = crate::ui::bindings::clone_stock_tool(index) {
                                            detail_tool_id.set(Some(new_id));
                                            stock_feedback.set("Cloned tool".to_string());
                                        }
                                    },
                                    "Clone Tool"
                                }
                                button {
                                    class: "btn btn-secondary",
                                    title: "Reset every edited field back to its original catalog value",
                                    onclick: move |_| {
                                        crate::ui::bindings::revert_stock_tool(index);
                                        stock_feedback.set("Reverted tool to catalog values".to_string());
                                    },
                                    "Revert to catalog"
                                }
                            }
                        }

                        // Schema-driven tool editor over the AppData stock singleton.
                        // Edits write to `overrides` (`/tools/{index}/overrides/…`);
                        // `base` stays the immutable catalog original. A field that
                        // differs from base shows an orange revert control (see
                        // `field_widget`). Edits persist straight to the datastore and
                        // the table refreshes via the store-revision effect.
                        div { class: "stock-detail-form",
                            div { class: "field",
                                label { "Source catalog" }
                                div { class: "stock-detail-readonly", "{tool.source_catalog}" }
                            }
                            StockForm { ptr: format!("/tools/{index}/overrides") }
                            StockField { ptr: format!("/tools/{index}/availability") }
                            StockField { ptr: format!("/tools/{index}/preference") }
                            div { class: "field",
                                label { "Tool ID" }
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
                                    .map(|(original_index, tool)| {
                                        // Position in `snapshot.tools` == the AppData
                                        // `/tools` array index (kept in step by the
                                        // refresh effect), used to address inline edits.
                                        let row_index = *original_index;
                                        let tool_id = tool.id.clone();
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
                                                    let tool_id = tool_id.clone();
                                                    move |_| detail_tool_id.set(Some(tool_id.clone()))
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
                                                        onchange: move |evt| {
                                                            crate::ui::bindings::set_stock_availability(
                                                                row_index,
                                                                evt.value() == "in-stock",
                                                            );
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

fn tool_status_value(status: ToolStatus) -> &'static str {
    match status {
        ToolStatus::InStock => "in-stock",
        ToolStatus::OutOfStock => "out-of-stock",
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

fn stock_tool_status_rank(status: ToolStatus) -> u8 {
    match status {
        ToolStatus::InStock => 0,
        ToolStatus::OutOfStock => 1,
    }
}

fn stock_tool_preference_rank(preference: ToolPreference) -> u8 {
    match preference {
        ToolPreference::Preferred => 0,
        ToolPreference::Neutral => 1,
        ToolPreference::NotPreferred => 2,
    }
}

fn tool_diameter(tool: &Tool, unit_system: UserUnitSystem) -> String {
    unit_format::format_length_display(tool.diameter, unit_system)
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

fn catalog_tool_diameter(tool: &CatalogStockTool, unit_system: UserUnitSystem) -> String {
    unit_format::format_length_display(tool.diameter, unit_system)
}


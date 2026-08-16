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
    // Where each tool is pinned across *every* rack, not just one machine's: a tool may
    // be expected in several changers at once, and a stock row that shows one slot for a
    // machine it never names cannot say which. Computed once for the table, then indexed
    // per row.
    let pinning = crate::runtime::tooling::pinned_rack_slots(&snapshot);
    let has_atc = pinning.rack_count > 0;
    let unit_system = snapshot.unit_system;

    let mut show_catalog_picker = use_signal(|| false);
    let mut selected_catalog_tool_keys = use_signal(|| BTreeSet::<String>::new());
    // One end of a shift-range in the catalog picker: the last tool clicked *without*
    // shift, held as `(section key, tool key)`.
    //
    // The section travels with it so a range can only ever run inside the section that
    // anchored it — see `catalog_click_range`. It moves on a plain click and stays put on
    // a shift-click, so repeated shift-clicks grow and shrink one run from a fixed end
    // rather than walking the anchor along behind the cursor.
    let mut catalog_anchor = use_signal(|| None::<(String, String)>);
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
                                catalog_anchor.set(None);
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
                                p {
                                    "Click a tool to select it. Shift-click to take the \
                                     whole run between it and your last click, or use a \
                                     section's header box to take the section."
                                }
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
                                        {
                                        // This section's tool keys in display order — the
                                        // run a shift-click slices out of. Behind an `Rc`
                                        // because every row's handler needs to own a copy,
                                        // and cloning the whole `Vec` per row would be
                                        // quadratic in a section's length on every render.
                                        let section_keys: std::rc::Rc<Vec<String>> =
                                            std::rc::Rc::new(
                                                section.tools.iter().map(|t| t.key.clone()).collect(),
                                            );
                                        let section_key = section.key.clone();
                                        let section_selected = section_keys
                                            .iter()
                                            .filter(|key| {
                                                selected_catalog_tool_keys.read().contains(key.as_str())
                                            })
                                            .count();
                                        // Not tri-state: nothing in the theme styles an
                                        // indeterminate box, so a part-selected section
                                        // reads as unchecked and takes the rest on click.
                                        let whole_section_selected = !section_keys.is_empty()
                                            && section_selected == section_keys.len();
                                        rsx! {
                                        details {
                                            key: "{section.key}",
                                            class: "catalog-node section-node",
                                            summary { class: "catalog-node-summary",
                                                "{section.name} ({section.tools.len()} tools)"
                                            }

                                            div { class: "catalog-tool-list",
                                                div { class: "catalog-tool-header",
                                                    // Lands in the grid's first column,
                                                    // which the header left empty over the
                                                    // rows' checkboxes (its labels are
                                                    // pinned to columns 2-4 in the theme).
                                                    input {
                                                        r#type: "checkbox",
                                                        checked: whole_section_selected,
                                                        oninput: {
                                                            let section_keys = section_keys.clone();
                                                            move |evt: FormEvent| {
                                                                let checked = evt.checked();
                                                                selected_catalog_tool_keys
                                                                    .with_mut(|selected| {
                                                                        for key in section_keys.iter() {
                                                                            if checked {
                                                                                selected.insert(key.clone());
                                                                            } else {
                                                                                selected.remove(key);
                                                                            }
                                                                        }
                                                                    });
                                                            }
                                                        },
                                                    }
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
                                                    // A `div` with its own click handler
                                                    // rather than a `label` wrapping the
                                                    // box: `FormEvent` carries no modifiers,
                                                    // and a label's click both fires its own
                                                    // handler and activates the checkbox, so
                                                    // a shift-click would be handled twice.
                                                    // The box below is now an indicator —
                                                    // CSS passes clicks through it to here.
                                                    div {
                                                        key: "{tool.key}",
                                                        class: if selected_catalog_tool_keys.read().contains(&tool.key) {
                                                            "catalog-tool-row selected"
                                                        } else {
                                                            "catalog-tool-row"
                                                        },
                                                        onclick: {
                                                            let tool_key = tool.key.clone();
                                                            let section_key = section_key.clone();
                                                            let section_keys = section_keys.clone();
                                                            move |evt: Event<MouseData>| {
                                                                let shift = evt.modifiers().shift();
                                                                let anchor = catalog_anchor.read().clone();
                                                                // Only an anchor from *this*
                                                                // section can start a run.
                                                                let anchor_here = anchor
                                                                    .as_ref()
                                                                    .filter(|(sec, _)| sec == &section_key)
                                                                    .map(|(_, key)| key.clone());
                                                                let keys = catalog_click_range(
                                                                    &section_keys,
                                                                    anchor_here.as_deref(),
                                                                    &tool_key,
                                                                    shift,
                                                                );
                                                                // The clicked row decides
                                                                // the whole run's fate, so
                                                                // one gesture both fills a
                                                                // range and empties one.
                                                                let select = !selected_catalog_tool_keys
                                                                    .read()
                                                                    .contains(&tool_key);
                                                                selected_catalog_tool_keys
                                                                    .with_mut(|selected| {
                                                                        for key in &keys {
                                                                            if select {
                                                                                selected.insert(key.clone());
                                                                            } else {
                                                                                selected.remove(key);
                                                                            }
                                                                        }
                                                                    });
                                                                // Re-anchor unless a run was
                                                                // actually taken, so a shift
                                                                // that had nothing to extend
                                                                // still leaves an end to
                                                                // extend from next time.
                                                                if keys.len() == 1 {
                                                                    catalog_anchor.set(Some((
                                                                        section_key.clone(),
                                                                        tool_key.clone(),
                                                                    )));
                                                                }
                                                            }
                                                        },
                                                        input {
                                                            r#type: "checkbox",
                                                            checked: selected_catalog_tool_keys.read().contains(&tool.key),
                                                            // Out of the tab order along
                                                            // with the handler: focused, it
                                                            // would toggle on Space and then
                                                            // be reverted by the next render,
                                                            // since the row's click is the
                                                            // only thing that writes the
                                                            // selection. A control that
                                                            // visibly does nothing is worse
                                                            // than one that is not offered.
                                                            // The section's header box stays
                                                            // reachable and does work.
                                                            tabindex: "-1",
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
                                    let outcome = crate::ui::bindings::add_stock_from_catalog(&selected);
                                    stock_feedback.set(describe_addition(outcome));
                                    selected_catalog_tool_keys.set(BTreeSet::new());
                                    catalog_anchor.set(None);
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
                                    th {
                                        title: "The slot each machine's rack pins this tool to, one entry per rack",
                                        "ATC"
                                    }
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
                                        // One `Tn` per rack that pins this tool, in the
                                        // same order for every row; the title names the
                                        // machine each belongs to.
                                        let atc_slots = pinning.slots_label(&tool_id);
                                        let atc_detail = pinning.detail(&tool_id);
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
                                                    // Editable in the row, like status beside it: both decide
                                                    // whether the planner may pick this tool, and opening the
                                                    // detail view to change one of them was the odd rule out.
                                                    select {
                                                        class: "stock-inline-select {tool.preference.class_name()}",
                                                        value: tool_preference_value(tool.preference),
                                                        // The row's double-click opens the tool; without this,
                                                        // using the control would also open it.
                                                        ondoubleclick: move |evt| evt.stop_propagation(),
                                                        onchange: move |evt| {
                                                            crate::ui::bindings::set_stock_preference(row_index, &evt.value());
                                                        },
                                                        option { value: "preferred", "Preferred" }
                                                        option { value: "neutral", "Neutral" }
                                                        option { value: "not_preferred", "Not preferred" }
                                                    }
                                                }
                                                if has_atc {
                                                    td {
                                                        if atc_slots.is_empty() {
                                                            span { class: "atc-empty", "-" }
                                                        } else {
                                                            span { class: "atc-indicator", title: "{atc_detail}",
                                                                span { class: "atc-dot" }
                                                                span { "{atc_slots}" }
                                                            }
                                                        }
                                                    }
                                                }
                                                td {
                                                    select {
                                                        class: "stock-inline-select {tool.status.class_name()}",
                                                        value: tool_status_value(tool.status),
                                                        ondoubleclick: move |evt| evt.stop_propagation(),
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

/// What a bulk add did, in words.
///
/// The count alone was misleading: picking five tools already in stock reported
/// "Added 0 tool(s)", which reads as a broken button rather than as the picker
/// declining to give you a second copy of what you own.
fn describe_addition(outcome: crate::ui::bindings::StockAddition) -> String {
    match (outcome.added, outcome.skipped) {
        (0, 0) => "Nothing to add".to_string(),
        (0, skipped) => format!("{skipped} tool(s) already in stock — nothing added"),
        (added, 0) => format!("Added {added} tool(s) from catalogs"),
        (added, skipped) => {
            format!("Added {added} tool(s) — {skipped} already in stock")
        }
    }
}

fn tool_status_value(status: ToolStatus) -> &'static str {
    match status {
        ToolStatus::InStock => "in-stock",
        ToolStatus::OutOfStock => "out-of-stock",
    }
}

/// The `<option>` value for a preference — the schema's own storage key, so the value
/// the DOM carries is the value written. Status has three spellings of one enum
/// (`in_stock` stored, `in-stock` in the DOM, "In stock" shown); this adds no fourth.
fn tool_preference_value(preference: ToolPreference) -> &'static str {
    match preference {
        ToolPreference::Preferred => "preferred",
        ToolPreference::Neutral => "neutral",
        ToolPreference::NotPreferred => "not_preferred",
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

/// The catalog tools one click acts on: the clicked tool alone, or the whole run between
/// the anchor and it when shift is held.
///
/// `ordered` is **one section's** tool keys in display order, which is what confines a
/// range to the section the operator can see. The catalog tree is built from `<details>`
/// elements whose open state belongs to the DOM, so nothing here can tell an expanded
/// section from a collapsed one; a range that could cross a section boundary would
/// therefore be able to select tools with no way for the operator to notice before
/// pressing Add.
///
/// Every degenerate case degrades to a plain click rather than guessing: shift held with
/// no anchor yet, and an anchor that is not in `ordered` at all — it belongs to another
/// section, or the catalog was reimported and its positional keys shifted under it.
fn catalog_click_range(
    ordered: &[String],
    anchor: Option<&str>,
    clicked: &str,
    shift: bool,
) -> Vec<String> {
    let alone = || vec![clicked.to_string()];
    if !shift {
        return alone();
    }
    let Some(anchor) = anchor else { return alone() };

    let find = |key: &str| ordered.iter().position(|candidate| candidate == key);
    match (find(anchor), find(clicked)) {
        // Inclusive of both ends, and ordered low-to-high so shift-clicking up the list
        // gives the same run as shift-clicking down it.
        (Some(from), Some(to)) => ordered[from.min(to)..=from.max(to)].to_vec(),
        _ => alone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section() -> Vec<String> {
        ["t0", "t1", "t2", "t3", "t4"].iter().map(|k| k.to_string()).collect()
    }

    #[test]
    fn without_shift_a_click_acts_on_its_own_tool() {
        // Even with a perfectly good anchor sitting there — the modifier is the whole
        // difference between toggling one tool and toggling twenty.
        assert_eq!(catalog_click_range(&section(), Some("t0"), "t3", false), vec!["t3"]);
    }

    #[test]
    fn shift_covers_the_run_between_the_anchor_and_the_click() {
        assert_eq!(
            catalog_click_range(&section(), Some("t1"), "t3", true),
            vec!["t1", "t2", "t3"],
            "both ends included"
        );
    }

    /// Selecting up the list and down it must give the same run, or the gesture would
    /// depend on which end the operator happened to click first.
    #[test]
    fn a_run_reads_the_same_in_both_directions() {
        let (down, up) = (
            catalog_click_range(&section(), Some("t1"), "t4", true),
            catalog_click_range(&section(), Some("t4"), "t1", true),
        );
        assert_eq!(down, up);
        assert_eq!(down, vec!["t1", "t2", "t3", "t4"]);
    }

    #[test]
    fn an_anchor_on_the_clicked_tool_is_a_run_of_one() {
        assert_eq!(catalog_click_range(&section(), Some("t2"), "t2", true), vec!["t2"]);
    }

    /// The three ways a range has no meaning. Each degrades to a plain click rather than
    /// to nothing: a shift-click that silently did nothing reads as the list being broken.
    #[test]
    fn a_range_with_no_meaning_degrades_to_a_plain_click() {
        // Shift held before anything has been clicked.
        assert_eq!(catalog_click_range(&section(), None, "t2", true), vec!["t2"]);
        // An anchor from another section — the case that keeps a run inside the section
        // the operator can see.
        assert_eq!(
            catalog_click_range(&section(), Some("other::s1::t0"), "t2", true),
            vec!["t2"]
        );
        // An anchor whose key no longer exists here, as after a catalog reimport shifts
        // the positional keys under it.
        assert_eq!(catalog_click_range(&section(), Some("t9"), "t2", true), vec!["t2"]);
    }

    /// "Added 0 tool(s)" is not an explanation. Picking tools already in stock reported
    /// exactly that, which reads as a broken button rather than as the picker declining
    /// to give a second copy of what is already owned.
    #[test]
    fn a_bulk_add_says_what_it_skipped() {
        use crate::ui::bindings::StockAddition;

        assert_eq!(
            describe_addition(StockAddition { added: 3, skipped: 0 }),
            "Added 3 tool(s) from catalogs"
        );
        assert_eq!(
            describe_addition(StockAddition { added: 0, skipped: 5 }),
            "5 tool(s) already in stock — nothing added"
        );
        assert_eq!(
            describe_addition(StockAddition { added: 2, skipped: 3 }),
            "Added 2 tool(s) — 3 already in stock"
        );
    }

    #[test]
    fn a_one_tool_section_has_nothing_to_range_over() {
        let single = vec!["only".to_string()];
        assert_eq!(catalog_click_range(&single, Some("only"), "only", true), vec!["only"]);
    }
}


use dioxus::prelude::*;
use std::collections::BTreeSet;

use crate::ui::bindings::{StockField, StockForm};
use units::user_format as unit_format;

use crate::data::model::*;
use crate::ui::navigation::StockSortColumn;

/// Orders `tools` — `(index in stock, tool)` — by `column`, reversing for `descending`.
///
/// Every comparison ends on the stock index, so the order is a **total function of the
/// list** rather than of the order it arrived in: ties among a dozen 0.8 mm drills would
/// otherwise shuffle between renders, which reads as the table twitching.
///
/// The index tie-break is the one thing `descending` does not flip. Reversing it too would
/// mean a column with many ties re-ordered its ties as well, so clicking a header twice
/// scrambled rows that share a value instead of just turning the groups over.
fn sort_stock(
    tools: &mut [(usize, &Tool)],
    column: StockSortColumn,
    descending: bool,
    usage: &crate::runtime::tooling::ToolUsage,
) {
    use std::cmp::Ordering;

    // Newest first is what "recent" has always meant here, so it is the *unreversed* sense
    // of that column and Reset view lands on it.
    if column == StockSortColumn::Recent {
        tools.sort_by(|left, right| {
            let by_age = right.0.cmp(&left.0);
            if descending { by_age.reverse() } else { by_age }
        });
        return;
    }

    // How spoken-for a tool is, so the Usage column sorts by the thing it displays: what
    // the job loads first, then what a toolset merely pins, then the rest. Ascending puts
    // the busiest at the top, because "what does this job use" is the question the column
    // was added to answer and nobody clicks a header hoping to see the unused bits first.
    let usage_rank = |tool: &Tool| {
        let uses = usage.for_tool(&tool.id);
        match (uses.in_current_job(), uses.referenced()) {
            (true, true) => 0,
            (true, false) => 1,
            (false, true) => 2,
            (false, false) => 3,
        }
    };

    tools.sort_by(|left, right| {
        let ordering = match column {
            StockSortColumn::Recent => Ordering::Equal, // handled above
            StockSortColumn::Type => {
                stock_tool_type_rank(&left.1.kind).cmp(&stock_tool_type_rank(&right.1.kind))
            }
            StockSortColumn::Diameter => left
                .1
                .diameter
                .as_mm()
                .partial_cmp(&right.1.diameter.as_mm())
                .unwrap_or(Ordering::Equal),
            StockSortColumn::Name => left
                .1
                .display_name()
                .to_ascii_lowercase()
                .cmp(&right.1.display_name().to_ascii_lowercase()),
            StockSortColumn::Source => left
                .1
                .source_catalog
                .to_ascii_lowercase()
                .cmp(&right.1.source_catalog.to_ascii_lowercase()),
            StockSortColumn::Preference => stock_tool_preference_rank(left.1.preference)
                .cmp(&stock_tool_preference_rank(right.1.preference)),
            StockSortColumn::Usage => usage_rank(left.1).cmp(&usage_rank(right.1)),
            StockSortColumn::Status => {
                stock_tool_status_rank(left.1.status).cmp(&stock_tool_status_rank(right.1.status))
            }
        };
        let ordering = if descending { ordering.reverse() } else { ordering };
        ordering.then_with(|| right.0.cmp(&left.0))
    });
}

/// What the Usage dots mean for one tool, spelled out.
///
/// The dots say *whether*; this says *where*, which is the half that is actually
/// actionable — "pinned somewhere" is not much use until you know it is T4 of the toolset
/// you were about to edit.
///
/// `planned` false is its own sentence rather than an omission. With no board there is no
/// tooling plan, so nothing can be in the job — and a tooltip that simply left the job
/// section out would read as "this job does not use it", which is a different and wrong
/// answer.
fn usage_tooltip(uses: &crate::runtime::tooling::ToolUse, planned: bool) -> String {
    let mut lines: Vec<String> = Vec::new();

    for (slot, toolset) in &uses.in_toolsets {
        lines.push(format!("{slot} in '{toolset}'"));
    }
    if lines.is_empty() {
        lines.push("Not pinned in any toolset".to_string());
    }

    if !planned {
        lines.push("\n…and in the job: not planned yet — load a board".to_string());
    } else if uses.in_job.is_empty() {
        lines.push("\n…and in the job: not used".to_string());
    } else {
        lines.push("\n…and in the job:".to_string());
        for (slot, cnc) in &uses.in_job {
            lines.push(format!("{slot} in '{cnc}'"));
        }
    }

    lines.join("\n")
}

/// One sortable column heading.
///
/// Click to sort by it, click again to turn it over — the convention every table an
/// operator has ever used already follows, which is why the sort left the dropdown it was
/// in. The arrow marks the active column and its direction, so the state is readable
/// without clicking anything.
///
/// A component rather than a macro or a loop because a header is a `<th>` in a fixed row:
/// the columns are not a list, they are the table's shape, and one of them is conditional
/// on the rack existing at all.
#[component]
fn SortHeader(
    state: Signal<crate::runtime::AppCtx>,
    column: StockSortColumn,
    active: StockSortColumn,
    descending: bool,
    label: String,
    /// The tooltip, when the column has one to add. Empty for the columns that do not —
    /// the heading already says what they are.
    title: String,
) -> Element {
    let is_active = column == active;
    // A fresh column starts ascending; the active one flips. Nothing here returns to
    // "unsorted": that is Reset view's job, and a third click that silently dropped the
    // sort would leave the table in an order nobody asked for.
    let next_descending = is_active && !descending;
    let arrow = match (is_active, descending) {
        (true, false) => " ▲",
        (true, true) => " ▼",
        (false, _) => "",
    };
    let hint = if title.is_empty() {
        format!("Sort by {label}")
    } else {
        format!("{title}\n\nSort by {label}")
    };

    rsx! {
        th {
            class: if is_active { "stock-sort-header is-active" } else { "stock-sort-header" },
            title: "{hint}",
            "aria-sort": match (is_active, descending) {
                (true, false) => "ascending",
                (true, true) => "descending",
                (false, _) => "none",
            },
            onclick: move |_| {
                super::mutate_ctx(state, |ctx| ctx.app.set_stock_sort(column, next_descending));
            },
            "{label}{arrow}"
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

    // AppData owns stock.yaml. The detail editor writes tool fields straight into the
    // datastore singleton via StockField/StockForm, bumping the store revision; the
    // root's bridge (`refresh_legacy_projections`) mirrors that back into the legacy
    // in-memory `tools` this table reads, so table and detail stay coherent without this
    // screen having to watch for it.
    let snapshot = state.read().clone();
    // Who wants each tool: the toolsets that pin it, and the racks this job loads it into.
    //
    // Computed once for the whole table and then indexed per row — the job half runs the
    // assigner, so asking it per row would plan the job once per tool in stock.
    //
    // Memoised on the context alone, which is what keeps typing in the search box from
    // re-planning: the filter and the sort are separate state, so a keystroke re-renders
    // the table without touching anything this reads.
    let usage_memo = use_memo(move || {
        let ctx = state.read();
        crate::runtime::tooling::tool_usage(&ctx.app, ctx.stitched_board_data.as_ref())
    });
    let usage = usage_memo.read();
    let usage_header_title = if usage.planned {
        "Green: loaded by the job on screen. Blue: pinned in a toolset. Hover a row for \
         the slots and where they are."
            .to_string()
    } else {
        // Nothing green with no plan behind it, and that is not the same as nothing being
        // used — say so in the header rather than letting an empty column read as an answer.
        "Blue: pinned in a toolset. Nothing shows as in-job because there is no plan yet — \
         load a board and select a machining profile."
            .to_string()
    };
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
    // The sort is **not** a signal: it lives in settings, so the snapshot below is the
    // current value and a header click writes through `mutate_ctx` like any other setting.
    // A local copy would be a second source for the same number.
    let sort_column = snapshot.stock_sort_column;
    let sort_descending = snapshot.stock_sort_descending;

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

    sort_stock(&mut filtered_tools, sort_column, sort_descending, &usage);

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
                        // Where the sort dropdown was. Sorting moved to the headers, which
                        // is where a table is sorted — leaving this spot for the one thing
                        // the headers cannot do, which is put everything back.
                        //
                        // It clears the filter and the search as well as the sort: the
                        // three together are why the list looks the way it does, and when
                        // it looks wrong the useful button is the one that undoes all of
                        // them rather than the one that undoes a third.
                        button {
                            class: "btn btn-secondary",
                            title: "Clear the sort, the type filter and the search",
                            disabled: sort_column == StockSortColumn::Recent
                                && !sort_descending
                                && type_filter == StockTypeFilter::All
                                && filter_lower.is_empty(),
                            onclick: move |_| {
                                stock_type_filter.set(StockTypeFilter::All);
                                stock_filter.set(String::new());
                                super::mutate_ctx(
                                    state,
                                    |ctx| ctx.app.set_stock_sort(StockSortColumn::Recent, false),
                                );
                            },
                            "Reset view"
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
                                SortHeader {
                                    state, column: StockSortColumn::Type,
                                    active: sort_column, descending: sort_descending,
                                    label: "Type", title: String::new(),
                                }
                                SortHeader {
                                    state, column: StockSortColumn::Diameter,
                                    active: sort_column, descending: sort_descending,
                                    label: "Diameter", title: String::new(),
                                }
                                SortHeader {
                                    state, column: StockSortColumn::Name,
                                    active: sort_column, descending: sort_descending,
                                    label: "Name", title: String::new(),
                                }
                                SortHeader {
                                    state, column: StockSortColumn::Source,
                                    active: sort_column, descending: sort_descending,
                                    label: "Source catalog", title: String::new(),
                                }
                                SortHeader {
                                    state, column: StockSortColumn::Preference,
                                    active: sort_column, descending: sort_descending,
                                    label: "Preference", title: String::new(),
                                }
                                // Always shown, where the ATC column was hidden without a
                                // tool changer. Usage is an answer either way: a job on a
                                // manual-change machine still loads tools, and until now the
                                // one setup that has to plan its changes by hand was the one
                                // that could not see what it needed.
                                SortHeader {
                                    state, column: StockSortColumn::Usage,
                                    active: sort_column, descending: sort_descending,
                                    label: "Usage", title: usage_header_title.clone(),
                                }
                                SortHeader {
                                    state, column: StockSortColumn::Status,
                                    active: sort_column, descending: sort_descending,
                                    label: "Status", title: String::new(),
                                }
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
                                        // Two independent facts, two dots, and the detail
                                        // on hover: pinned in a toolset, loaded by this job,
                                        // or both — "both" being the case a single verdict
                                        // would have hidden.
                                        let uses = usage.for_tool(&tool_id);
                                        let usage_detail = usage_tooltip(&uses, usage.planned);
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
                                                td {
                                                    span {
                                                        class: "usage-indicator",
                                                        title: "{usage_detail}",
                                                        if uses.in_current_job() {
                                                            span { class: "usage-dot is-job" }
                                                        }
                                                        if uses.referenced() {
                                                            span { class: "usage-dot is-toolset" }
                                                        }
                                                        if !uses.in_current_job() && !uses.referenced() {
                                                            span { class: "usage-empty", "–" }
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


#[cfg(test)]
mod sort_tests {
    use super::*;
    use crate::runtime::tooling::ToolUsage;

    /// A stock tool. Only the fields the comparators read are meaningful.
    fn tool(id: &str, kind: &str, diameter_mm: f64, name: &str, source: &str) -> Tool {
        Tool {
            id: id.to_string(),
            composite_name: name.to_string(),
            name: name.to_string(),
            kind: kind.to_string(),
            diameter: units::Length::from_mm(diameter_mm),
            catalog_diameter: None,
            point_angle: units::Angle::from_degrees(118.0),
            catalog_point_angle: None,
            flute_length: None,
            z_min_depth: None,
            table_feed: None,
            catalog_table_feed: None,
            z_feed: None,
            catalog_z_feed: None,
            spindle_speed: None,
            catalog_spindle_speed: None,
            status: ToolStatus::InStock,
            preference: ToolPreference::Neutral,
            source_catalog: source.to_string(),
            manufacturer: None,
            sku: None,
        }
    }

    /// Insertion order, as the screen holds it.
    fn shelf() -> Vec<Tool> {
        vec![
            tool("a", "Drill bit", 0.8, "Zeta drill", "generic"),
            tool("b", "Router bit", 2.0, "Alpha router", "vendor"),
            tool("c", "Drill bit", 0.3, "Mid drill", "generic"),
        ]
    }

    /// The ids in the order the sort put them.
    fn order(tools: &[Tool], column: StockSortColumn, descending: bool) -> Vec<&str> {
        let mut rows: Vec<(usize, &Tool)> = tools.iter().enumerate().collect();
        sort_stock(&mut rows, column, descending, &ToolUsage::default());
        rows.iter().map(|(_, tool)| tool.id.as_str()).collect()
    }

    /// Every column is a sort, not a filter, and lands on one answer both ways round.
    ///
    /// Deliberately **not** asserting that reversing gives the list backwards: the
    /// tie-break does not flip, so a column whose rows all tie — preference, status and ATC
    /// on this shelf, since nothing there varies — comes back in the same order either way.
    /// That is the documented behaviour and the reason for it is in `sort_stock`.
    #[test]
    fn every_column_keeps_every_row_both_ways() {
        let shelf = shelf();
        for column in [
            StockSortColumn::Recent,
            StockSortColumn::Type,
            StockSortColumn::Diameter,
            StockSortColumn::Name,
            StockSortColumn::Source,
            StockSortColumn::Preference,
            StockSortColumn::Usage,
            StockSortColumn::Status,
        ] {
            for descending in [false, true] {
                let sorted = order(&shelf, column, descending);
                assert_eq!(
                    sorted.len(),
                    shelf.len(),
                    "{column:?} descending={descending} dropped a row",
                );
                let mut unique = sorted.clone();
                unique.sort_unstable();
                unique.dedup();
                assert_eq!(unique.len(), shelf.len(), "{column:?} duplicated a row");
                assert_eq!(
                    sorted,
                    order(&shelf, column, descending),
                    "{column:?} is not a function of the list",
                );
            }
        }
    }

    /// Where the values are all distinct there is nothing to tie, so reversing really is the
    /// list backwards — which is what clicking a header twice has to feel like.
    #[test]
    fn a_column_of_distinct_values_reverses_exactly() {
        let shelf = shelf();
        for column in [StockSortColumn::Diameter, StockSortColumn::Name] {
            let up = order(&shelf, column, false);
            let mut down = order(&shelf, column, true);
            down.reverse();
            assert_eq!(up, down, "{column:?}");
        }
    }

    /// The orderings that have a reading worth pinning, rather than only being reversible.
    #[test]
    fn the_orderings_read_the_way_the_column_says() {
        let shelf = shelf();

        assert_eq!(order(&shelf, StockSortColumn::Diameter, false), ["c", "a", "b"], "0.3, 0.8, 2.0");
        assert_eq!(order(&shelf, StockSortColumn::Diameter, true), ["b", "a", "c"]);
        assert_eq!(
            order(&shelf, StockSortColumn::Name, false),
            ["b", "c", "a"],
            "Alpha, Mid, Zeta \u{2014} and case-insensitively",
        );
        // generic before vendor — and within the two generics the tie-break is newest
        // first, which is `recent`'s sense and what the old sort modes each ended on.
        assert_eq!(order(&shelf, StockSortColumn::Source, false), ["c", "a", "b"]);
        assert_eq!(
            order(&shelf, StockSortColumn::Type, false),
            ["c", "a", "b"],
            "drills before routers, newest drill first",
        );
    }

    /// `recent` is newest first, which is the order the table has before anyone sorts it and
    /// what Reset view returns to. Modelled as a column value rather than an absent one, so
    /// it has to actually order.
    #[test]
    fn recent_is_newest_first() {
        let shelf = shelf();

        assert_eq!(
            order(&shelf, StockSortColumn::Recent, false),
            ["c", "b", "a"],
            "last added first",
        );
        assert_eq!(order(&shelf, StockSortColumn::Recent, true), ["a", "b", "c"]);
    }

    /// **The order is a function of the list, not of the order it arrived in.**
    ///
    /// Every comparison ends on the stock index, so a column full of ties still has one
    /// answer. Without it, a dozen 0.8 mm drills would sit in whatever order the sort
    /// happened to leave them and shuffle between renders — which reads as the table
    /// twitching under the cursor rather than as a sort at all.
    #[test]
    fn ties_are_broken_so_the_order_is_stable() {
        let tied: Vec<Tool> = (0..5)
            .map(|n| tool(&format!("t{n}"), "Drill bit", 0.8, "same", "generic"))
            .collect();

        let first = order(&tied, StockSortColumn::Diameter, false);
        let again = order(&tied, StockSortColumn::Diameter, false);
        assert_eq!(first, again);

        // And reversing turns the groups over without scrambling within them: the tie-break
        // is the one comparison `descending` does not flip.
        assert_eq!(order(&tied, StockSortColumn::Diameter, true), first);
    }

    /// An empty shelf sorts to nothing rather than panicking, on every column.
    #[test]
    fn an_empty_shelf_sorts_to_nothing() {
        for column in [StockSortColumn::Recent, StockSortColumn::Diameter, StockSortColumn::Usage] {
            assert!(order(&[], column, false).is_empty());
        }
    }
}

/// The Usage column's tooltip — the only place the two halves are put into words, so the
/// wording is the thing under test.
#[cfg(test)]
mod usage_tooltip_tests {
    use super::*;
    use crate::runtime::tooling::ToolUse;

    fn uses(toolsets: &[(&str, &str)], job: &[(&str, &str)]) -> ToolUse {
        let pair = |(slot, name): &(&str, &str)| (slot.to_string(), name.to_string());
        ToolUse {
            in_toolsets: toolsets.iter().map(pair).collect(),
            in_job: job.iter().map(pair).collect(),
        }
    }

    /// The shape that was asked for: a line per toolset, then the job's own section.
    #[test]
    fn both_halves_read_as_two_sections() {
        let tip = usage_tooltip(&uses(&[("T4", "Metric"), ("T2", "Imperial")], &[("T2", "CNC 4")]), true);

        assert_eq!(
            tip,
            "T4 in 'Metric'\nT2 in 'Imperial'\n\n…and in the job:\nT2 in 'CNC 4'",
        );
    }

    /// Green with no blue: the job loaded it out of a spare slot and no toolset pins it.
    /// The first section has to *say* that rather than be missing, or the tooltip opens on
    /// the job section and reads as though it were the whole answer.
    #[test]
    fn a_tool_only_the_job_loads_says_it_is_pinned_nowhere() {
        let tip = usage_tooltip(&uses(&[], &[("T3", "Mill")]), true);

        assert!(tip.starts_with("Not pinned in any toolset"));
        assert!(tip.ends_with("…and in the job:\nT3 in 'Mill'"));
    }

    /// Blue with no green: pinned, and this job does not want it. Not the same sentence as
    /// "nothing has been planned", which is the next test — the dot is empty either way and
    /// the tooltip is what tells them apart.
    #[test]
    fn a_pinned_tool_the_job_skips_says_not_used() {
        let tip = usage_tooltip(&uses(&[("T1", "Metric")], &[]), true);

        assert_eq!(tip, "T1 in 'Metric'\n\n…and in the job: not used");
    }

    /// **No plan is not "unused".** With no board there is no answer to give, and saying
    /// "not used" would be inventing one.
    #[test]
    fn with_nothing_planned_the_job_half_withholds_rather_than_denies() {
        let tip = usage_tooltip(&uses(&[("T1", "Metric")], &[]), false);

        assert!(
            tip.ends_with("…and in the job: not planned yet — load a board"),
            "got {tip:?}",
        );
        assert!(!tip.contains("not used"));
    }

    /// A tool nothing mentions still produces both sentences, so an operator clicking a
    /// grey row learns why it is grey.
    #[test]
    fn an_unused_tool_still_answers_both_questions() {
        let tip = usage_tooltip(&ToolUse::default(), true);

        assert_eq!(tip, "Not pinned in any toolset\n\n…and in the job: not used");
    }
}

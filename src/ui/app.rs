use dioxus::prelude::*;
use std::collections::BTreeSet;
use std::cmp::Ordering;

use crate::units::{
    FeedRate, FeedRateUnit, Length, LengthUnit, RotationalSpeed,
    RotationalSpeedUnit,
};
use super::boot_data;
use super::model::*;
use super::theme::APP_STYLE;

#[component]
pub fn AppRoot() -> Element {
    let boot = boot_data().clone();
    let mut state = use_signal(|| UiState::new(boot.save_filename_override.clone()));
    let mut show_error_details = use_signal(|| false);

    let snapshot = state.read().clone();
    let nav_screens = Screen::visible(snapshot.selected_machine_has_atc());
    let error_count = snapshot.errors.iter().filter(|e| e.is_error).count();
    let warning_count = snapshot.errors.len().saturating_sub(error_count);

    rsx! {
        style { "{APP_STYLE}" }

        div { class: if snapshot.theme == Theme::Dark { "app-shell theme-dark" } else { "app-shell theme-light" },

            if snapshot.show_first_launch {
                div { class: "wizard-overlay",
                    div { class: "wizard-dialog",
                        h2 { "Welcome to KiCad CNC Generator" }
                        p { "Create your first CNC profile to start using the plugin." }
                        div { class: "wizard-actions",
                            button {
                                class: "btn btn-primary",
                                onclick: move |_| {
                                    state.with_mut(|s| s.add_demo_machine());
                                },
                                "Create Demo Machine"
                            }
                            button {
                                class: "btn btn-secondary",
                                onclick: move |_| {
                                    state
                                        .with_mut(|s| {
                                            s.show_first_launch = false;
                                            s.selected_screen = Screen::Setup;
                                        });
                                },
                                "Skip"
                            }
                        }
                    }
                }
            }

            div { class: "top-bar",
                div { class: "title", "KiCad CNC Generator" }
                div { class: "divider" }

                div { class: "top-control",
                    label { "CNC Profile" }
                    select {
                        value: snapshot.selected_machine_id.clone().unwrap_or_default(),
                        onchange: move |evt| {
                            let value = evt.value();
                            state
                                .with_mut(|s| {
                                    s.selected_machine_id = if value.is_empty() {
                                        None
                                    } else {
                                        Some(value)
                                    };
                                });
                        },
                        option { value: "", "Select machine..." }
                        for machine in snapshot.machines.iter() {
                            option { value: machine.id.clone(), "{machine.name}" }
                        }
                    }
                }

                div { class: "spacer" }

                div { class: "status-line",
                    if snapshot.generation_state == GenerationState::Generating {
                        span { class: "status-pill status-busy", "Generating" }
                    } else if error_count == 0 && warning_count == 0 {
                        span { class: "status-pill status-ok", "Ready" }
                    } else {
                        span { class: "status-pill status-warn",
                            "{error_count} errors, {warning_count} warnings"
                        }
                    }

                    button {
                        class: "btn btn-icon",
                        onclick: move |_| {
                            state.with_mut(|s| s.select_screen(Screen::Setup));
                        },
                        "Setup"
                    }
                }
            }

            if !snapshot.errors.is_empty() {
                div { class: "error-banner",
                    button {
                        class: "error-toggle",
                        onclick: move |_| {
                            let open = *show_error_details.read();
                            show_error_details.set(!open);
                        },
                        "{error_count} errors, {warning_count} warnings - click for details"
                    }

                    if *show_error_details.read() {
                        div { class: "error-list",
                            for err in snapshot.errors.iter() {
                                div { class: if err.is_error { "error-item error" } else { "error-item warning" },
                                    div { class: "error-title", "{err.message}" }
                                    if let Some(details) = err.details.as_ref() {
                                        div { class: "error-details", "{details}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            div { class: "work-area",
                aside { class: "left-nav",
                    for screen in nav_screens.iter() {
                        button {
                            key: "{screen.key()}",
                            class: if *screen == snapshot.selected_screen { "nav-item active" } else { "nav-item" },
                            onclick: {
                                let target = *screen;
                                move |_| {
                                    state.with_mut(|s| s.select_screen(target));
                                }
                            },
                            "{screen.label()}"
                        }
                    }
                }

                main { class: "main-content",
                    match snapshot.selected_screen {
                        Screen::Setup => rsx! {
                            SetupScreen { state, boot: boot.clone() }
                        },
                        Screen::Stock => rsx! {
                            StockScreen { state }
                        },
                        Screen::Job => rsx! {
                            JobScreen { state }
                        },
                        Screen::BoardView => rsx! {
                            BoardViewScreen { state }
                        },
                        Screen::Program => rsx! {
                            ProgramScreen { state }
                        },
                        Screen::Rack => rsx! {
                            RackScreen { state }
                        },
                    }
                }
            }

            div { class: "footer-line",
                span { class: if boot.kicad_status.starts_with("Connected") { "kicad-ok" } else { "kicad-err" },
                    "KiCad: {boot.kicad_status}"
                }
                span { class: "env-summary", "{boot.env_summary}" }
            }
        }
    }
}

#[component]
fn SetupScreen(state: Signal<UiState>, boot: UiLaunchData) -> Element {
    let snapshot = state.read().clone();

    rsx! {
        div { class: "screen split",
            section { class: "panel fixed",
                h3 { "Settings" }

                div { class: "field",
                    label { "Unit System" }
                    select {
                        value: snapshot.unit_system.as_str(),
                        onchange: move |evt| {
                            let v = evt.value();
                            state
                                .with_mut(|s| {
                                    s.unit_system = if v == "imperial" {
                                        UnitSystem::Imperial
                                    } else {
                                        UnitSystem::Metric
                                    };
                                });
                        },
                        option { value: "metric", "Metric (mm, mm/min)" }
                        option { value: "imperial", "Imperial (mil, in/min)" }
                    }
                }

                div { class: "field",
                    label { "Theme" }
                    select {
                        value: snapshot.theme.as_str(),
                        onchange: move |evt| {
                            let v = evt.value();
                            state
                                .with_mut(|s| {
                                    s.theme = if v == "light" { Theme::Light } else { Theme::Dark };
                                });
                        },
                        option { value: "light", "Light" }
                        option { value: "dark", "Dark" }
                    }
                }

                div { class: "diagnostics",
                    h4 { "Runtime Diagnostics" }
                    p { class: "diag-status", "{boot.kicad_status}" }
                    details {
                        summary { "CLI arguments ({boot.cli_args.len()})" }
                        ol {
                            for arg in boot.cli_args.iter() {
                                li { "{arg}" }
                            }
                        }
                    }
                    details {
                        summary { "Environment ({boot.env_vars.len()})" }
                        div { class: "env-table",
                            table {
                                thead {
                                    tr {
                                        th { "Name" }
                                        th { "Value" }
                                    }
                                }
                                tbody {
                                    for (name , value) in boot.env_vars.iter() {
                                        tr {
                                            td { "{name}" }
                                            td { "{value}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            section { class: "panel grow",
                div { class: "panel-header",
                    h3 { "CNC Machine Profiles" }
                    div { class: "actions",
                        button {
                            class: "btn btn-primary",
                            onclick: move |_| {
                                state.with_mut(|s| s.add_demo_machine());
                            },
                            "New Machine"
                        }
                        button {
                            class: "btn btn-secondary",
                            onclick: move |_| {
                                state.with_mut(|s| s.clone_selected_machine());
                            },
                            "Clone"
                        }
                        button {
                            class: "btn btn-danger",
                            onclick: move |_| {
                                state.with_mut(|s| s.remove_selected_machine());
                            },
                            "Delete"
                        }
                    }
                }

                if snapshot.machines.is_empty() {
                    div { class: "empty-state",
                        p { "No machine profiles configured." }
                        p { "Use New Machine to add one." }
                    }
                } else {
                    div { class: "card-grid",
                        for machine in snapshot.machines.iter() {
                            article {
                                key: "{machine.id}",
                                class: if Some(machine.id.clone()) == snapshot.selected_machine_id { "machine-card active" } else { "machine-card" },
                                h4 { "{machine.name}" }
                                p {
                                    "Fixture: {machine.fixture_plate_max_x} x {machine.fixture_plate_max_y} mm"
                                }
                                p {
                                    "Spindle: {machine.spindle_min_rpm} - {machine.spindle_max_rpm} rpm"
                                }
                                p { "ATC slots: {machine.atc_slot_count}" }
                                button {
                                    class: "btn btn-small",
                                    onclick: {
                                        let machine_id = machine.id.clone();
                                        move |_| {
                                            state.with_mut(|s| s.selected_machine_id = Some(machine_id.clone()));
                                        }
                                    },
                                    if Some(machine.id.clone()) == snapshot.selected_machine_id {
                                        "Selected"
                                    } else {
                                        "Select"
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

#[component]
fn StockScreen(state: Signal<UiState>) -> Element {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum StockSortColumn {
        Name,
        Type,
        Size,
        Status,
        Operations,
    }

    let snapshot = state.read().clone();
    let catalog_index = load_stock_catalog_index();
    let mut show_catalog_picker = use_signal(|| false);
    let mut selected_tool_keys = use_signal(|| BTreeSet::<String>::new());
    let mut sort_column = use_signal(|| StockSortColumn::Name);
    let mut sort_ascending = use_signal(|| true);
    let mut view_tool_id = use_signal(|| Option::<String>::None);
    let mut edit_tool_id = use_signal(|| Option::<String>::None);
    let mut edit_id = use_signal(String::new);
    let mut edit_name = use_signal(String::new);
    let mut edit_kind = use_signal(String::new);
    let mut edit_diameter_mm = use_signal(String::new);
    let mut edit_status = use_signal(|| "in_stock".to_string());
    let mut edit_operation_count = use_signal(String::new);
    let mut edit_manufacturer = use_signal(String::new);
    let mut edit_sku = use_signal(String::new);
    let mut edit_feed_rate = use_signal(String::new);
    let mut edit_spindle_rpm = use_signal(String::new);
    let selected_count = selected_tool_keys.read().len();
    let unit_system = snapshot.unit_system;
    let flat_catalog_tools: Vec<CatalogStockTool> = catalog_index
        .iter()
        .flat_map(|c| c.sections.iter())
        .flat_map(|s| s.tools.iter().cloned())
        .collect();

    // Precompute size strings for the stock table.
    struct StockRowDisplay {
        id: String,
        name: String,
        kind: String,
        size_primary: String,
        size_sort_value: f64,
        status: ToolStatus,
        operation_count: u32,
    }

    fn format_length_for_unit(diameter_mm: f64, unit_system: UnitSystem) -> String {
        let len = Length::from_mm(diameter_mm);
        if unit_system == UnitSystem::Metric {
            format!("{}", Length::from_mm(len.as_mm()))
        } else {
            format!("{}", Length::from_inch(len.as_inch()))
        }
    }

    fn format_feed_for_unit(feed_mm_min: Option<f32>, unit_system: UnitSystem) -> String {
        match feed_mm_min {
            Some(v) => {
                let feed = FeedRate::from_mm_per_min(v as f64);
                if unit_system == UnitSystem::Metric {
                    format!("{}", feed)
                } else {
                    format!("{}", FeedRate::from_in_per_min(feed.as_in_per_min()))
                }
            }
            None => "n/a".to_string(),
        }
    }

    fn format_speed(rpm: Option<u32>) -> String {
        match rpm {
            Some(v) => format!("{}", RotationalSpeed::from_rpm(v as f64)),
            None => "n/a".to_string(),
        }
    }

    let mut stock_rows: Vec<StockRowDisplay> = snapshot
        .tools
        .iter()
        .map(|tool| {
            let len = Length::from_mm(tool.diameter_mm as f64);
            let (size_primary, size_sort_value) = if unit_system == UnitSystem::Metric {
                (format_length_for_unit(len.as_mm(), unit_system), len.as_mm())
            } else {
                (format_length_for_unit(len.as_mm(), unit_system), len.as_inch())
            };
            StockRowDisplay {
                id: tool.id.clone(),
                name: tool.name.clone(),
                kind: tool.kind.clone(),
                size_primary,
                size_sort_value,
                status: tool.status,
                operation_count: tool.operation_count,
            }
        })
        .collect();

    let active_sort_column = *sort_column.read();
    let active_sort_ascending = *sort_ascending.read();
    stock_rows.sort_by(|a, b| {
        let ord = match active_sort_column {
            StockSortColumn::Name => a
                .name
                .to_ascii_lowercase()
                .cmp(&b.name.to_ascii_lowercase()),
            StockSortColumn::Type => a
                .kind
                .to_ascii_lowercase()
                .cmp(&b.kind.to_ascii_lowercase()),
            StockSortColumn::Size => a
                .size_sort_value
                .partial_cmp(&b.size_sort_value)
                .unwrap_or(Ordering::Equal),
            StockSortColumn::Status => a
                .status
                .label()
                .to_ascii_lowercase()
                .cmp(&b.status.label().to_ascii_lowercase()),
            StockSortColumn::Operations => a.operation_count.cmp(&b.operation_count),
        };

        if active_sort_ascending {
            ord
        } else {
            ord.reverse()
        }
    });

    let view_tool = view_tool_id
        .read()
        .as_ref()
        .and_then(|id| snapshot.tools.iter().find(|t| t.id == *id).cloned());

    let sort_marker = |column: StockSortColumn| -> &'static str {
        if active_sort_column == column {
            if active_sort_ascending {
                " ▲"
            } else {
                " ▼"
            }
        } else {
            ""
        }
    };

    let status_value = |status: ToolStatus| -> &'static str {
        match status {
            ToolStatus::InStock => "in_stock",
            ToolStatus::InRack => "in_rack",
            ToolStatus::OutOfStock => "out_of_stock",
            ToolStatus::New => "new",
            ToolStatus::NotPreferred => "not_preferred",
        }
    };

    let parse_status = |raw: &str| -> ToolStatus {
        match raw {
            "in_rack" => ToolStatus::InRack,
            "out_of_stock" => ToolStatus::OutOfStock,
            "new" => ToolStatus::New,
            "not_preferred" => ToolStatus::NotPreferred,
            _ => ToolStatus::InStock,
        }
    };

    rsx! {
        div { class: "screen single",
            div { class: "panel-header",
                h3 { "Tool Stock" }
                button {
                    class: "btn btn-primary",
                    onclick: move |_| {
                        selected_tool_keys.set(BTreeSet::new());
                        show_catalog_picker.set(true);
                    },
                    "Add Tool"
                }
            }

            if *show_catalog_picker.read() {
                div { class: "wizard-overlay",
                    div { class: "catalog-picker-dialog",
                        div { class: "panel-header",
                            div {
                                h3 { "Add Tools From Catalog" }
                                p { "Expand a section, then use Select All / None to pick tools." }
                            }
                        }

                        div { class: "catalog-picker-list",
                            for catalog in catalog_index.iter() {
                                details {
                                    key: "{catalog.key}",
                                    class: "catalog-node",
                                    summary { class: "catalog-node-summary",
                                        "{catalog.name} ({catalog.sections.len()} sections)"
                                    }

                                    for section in catalog.sections.iter() {
                                        details {
                                            key: "{section.key}",
                                            class: "catalog-node section-node",
                                            summary { class: "catalog-node-summary",
                                                "{section.name} ({section.tools.len()} tools)"
                                            }

                                            div { class: "section-controls",
                                                button {
                                                    class: "btn-link",
                                                    onclick: {
                                                        let keys: Vec<String> = section.tools.iter().map(|t| t.key.clone()).collect();
                                                        move |_| {
                                                            selected_tool_keys
                                                                .with_mut(|s| {
                                                                    for k in &keys {
                                                                        s.insert(k.clone());
                                                                    }
                                                                });
                                                        }
                                                    },
                                                    "Select All"
                                                }
                                                span { class: "section-controls-sep",
                                                    "|"
                                                }
                                                button {
                                                    class: "btn-link",
                                                    onclick: {
                                                        let keys: Vec<String> = section.tools.iter().map(|t| t.key.clone()).collect();
                                                        move |_| {
                                                            selected_tool_keys
                                                                .with_mut(|s| {
                                                                    for k in &keys {
                                                                        s.remove(k);
                                                                    }
                                                                });
                                                        }
                                                    },
                                                    "Select None"
                                                }
                                            }

                                            div { class: "catalog-tool-list",
                                                for tool in section.tools.iter() {
                                                    label {
                                                        key: "{tool.key}",
                                                        class: "catalog-tool-row",
                                                        input {
                                                            r#type: "checkbox",
                                                            checked: selected_tool_keys.read().contains(&tool.key),
                                                            oninput: {
                                                                let tool_key = tool.key.clone();
                                                                move |evt: FormEvent| {
                                                                    let checked = evt.checked();
                                                                    selected_tool_keys
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
                                                        span { class: "catalog-tool-main",
                                                            "{tool.display_name}"
                                                        }
                                                        span { class: "catalog-tool-meta",
                                                            "{tool.kind} - {format_length_for_unit(tool.diameter_mm as f64, unit_system)}"
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
                                onclick: move |_| {
                                    show_catalog_picker.set(false);
                                },
                                "Cancel"
                            }
                            button {
                                class: "btn btn-primary",
                                disabled: selected_count == 0,
                                onclick: {
                                    let tools = flat_catalog_tools.clone();
                                    move |_| {
                                        let selected = selected_tool_keys.read().clone();
                                        state
                                            .with_mut(|s| {
                                                for tool in tools.iter().filter(|t| selected.contains(&t.key)) {
                                                    let has_same_sku = tool
                                                        .sku
                                                        .as_ref()
                                                        .map(|sku| !sku.trim().is_empty())
                                                        .unwrap_or(false)
                                                        && s
                                                            .tools
                                                            .iter()
                                                            .any(|existing| {
                                                                existing
                                                                    .sku
                                                                    .as_ref()
                                                                    .map(|x| x == tool.sku.as_ref().unwrap())
                                                                    .unwrap_or(false)
                                                            });
                                                    let has_same_identity = s
                                                        .tools
                                                        .iter()
                                                        .any(|existing| {
                                                            existing.name == tool.display_name
                                                                && existing.kind == tool.kind
                                                                && (existing.diameter_mm - tool.diameter_mm).abs() < 0.0001
                                                        });
                                                    if has_same_sku || has_same_identity {
                                                        continue;
                                                    }
                                                    let next_id = format!("tool-{}", s.tools.len() + 1);
                                                    let sku = tool
                                                        .sku
                                                        .as_ref()
                                                        .and_then(|v| {
                                                            if v.trim().is_empty() { None } else { Some(v.clone()) }
                                                        });
                                                    s.tools
                                                        .push(Tool {
                                                            id: next_id,
                                                            name: tool.display_name.clone(),
                                                            kind: tool.kind.clone(),
                                                            diameter_mm: tool.diameter_mm,
                                                            feed_rate_mm_min: tool.feed_rate_mm_min,
                                                            spindle_rpm: tool.spindle_rpm,
                                                            status: ToolStatus::New,
                                                            operation_count: 0,
                                                            manufacturer: Some(
                                                                format!("{} / {}", tool.catalog_name, tool.section_name),
                                                            ),
                                                            sku,
                                                            native_is_inch: tool.native_is_inch,
                                                        });
                                                }
                                            });
                                        selected_tool_keys.set(BTreeSet::new());
                                        show_catalog_picker.set(false);
                                    }
                                },
                                "Add Selected ({selected_count})"
                            }
                        }
                    }
                }
            }

            if let Some(tool) = view_tool {
                div { class: "wizard-overlay",
                    div { class: "catalog-picker-dialog",
                        div { class: "panel-header",
                            h3 { "Tool Details" }
                        }

                        div { class: "edit-grid",
                            div { class: "field",
                                label { "ID" }
                                input { value: tool.id.clone(), disabled: true }
                            }
                            div { class: "field",
                                label { "Name" }
                                input { value: tool.name.clone(), disabled: true }
                            }
                            div { class: "field",
                                label { "Type" }
                                input { value: tool.kind.clone(), disabled: true }
                            }
                            div { class: "field",
                                label { "Diameter" }
                                input {
                                    value: format_length_for_unit(tool.diameter_mm as f64, unit_system),
                                    disabled: true,
                                }
                            }
                            div { class: "field",
                                label { "Feed Rate" }
                                input {
                                    value: format_feed_for_unit(tool.feed_rate_mm_min, unit_system),
                                    disabled: true,
                                }
                            }
                            div { class: "field",
                                label { "Spindle Speed" }
                                input {
                                    value: format_speed(tool.spindle_rpm),
                                    disabled: true,
                                }
                            }
                            div { class: "field",
                                label { "Status" }
                                input {
                                    value: tool.status.label(),
                                    disabled: true,
                                }
                            }
                            div { class: "field",
                                label { "Operations" }
                                input {
                                    value: format!("{}", tool.operation_count),
                                    disabled: true,
                                }
                            }
                            div { class: "field",
                                label { "Manufacturer" }
                                input {
                                    value: tool.manufacturer.clone().unwrap_or_default(),
                                    disabled: true,
                                }
                            }
                            div { class: "field",
                                label { "SKU" }
                                input {
                                    value: tool.sku.clone().unwrap_or_default(),
                                    disabled: true,
                                }
                            }
                        }

                        div { class: "wizard-actions",
                            button {
                                class: "btn btn-secondary",
                                onclick: move |_| {
                                    view_tool_id.set(None);
                                },
                                "Close"
                            }
                        }
                    }
                }
            }

            if edit_tool_id.read().is_some() {
                div { class: "wizard-overlay",
                    div { class: "catalog-picker-dialog",
                        div { class: "panel-header",
                            h3 { "Edit Tool" }
                        }

                        div { class: "edit-grid",
                            div { class: "field",
                                label { "ID" }
                                input {
                                    value: edit_id.read().clone(),
                                    oninput: move |evt| edit_id.set(evt.value()),
                                }
                            }
                            div { class: "field",
                                label { "Name" }
                                input {
                                    value: edit_name.read().clone(),
                                    oninput: move |evt| edit_name.set(evt.value()),
                                }
                            }
                            div { class: "field",
                                label { "Type" }
                                input {
                                    value: edit_kind.read().clone(),
                                    oninput: move |evt| edit_kind.set(evt.value()),
                                }
                            }
                            div { class: "field",
                                label { "Diameter" }
                                input {
                                    value: edit_diameter_mm.read().clone(),
                                    oninput: move |evt| edit_diameter_mm.set(evt.value()),
                                }
                            }
                            div { class: "field",
                                label { "Feed Rate" }
                                input {
                                    value: edit_feed_rate.read().clone(),
                                    oninput: move |evt| edit_feed_rate.set(evt.value()),
                                }
                            }
                            div { class: "field",
                                label { "Spindle Speed (rpm)" }
                                input {
                                    value: edit_spindle_rpm.read().clone(),
                                    oninput: move |evt| edit_spindle_rpm.set(evt.value()),
                                }
                            }
                            div { class: "field",
                                label { "Status" }
                                select {
                                    value: edit_status.read().clone(),
                                    onchange: move |evt| edit_status.set(evt.value()),
                                    option { value: "in_stock", "In Stock" }
                                    option { value: "in_rack", "In Rack" }
                                    option { value: "out_of_stock", "Out Of Stock" }
                                    option { value: "new", "New" }
                                    option { value: "not_preferred", "Not Preferred" }
                                }
                            }
                            div { class: "field",
                                label { "Operations" }
                                input {
                                    value: edit_operation_count.read().clone(),
                                    oninput: move |evt| edit_operation_count.set(evt.value()),
                                }
                            }
                            div { class: "field",
                                label { "Manufacturer" }
                                input {
                                    value: edit_manufacturer.read().clone(),
                                    oninput: move |evt| edit_manufacturer.set(evt.value()),
                                }
                            }
                            div { class: "field",
                                label { "SKU" }
                                input {
                                    value: edit_sku.read().clone(),
                                    oninput: move |evt| edit_sku.set(evt.value()),
                                }
                            }
                        }

                        div { class: "wizard-actions",
                            button {
                                class: "btn btn-secondary",
                                onclick: move |_| {
                                    edit_tool_id.set(None);
                                },
                                "Cancel"
                            }
                            button {
                                class: "btn btn-primary",
                                onclick: move |_| {
                                    let target_id_opt = edit_tool_id.read().clone();
                                    if let Some(target_id) = target_id_opt {
                                        let new_id = edit_id.read().trim().to_string();
                                        let new_name = edit_name.read().trim().to_string();
                                        let new_kind = edit_kind.read().trim().to_string();
                                        let new_diameter_mm = {
                                            let raw = edit_diameter_mm.read().trim().to_string();
                                            if raw.is_empty() {
                                                0.0
                                            } else {
                                                let default_unit = if unit_system == UnitSystem::Metric {
                                                    Some(LengthUnit::Mm)
                                                } else {
                                                    Some(LengthUnit::Inch)
                                                };
                                                Length::from_string(&raw, default_unit)
                                                    .map(|v| v.as_mm() as f32)
                                                    .unwrap_or(0.0)
                                            }
                                        };
                                        let new_feed_rate_mm_min = {
                                            let raw = edit_feed_rate.read().trim().to_string();
                                            if raw.is_empty() {
                                                None
                                            } else {
                                                let default_unit = if unit_system == UnitSystem::Metric {
                                                    Some(FeedRateUnit::MmPerMin)
                                                } else {
                                                    Some(FeedRateUnit::InPerMin)
                                                };
                                                FeedRate::from_string(&raw, default_unit)
                                                    .ok()
                                                    .map(|v| v.as_mm_per_min() as f32)
                                            }
                                        };
                                        let new_spindle_rpm = {
                                            let raw = edit_spindle_rpm.read().trim().to_string();
                                            if raw.is_empty() {
                                                None
                                            } else {
                                                RotationalSpeed::from_string(&raw, Some(RotationalSpeedUnit::Rpm))
                                                    .ok()
                                                    .map(|v| v.as_rpm().round() as u32)
                                            }
                                        };
                                        let new_operation_count = edit_operation_count
                                            .read()
                                            .trim()
                                            .parse::<u32>()
                                            .unwrap_or(0);
                                        let new_status = parse_status(edit_status.read().as_str());
                                        let new_manufacturer = {
                                            let m = edit_manufacturer.read().trim().to_string();
                                            if m.is_empty() { None } else { Some(m) }
                                        };
                                        let new_sku = {
                                            let s = edit_sku.read().trim().to_string();
                                            if s.is_empty() { None } else { Some(s) }
                                        };
                                        state
                                            .with_mut(|s| {
                                                if let Some(tool) = s.tools.iter_mut().find(|t| t.id == target_id) {
                                                    tool.id = if new_id.is_empty() {
                                                        target_id.clone()
                                                    } else {
                                                        new_id.clone()
                                                    };
                                                    tool.name = new_name.clone();
                                                    tool.kind = new_kind.clone();
                                                    tool.diameter_mm = new_diameter_mm;
                                                    tool.feed_rate_mm_min = new_feed_rate_mm_min;
                                                    tool.spindle_rpm = new_spindle_rpm;
                                                    tool.status = new_status;
                                                    tool.operation_count = new_operation_count;
                                                    tool.manufacturer = new_manufacturer.clone();
                                                    tool.sku = new_sku.clone();
                                                }
                                            });
                                        edit_tool_id.set(None);
                                    }
                                },
                                "Save"
                            }
                        }
                    }
                }
            }

            if snapshot.tools.is_empty() {
                div { class: "empty-state",
                    p { "No tools in stock." }
                }
            } else {
                div { class: "table-wrap",
                    table {
                        thead {
                            tr {
                                th { class: "stock-col-description",
                                    button {
                                        class: "th-sort-btn",
                                        onclick: move |_| {
                                            if *sort_column.read() == StockSortColumn::Name {
                                                let next = !*sort_ascending.read();
                                                sort_ascending.set(next);
                                            } else {
                                                sort_column.set(StockSortColumn::Name);
                                                sort_ascending.set(true);
                                            }
                                        },
                                        "Name{sort_marker(StockSortColumn::Name)}"
                                    }
                                }
                                th {
                                    button {
                                        class: "th-sort-btn",
                                        onclick: move |_| {
                                            if *sort_column.read() == StockSortColumn::Type {
                                                let next = !*sort_ascending.read();
                                                sort_ascending.set(next);
                                            } else {
                                                sort_column.set(StockSortColumn::Type);
                                                sort_ascending.set(true);
                                            }
                                        },
                                        "Type{sort_marker(StockSortColumn::Type)}"
                                    }
                                }
                                th {
                                    button {
                                        class: "th-sort-btn",
                                        onclick: move |_| {
                                            if *sort_column.read() == StockSortColumn::Size {
                                                let next = !*sort_ascending.read();
                                                sort_ascending.set(next);
                                            } else {
                                                sort_column.set(StockSortColumn::Size);
                                                sort_ascending.set(true);
                                            }
                                        },
                                        "Size{sort_marker(StockSortColumn::Size)}"
                                    }
                                }
                                th {
                                    button {
                                        class: "th-sort-btn",
                                        onclick: move |_| {
                                            if *sort_column.read() == StockSortColumn::Status {
                                                let next = !*sort_ascending.read();
                                                sort_ascending.set(next);
                                            } else {
                                                sort_column.set(StockSortColumn::Status);
                                                sort_ascending.set(true);
                                            }
                                        },
                                        "Status{sort_marker(StockSortColumn::Status)}"
                                    }
                                }
                                th {
                                    button {
                                        class: "th-sort-btn",
                                        onclick: move |_| {
                                            if *sort_column.read() == StockSortColumn::Operations {
                                                let next = !*sort_ascending.read();
                                                sort_ascending.set(next);
                                            } else {
                                                sort_column.set(StockSortColumn::Operations);
                                                sort_ascending.set(true);
                                            }
                                        },
                                        "Operations{sort_marker(StockSortColumn::Operations)}"
                                    }
                                }
                                th { class: "stock-actions-cell", "Actions" }
                            }
                        }
                        tbody {
                            for row in stock_rows.iter() {
                                tr {
                                    td { class: "stock-col-description", "{row.name}" }
                                    td { "{row.kind}" }
                                    td { class: "size-cell",
                                        span { "{row.size_primary}" }
                                    }
                                    td {
                                        span { class: "status-chip {row.status.class_name()}",
                                            "{row.status.label()}"
                                        }
                                    }
                                    td { "{row.operation_count}" }
                                    td { class: "stock-actions-cell",
                                        div { class: "stock-actions",
                                            button {
                                                class: "btn btn-small btn-secondary",
                                                onclick: {
                                                    let tool_id = row.id.clone();
                                                    move |_| {
                                                        view_tool_id.set(Some(tool_id.clone()));
                                                    }
                                                },
                                                "View"
                                            }
                                            button {
                                                class: "btn btn-small btn-secondary",
                                                onclick: {
                                                    let tool_id = row.id.clone();
                                                    let app_state = state;
                                                    move |_| {
                                                        let tool_opt = app_state
                                                            .read()
                                                            .tools
                                                            .iter()
                                                            .find(|t| t.id == tool_id)
                                                            .cloned();

                                                        if let Some(tool) = tool_opt {
                                                            edit_id.set(tool.id.clone());
                                                            edit_name.set(tool.name.clone());
                                                            edit_kind.set(tool.kind.clone());
                                                            edit_diameter_mm
                                                                .set(format_length_for_unit(tool.diameter_mm as f64, unit_system));
                                                            edit_feed_rate
                                                                .set(format_feed_for_unit(tool.feed_rate_mm_min, unit_system));
                                                            edit_spindle_rpm.set(format_speed(tool.spindle_rpm));
                                                            edit_status.set(status_value(tool.status).to_string());
                                                            edit_operation_count.set(format!("{}", tool.operation_count));
                                                            edit_manufacturer.set(tool.manufacturer.unwrap_or_default());
                                                            edit_sku.set(tool.sku.unwrap_or_default());
                                                            edit_tool_id.set(Some(tool_id.clone()));
                                                        }
                                                    }
                                                },
                                                "Edit"
                                            }
                                            button {
                                                class: "btn btn-small btn-danger",
                                                onclick: {
                                                    let tool_id = row.id.clone();
                                                    move |_| {
                                                        state
                                                            .with_mut(|s| {
                                                                s.tools.retain(|t| t.id != tool_id);
                                                            });
                                                    }
                                                },
                                                "Remove"
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
}

#[component]
fn JobScreen(state: Signal<UiState>) -> Element {
    let snapshot = state.read().clone();
    let has_atc = snapshot.selected_machine_has_atc();
    let routing_enabled = snapshot
        .job_config
        .selected_operations
        .contains(&ProductionOperation::RouteBoard);

    rsx! {
        div { class: "screen split",
            section { class: "panel grow board-preview",
                div { class: "preview-box", "PCB Preview" }
                p { "Use Board View for visual verification." }
            }

            section { class: "panel fixed",
                h3 { "Job Configuration" }

                div { class: "field",
                    label { "Operations" }
                    for op in ProductionOperation::all().iter() {
                        button {
                            key: "{op.label()}",
                            class: if snapshot.job_config.selected_operations.contains(op) { "btn-op active" } else { "btn-op" },
                            onclick: {
                                let operation = *op;
                                move |_| {
                                    state.with_mut(|s| s.toggle_operation(operation));
                                }
                            },
                            "{op.label()}"
                        }
                    }
                }

                div { class: "field",
                    label { "Side" }
                    select {
                        value: snapshot.job_config.side.as_str(),
                        onchange: move |evt| {
                            let v = evt.value();
                            state
                                .with_mut(|s| {
                                    s.job_config.side = if v == "back" { Side::Back } else { Side::Front };
                                });
                        },
                        option { value: "front", "Front" }
                        option { value: "back", "Back" }
                    }
                }

                div { class: "field",
                    label { "Rotation" }
                    select {
                        value: snapshot.job_config.rotation_mode.as_str(),
                        onchange: move |evt| {
                            let v = evt.value();
                            state
                                .with_mut(|s| {
                                    s.job_config.rotation_mode = if v == "manual" {
                                        RotationMode::Manual
                                    } else {
                                        RotationMode::Auto
                                    };
                                });
                        },
                        option { value: "auto", "Auto" }
                        option { value: "manual", "Manual" }
                    }

                    if snapshot.job_config.rotation_mode == RotationMode::Manual {
                        input {
                            r#type: "number",
                            value: "{snapshot.job_config.rotation_angle}",
                            oninput: move |evt| {
                                let value = evt.value().parse::<i32>().unwrap_or(0);
                                state.with_mut(|s| s.set_rotation_angle(value));
                            },
                        }
                    }
                }

                if has_atc {
                    div { class: "field",
                        label { "ATC Rack Strategy" }
                        select {
                            value: snapshot.job_config.atc_rack_strategy.as_str(),
                            onchange: move |evt| {
                                let v = evt.value();
                                state
                                    .with_mut(|s| {
                                        s.job_config.atc_rack_strategy = if v == "overwrite" {
                                            AtcRackStrategy::Overwrite
                                        } else {
                                            AtcRackStrategy::Reuse
                                        };
                                    });
                            },
                            option { value: "reuse", "Reuse rack" }
                            option { value: "overwrite", "Overwrite rack" }
                        }
                    }
                }

                if routing_enabled {
                    div { class: "field",
                        label { "Routing Options" }
                        div { class: "inline-field",
                            span { "Tab count" }
                            input {
                                r#type: "number",
                                min: "0",
                                value: "{snapshot.job_config.tab_count}",
                                oninput: move |evt| {
                                    let value = evt.value().parse::<u8>().unwrap_or(0);
                                    state.with_mut(|s| s.job_config.tab_count = value);
                                },
                            }
                        }
                        div { class: "inline-field",
                            span { "Tab width mm" }
                            input {
                                r#type: "number",
                                step: "0.1",
                                value: "{snapshot.job_config.tab_width_mm}",
                                oninput: move |evt| {
                                    let value = evt.value().parse::<f32>().unwrap_or(0.0);
                                    state.with_mut(|s| s.job_config.tab_width_mm = value);
                                },
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn BoardViewScreen(state: Signal<UiState>) -> Element {
    let snapshot = state.read().clone();

    rsx! {
        div { class: "screen split",
            section { class: "panel grow board-preview",
                div { class: "canvas-mock", "Board canvas placeholder" }
                p { "Zoom, pan and path rendering can be attached to a real canvas backend later." }
            }
            section { class: "panel fixed",
                h3 { "Board Visualization" }
                div { class: "field",
                    label { "Layers" }

                    label { class: "checkbox-line",
                        input {
                            r#type: "checkbox",
                            checked: snapshot.board_layers.holes,
                            oninput: move |evt| {
                                state.with_mut(|s| s.board_layers.holes = evt.checked());
                            },
                        }
                        span { "Holes" }
                    }

                    label { class: "checkbox-line",
                        input {
                            r#type: "checkbox",
                            checked: snapshot.board_layers.routes,
                            oninput: move |evt| {
                                state.with_mut(|s| s.board_layers.routes = evt.checked());
                            },
                        }
                        span { "Routes" }
                    }

                    label { class: "checkbox-line",
                        input {
                            r#type: "checkbox",
                            checked: snapshot.board_layers.paths,
                            oninput: move |evt| {
                                state.with_mut(|s| s.board_layers.paths = evt.checked());
                            },
                        }
                        span { "Tool paths" }
                    }

                    label { class: "checkbox-line",
                        input {
                            r#type: "checkbox",
                            checked: snapshot.board_layers.tabs,
                            oninput: move |evt| {
                                state.with_mut(|s| s.board_layers.tabs = evt.checked());
                            },
                        }
                        span { "Tabs" }
                    }
                }
            }
        }
    }
}

#[component]
fn ProgramScreen(state: Signal<UiState>) -> Element {
    let snapshot = state.read().clone();
    let line_count = snapshot.gcode.lines().count();
    let char_count = snapshot.gcode.len();

    rsx! {
        div { class: "screen single",
            div { class: "panel-header",
                h3 { "Program" }
                div { class: "actions",
                    button { class: "btn btn-primary", "Save to File" }
                    button { class: "btn btn-secondary", "Save to Media" }
                    button { class: "btn btn-secondary", "Send to CNC" }
                }
            }

            if snapshot.gcode_modified {
                div { class: "modified-banner",
                    "Program modified. Regeneration will overwrite changes."
                }
            }

            textarea {
                class: "gcode-editor",
                value: snapshot.gcode.clone(),
                oninput: move |evt| {
                    let value = evt.value();
                    state
                        .with_mut(|s| {
                            s.gcode = value;
                            s.gcode_modified = true;
                        });
                },
            }

            div { class: "program-stats",
                span { "Save target: {snapshot.save_filename}" }
                span { "Lines: {line_count}" }
                span { "Characters: {char_count}" }
            }
        }
    }
}

#[component]
fn RackScreen(state: Signal<UiState>) -> Element {
    let snapshot = state.read().clone();
    let Some(machine) = snapshot.selected_machine().cloned() else {
        return rsx! {
            div { class: "screen single centered",
                p { "No machine selected." }
            }
        };
    };

    if machine.atc_slot_count == 0 {
        return rsx! {
            div { class: "screen single centered",
                p { "ATC is not enabled for the selected machine." }
            }
        };
    }

    let slot_views: Vec<(u8, String, String, String, String)> = (1..=machine.atc_slot_count)
        .map(|slot_num| {
            let slot = snapshot.rack_slots.get(&slot_num).cloned().unwrap_or(RackSlot {
                tool_id: None,
                locked: false,
                disabled: false,
            });

            let class_name = if slot.disabled {
                "rack-slot disabled"
            } else if slot.tool_id.is_some() {
                "rack-slot assigned"
            } else {
                "rack-slot"
            }
            .to_string();

            let tool_name = slot
                .tool_id
                .as_ref()
                .and_then(|id| snapshot.tools.iter().find(|t| &t.id == id))
                .map(|t| t.name.clone())
                .unwrap_or_else(|| "Empty".to_string());

            let locked = if slot.locked { "Yes" } else { "No" }.to_string();
            let disabled = if slot.disabled { "Yes" } else { "No" }.to_string();

            (slot_num, class_name, tool_name, locked, disabled)
        })
        .collect();

    let impact: Vec<(Tool, bool)> = snapshot
        .tools
        .iter()
        .filter(|t| t.status == ToolStatus::InRack)
        .map(|t| {
            let assigned = snapshot
                .rack_slots
                .values()
                .any(|slot| slot.tool_id.as_ref() == Some(&t.id));
            (t.clone(), assigned)
        })
        .collect();

    rsx! {
        div { class: "screen split",
            section { class: "panel grow",
                h3 { "ATC Rack Configuration" }
                p { "Configure tool positions in the {machine.atc_slot_count}-slot rack." }
                div { class: "rack-grid",
                    for (slot_num , class_name , tool_name , locked , disabled) in slot_views.iter() {
                        div { class: "{class_name}",
                            div { class: "rack-slot-title", "Slot #{slot_num}" }
                            p { "Tool: {tool_name}" }
                            p { "Locked: {locked}" }
                            p { "Disabled: {disabled}" }
                        }
                    }
                }
            }

            section { class: "panel fixed",
                h3 { "Job Impact" }
                p { "Tools marked as In Rack are expected for current operations." }
                div { class: "impact-list",
                    for (tool , assigned) in impact.iter() {
                        div { class: if *assigned { "impact-item ok" } else { "impact-item missing" },
                            div { class: "impact-name", "{tool.name}" }
                            div { class: "impact-state",
                                if *assigned {
                                    "Assigned"
                                } else {
                                    "Missing"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

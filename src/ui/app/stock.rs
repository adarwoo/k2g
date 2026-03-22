use dioxus::prelude::*;
use std::collections::BTreeSet;

use crate::units::{UserUnitDisplay, UserUnitSystem};

use super::super::model::*;

#[component]
pub fn StockScreen(state: Signal<UiState>) -> Element {
    let snapshot = state.read().clone();
    let user_unit_system = if snapshot.unit_system == UnitSystem::Imperial {
        UserUnitSystem::Imperial
    } else {
        UserUnitSystem::Metric
    };
    let mut show_catalog_picker = use_signal(|| false);
    let mut selected_tool_keys = use_signal(|| BTreeSet::<String>::new());
    let mut stock_feedback = use_signal(String::new);

    let selected_count = selected_tool_keys.read().len();

    rsx! {
        div { class: "screen single",
            div { class: "panel-header",
                h3 { "Stock" }
                button {
                    class: "btn btn-primary",
                    onclick: move |_| {
                        selected_tool_keys.set(BTreeSet::new());
                        show_catalog_picker.set(true);
                    },
                    "Add Tools From Catalog"
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
                                h3 { "Add Tools From Catalog" }
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
                                                        span { class: "catalog-tool-label",
                                                            "{tool.display_name}"
                                                        }
                                                        span { class: "catalog-tool-type",
                                                            "{catalog_tool_type(tool)}"
                                                        }
                                                        span { class: "catalog-tool-diameter",
                                                            "{catalog_tool_diameter(tool, user_unit_system)}"
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
                                disabled: selected_count == 0,
                                onclick: move |_| {
                                    let selected: Vec<String> = selected_tool_keys.read().iter().cloned().collect();
                                    let mut added = 0usize;
                                    state
                                        .with_mut(|s| {
                                            added = s.add_tools_from_catalog_selection(&selected);
                                        });
                                    stock_feedback.set(format!("Added {} tool(s) from catalogs", added));
                                    selected_tool_keys.set(BTreeSet::new());
                                    show_catalog_picker.set(false);
                                },
                                "Add Selected ({selected_count})"
                            }
                        }
                    }
                }
            }

            if snapshot.tools.is_empty() {
                div { class: "empty-state",
                    p { "No tools in stock." }
                    p { "Add tools from catalogs using the button above." }
                }
            } else {
                div { class: "table-wrap",
                    table {
                        thead {
                            tr {
                                th { "Name" }
                                th { "Type" }
                                th { "Diameter" }
                                th { "Status" }
                                th { "Ops" }
                            }
                        }
                        tbody {
                            for tool in snapshot.tools.iter() {
                                tr {
                                    td { "{tool.name}" }
                                    td { "{tool.kind}" }
                                    td { "{tool.diameter}" }
                                    td {
                                        span { class: "status-chip {tool.status.class_name()}",
                                            "{tool.status.label()}"
                                        }
                                    }
                                    td { "{tool.operation_count}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn catalog_tool_type(tool: &CatalogStockTool) -> &'static str {
    if tool.kind.eq_ignore_ascii_case("drill") {
        return "drillbit";
    }

    let lower_name = tool.display_name.to_ascii_lowercase();
    if lower_name.contains("v-bit") || lower_name.starts_with('v') {
        "v"
    } else if lower_name.contains("mill") || lower_name.contains("end") {
        "mill"
    } else {
        "router"
    }
}

fn catalog_tool_diameter(tool: &CatalogStockTool, user_unit_system: UserUnitSystem) -> String {
    let display = tool.diameter.unit_display(user_unit_system);
    if let Some(native) = display.native {
        format!("{} [{}]", display.user, native)
    } else {
        display.user
    }
}

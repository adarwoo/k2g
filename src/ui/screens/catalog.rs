use dioxus::prelude::*;
use rfd::FileDialog;
use std::fs;

use units::user_format as unit_format;

/// Catalog management screen — imports supplier catalogs, lists the stock sources
/// available to the tool picker, and shows the tools inside a selected catalog
/// (read-only), so a catalog's contents can be inspected without adding tools to
/// stock. Import/list/delete run against the legacy catalog manager on the
/// context; the datastore exposes catalogs read-only via `AppData::catalogs` for
/// reference resolution.
#[component]
pub fn CatalogScreen(state: Signal<crate::runtime::AppCtx>) -> Element {
    let import_feedback = use_signal(String::new);

    rsx! {
        div { class: "screen single",
            CatalogManagementPanel { state, import_feedback }
        }
    }
}

/// Catalog list + import/delete controls, plus a read-only view of the selected
/// catalog's tools (the same table shape the Stock screen uses).
#[component]
fn CatalogManagementPanel(
    state: Signal<crate::runtime::AppCtx>,
    import_feedback: Signal<String>,
) -> Element {
    let mut viewing_catalog_key = use_signal(|| None::<String>);

    use_effect(move || {
        super::mutate_ctx(state, |s| s.ensure_catalogs_loaded());
    });

    let snapshot = state.read().clone();
    let unit_system = snapshot.unit_system;

    // Default to the first catalog so contents are visible immediately.
    let viewed_key = viewing_catalog_key
        .read()
        .clone()
        .or_else(|| snapshot.catalogs.first().map(|c| c.key.clone()));
    let viewed_catalog = viewed_key
        .as_ref()
        .and_then(|k| snapshot.catalogs.iter().find(|c| &c.key == k));

    rsx! {
        section { class: "setup-stage",
            div { class: "setup-stage-header",
                h2 { "Catalog management" }
                p {
                    "Import supplier catalogs, browse their tools, and manage the stock sources available to the tool picker."
                }
            }

            article { class: "setup-card setup-card-list",
                div { class: "panel-header",
                    h3 { "Catalogs" }
                    button {
                        class: "btn btn-primary",
                        onclick: move |_| {
                            let picked = FileDialog::new()
                                .set_title("Import catalog")
                                .add_filter("Catalog YAML", &["yaml", "yml"])
                                .pick_file();

                            let Some(path) = picked else {
                                import_feedback.set("Catalog import canceled".to_string());
                                return;
                            };

                            let text = match fs::read_to_string(&path) {
                                Ok(text) => text,
                                Err(_) => {
                                    import_feedback
                                        .set("Catalog import failed: file not readable".to_string());
                                    return;
                                }
                            };
                            let stem = path
                                .file_stem()
                                .and_then(|name| name.to_str())
                                .unwrap_or("catalog")
                                .to_string();
                            state
                                .with_mut(|s| match s.import_catalog_text(&stem, &text) {
                                    Ok(name) => import_feedback.set(format!("Catalog '{name}' imported")),
                                    Err(msg) => import_feedback.set(msg),
                                });
                        },
                        "Import catalog"
                    }
                }

                if !import_feedback.read().is_empty() {
                    p { class: "diag-status", "{import_feedback.read()}" }
                }

                div { class: "table-wrap",
                    table {
                        thead {
                            tr {
                                th { "Catalog" }
                                th { "Type" }
                                th { "Sections" }
                                th { "Actions" }
                            }
                        }
                        tbody {
                            for catalog in snapshot.catalogs.iter() {
                                tr {
                                    key: "{catalog.key}",
                                    class: if Some(&catalog.key) == viewed_key.as_ref() { "catalog-row active" } else { "catalog-row" },
                                    onclick: {
                                        let key = catalog.key.clone();
                                        move |_| viewing_catalog_key.set(Some(key.clone()))
                                    },
                                    td { "{catalog.name}" }
                                    td {
                                        if catalog.built_in {
                                            "Built-in"
                                        } else {
                                            "Imported"
                                        }
                                    }
                                    td { "{catalog.sections.len()}" }
                                    td {
                                        if catalog.built_in {
                                            span { class: "status-chip status-new", "Protected" }
                                        } else {
                                            button {
                                                class: "btn btn-danger btn-small",
                                                onclick: {
                                                    let key = catalog.key.clone();
                                                    // stop the row's select handler from also firing
                                                    move |evt: Event<MouseData>| {
                                                        evt.stop_propagation();
                                                        state
                                                            .with_mut(|s| {
                                                                match s.remove_catalog(&key) {
                                                                    Ok(_) => import_feedback.set("Catalog deleted".to_string()),
                                                                    Err(msg) => import_feedback.set(msg),
                                                                }
                                                            });
                                                    }
                                                },
                                                "Delete"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Read-only contents of the selected catalog — the same tool table the
            // Stock screen shows, so tools can be inspected without adding them.
            if let Some(catalog) = viewed_catalog {
                article { class: "setup-card",
                    div { class: "panel-header",
                        h3 { "{catalog.name} — tools" }
                    }

                    if catalog.sections.is_empty() {
                        div { class: "empty-state",
                            p { "This catalog has no tools." }
                        }
                    } else {
                        div { class: "table-wrap",
                            table {
                                thead {
                                    tr {
                                        th { "Type" }
                                        th { "Diameter" }
                                        th { "Name" }
                                        th { "SKU" }
                                        th { "Point angle" }
                                        th { "Feed" }
                                        th { "Speed" }
                                    }
                                }
                                tbody {
                                    {catalog.sections.iter().map(|section| {
                                        rsx! {
                                            tr { key: "sec-{section.key}", class: "catalog-section-row",
                                                td { colspan: "7", "{section.name}" }
                                            }
                                            {section.tools.iter().map(|tool| {
                                                let sku = tool.sku.clone().unwrap_or_else(|| "\u{2014}".to_string());
                                                let feed = tool
                                                    .feed_rate
                                                    .map(|f| unit_format::format_feed_display(f, unit_system))
                                                    .unwrap_or_else(|| "\u{2014}".to_string());
                                                let speed = tool
                                                    .spindle_speed
                                                    .map(|s| unit_format::format_rotational_speed_display(s))
                                                    .unwrap_or_else(|| "\u{2014}".to_string());
                                                rsx! {
                                                    tr { key: "{tool.key}",
                                                        td { "{tool.kind}" }
                                                        td { "{unit_format::format_length_display(tool.diameter, unit_system)}" }
                                                        td { class: "stock-name-cell", "{tool.display_name}" }
                                                        td { "{sku}" }
                                                        td { "{unit_format::format_angle_display(tool.point_angle)}" }
                                                        td { "{feed}" }
                                                        td { "{speed}" }
                                                    }
                                                }
                                            })}
                                        }
                                    })}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

use dioxus::prelude::*;
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
    // One line for everything this screen reports — imports, deletions, and adding a
    // tool to stock. Sticky rather than a toast, matching the Stock screen beside it.
    let feedback = use_signal(String::new);

    rsx! {
        div { class: "screen single",
            CatalogManagementPanel { state, feedback }
        }
    }
}

/// Catalog list + import/delete controls, plus a read-only view of the selected
/// catalog's tools (the same table shape the Stock screen uses).
#[component]
fn CatalogManagementPanel(
    state: Signal<crate::runtime::AppCtx>,
    feedback: Signal<String>,
) -> Element {
    let mut viewing_catalog_key = use_signal(|| None::<String>);
    let mut detail_tool_key = use_signal(|| None::<String>);

    use_effect(move || {
        super::mutate_ctx(state, |s| s.ensure_catalogs_loaded());
    });

    // Adding a tool to stock writes to the datastore, which this screen would not
    // otherwise notice — it has no stock of its own to redraw, but the "already in
    // stock" warning is answered from the projection and would go stale. Same effect
    // the Stock screen runs.
    use_effect(move || {
        let _ = crate::ui::bindings::data_revision();
        crate::ui::bindings::refresh_legacy_stock();
        state.set(crate::runtime::ctx_snapshot());
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

    // Resolved every render rather than held, because a catalog tool's key is its
    // *position* (`<catalog>::s<n>::t<m>`) and re-importing a catalog shifts every key
    // after the change. A stale key must therefore resolve to nothing and close the
    // panel, never to whichever tool has since taken that slot.
    let detail_tool = detail_tool_key.read().clone().and_then(|key| {
        viewed_catalog
            .iter()
            .flat_map(|catalog| catalog.sections.iter())
            .flat_map(|section| section.tools.iter())
            .find(|tool| tool.key == key)
            .cloned()
    });

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
                            spawn(async move {
                                let picked = super::profiles_common::pick_import_file(
                                    "Import catalog",
                                    "Catalog YAML",
                                )
                                .await;

                                let Some(path) = picked else {
                                    feedback.set("Catalog import canceled".to_string());
                                    return;
                                };

                                let text = match fs::read_to_string(&path) {
                                    Ok(text) => text,
                                    Err(_) => {
                                        feedback
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
                                        Ok(name) => feedback.set(format!("Catalog '{name}' imported")),
                                        Err(msg) => feedback.set(msg),
                                    });
                            });
                        },
                        "Import catalog"
                    }
                }

                if !feedback.read().is_empty() {
                    p { class: "diag-status", "{feedback.read()}" }
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
                                                                    Ok(_) => feedback.set("Catalog deleted".to_string()),
                                                                    Err(msg) => feedback.set(msg),
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

            // One tool, opened from the table below. Replaces the table rather than
            // floating over it, which is how the Stock screen's tool detail behaves.
            if let Some(tool) = detail_tool.as_ref() {
                CatalogToolDetail {
                    tool: tool.clone(),
                    unit_system,
                    feedback,
                    on_close: move |_| detail_tool_key.set(None),
                }
            }

            // Read-only contents of the selected catalog — the same tool table the
            // Stock screen shows, so tools can be inspected without adding them.
            if let (Some(catalog), None) = (viewed_catalog, detail_tool.as_ref()) {
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
                                                    .table_feed
                                                    .map(|f| unit_format::format_feed_display(f, unit_system))
                                                    .unwrap_or_else(|| "\u{2014}".to_string());
                                                let speed = tool
                                                    .spindle_speed
                                                    .map(|s| unit_format::format_rotational_speed_display(s))
                                                    .unwrap_or_else(|| "\u{2014}".to_string());
                                                rsx! {
                                                    tr {
                                                        key: "{tool.key}",
                                                        class: "catalog-tool-table-row",
                                                        // The gesture the Stock table already uses for the
                                                        // same thing, on the screen where tools are compared.
                                                        ondoubleclick: {
                                                            let key = tool.key.clone();
                                                            move |_| detail_tool_key.set(Some(key.clone()))
                                                        },
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

/// One catalog tool, read-only, with the one action a catalog offers: put it in stock.
///
/// Read-only because a catalog is: the libraries are files the user supplies, and stock
/// tools are *copies*, so everything editable about a tool becomes editable the moment
/// it is added. This is the reference side of that pair.
///
/// It shows three values that appear nowhere else in the application — flute length,
/// minimum depth and the plunge feed — each of which the planner relies on: flute length
/// is what decides whether a cutter can reach through the board, and the plunge rating is
/// carried into stock rather than re-derived from the lateral feed.
#[component]
fn CatalogToolDetail(
    tool: crate::data::model::CatalogStockTool,
    unit_system: crate::data::model::UserUnitSystem,
    feedback: Signal<String>,
    on_close: EventHandler<()>,
) -> Element {
    let mut feedback = feedback;
    let length = |value: Option<units::Length>| {
        value
            .map(|v| unit_format::format_length_display(v, unit_system))
            .unwrap_or_else(|| "\u{2014}".to_string())
    };
    let feed = |value: Option<units::FeedRate>| {
        value
            .map(|v| unit_format::format_feed_display(v, unit_system))
            .unwrap_or_else(|| "\u{2014}".to_string())
    };

    let rows: Vec<(&'static str, String)> = vec![
        ("Name", tool.display_name.clone()),
        ("SKU", tool.sku.clone().unwrap_or_else(|| "\u{2014}".to_string())),
        ("Type", tool.kind.clone()),
        ("Diameter", unit_format::format_length_display(tool.diameter, unit_system)),
        ("Point angle", unit_format::format_angle_display(tool.point_angle)),
        ("Flute length", length(tool.flute_length)),
        ("Minimum depth", length(tool.z_min_depth)),
        ("Table feed (XY)", feed(tool.table_feed)),
        ("Plunge feed (Z)", feed(tool.z_feed)),
        (
            "Spindle speed",
            tool.spindle_speed
                .map(unit_format::format_rotational_speed_display)
                .unwrap_or_else(|| "\u{2014}".to_string()),
        ),
    ];

    let tool_key = tool.key.clone();

    rsx! {
        article { class: "setup-card",
            div { class: "panel-header",
                div {
                    h3 { "{tool.display_name}" }
                    p { "From the catalog. Adding it puts a copy in stock, which is yours to edit." }
                }
                div { class: "actions",
                    button {
                        class: "btn btn-secondary",
                        onclick: move |_| on_close.call(()),
                        "Back"
                    }
                    button {
                        class: "btn btn-primary",
                        onclick: move |_| {
                            // Warned about, then added anyway: a second copy of a tool is
                            // a real thing to own, with its own wear and its own
                            // in/out-of-stock state. It arrives named apart so the two can
                            // be told apart in the rack picker and the tooling plan.
                            let duplicate =
                                crate::ui::bindings::catalog_tool_already_in_stock(&tool_key);
                            match crate::ui::bindings::add_catalog_tool_to_stock(&tool_key) {
                                Some(name) if duplicate => feedback.set(format!(
                                    "Already in stock — added another copy as '{name}'"
                                )),
                                Some(name) => feedback.set(format!("Added '{name}' to stock")),
                                None => feedback
                                    .set("That tool is no longer in the catalog".to_string()),
                            }
                        },
                        "Add to stock"
                    }
                }
            }

            div { class: "stock-detail-form",
                for (label , value) in rows.into_iter() {
                    div { key: "{label}", class: "field",
                        label { "{label}" }
                        div { class: "stock-detail-readonly", "{value}" }
                    }
                }
            }
        }
    }
}

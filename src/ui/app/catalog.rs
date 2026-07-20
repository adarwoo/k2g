use dioxus::prelude::*;
use rfd::FileDialog;
use std::fs;

/// Catalog management screen — imports supplier catalogs and lists the stock
/// sources available to the tool picker. Import/list/delete run against the
/// legacy catalog manager on the context; the datastore exposes catalogs
/// read-only via `AppData::catalogs` for reference resolution.
#[component]
pub fn CatalogScreen(state: Signal<crate::app_state_impl::AppCtx>) -> Element {
    let import_feedback = use_signal(String::new);

    rsx! {
        div { class: "screen single",
            CatalogManagementPanel { state, import_feedback }
        }
    }
}

/// Catalog list + import/delete controls. Relocated here from the (now removed)
/// setup screen, which was the only other user.
#[component]
fn CatalogManagementPanel(
    state: Signal<crate::app_state_impl::AppCtx>,
    import_feedback: Signal<String>,
) -> Element {
    use_effect(move || {
        super::mutate_ctx(state, |s| s.ensure_catalogs_loaded());
    });

    let snapshot = state.read().clone();

    rsx! {
        section { class: "setup-stage",
            div { class: "setup-stage-header",
                h2 { "Catalog management" }
                p {
                    "Import supplier catalogs and manage the stock sources available to the tool picker."
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
                                                    move |_| {
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
        }
    }
}

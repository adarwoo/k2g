use dioxus::prelude::*;
use rfd::FileDialog;
use std::fs;

use super::setup::{cnc_profile_library, parse_machine_profile_yaml};
use super::super::model::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SetupTab {
    General,
    Cnc,
    Catalogs,
}

impl SetupTab {
    pub fn label(self) -> &'static str {
        match self {
            Self::General => "General Settings",
            Self::Cnc => "CNC Profiles",
            Self::Catalogs => "Catalogs",
        }
    }

    pub fn caption(self) -> &'static str {
        match self {
            Self::General => "Runtime and display",
            Self::Cnc => "Profile library and import",
            Self::Catalogs => "Installed stock catalogs",
        }
    }
}

#[component]
pub fn SetupSidebar(active_tab: Signal<SetupTab>) -> Element {
    let tabs = [SetupTab::General, SetupTab::Cnc, SetupTab::Catalogs];

    rsx! {
        aside { class: "setup-sidebar",
            div { class: "setup-sidebar-header",
                div { class: "setup-eyebrow", "Setup" }
                div { class: "setup-sidebar-title", "System configuration" }
            }
            div { class: "setup-sidebar-list",
                for tab in tabs {
                    button {
                        class: if *active_tab.read() == tab { "setup-sidebar-button active" } else { "setup-sidebar-button" },
                        onclick: move |_| active_tab.set(tab),
                        span { class: "setup-sidebar-button-title", "{tab.label()}" }
                        span { class: "setup-sidebar-button-caption", "{tab.caption()}" }
                    }
                }
            }
        }
    }
}

#[component]
pub fn GeneralSettingsPanel(
    state: Signal<UiState>,
    kicad_status: String,
    board_snapshot_summary: Option<String>,
) -> Element {
    let snapshot = state.read().clone();

    rsx! {
        section { class: "setup-stage",
            div { class: "setup-stage-header",
                h2 { "General settings" }
                p { "Apply global display preferences and inspect runtime diagnostics." }
            }

            div { class: "setup-card-grid two-up",
                article { class: "setup-card",
                    h3 { "Display" }
                    div { class: "field",
                        label { "Theme" }
                        select {
                            value: snapshot.theme.as_str(),
                            onchange: move |evt| {
                                let value = evt.value();
                                state
                                    .with_mut(|s| {
                                        s.theme = if value == "light" { Theme::Light } else { Theme::Dark };
                                    });
                            },
                            option { value: "dark", "Dark" }
                            option { value: "light", "Light" }
                        }
                    }
                }

                article { class: "setup-card",
                    h3 { "Runtime diagnostics" }
                    p { class: "diag-status", "{kicad_status}" }
                    if let Some(summary) = board_snapshot_summary.as_ref() {
                        p { class: "diag-status", "{summary}" }
                    } else {
                        p { class: "diag-status", "Board snapshot: unavailable" }
                    }
                }
            }
        }
    }
}

#[component]
pub fn MachineProfilesPanel(
    state: Signal<UiState>,
    selected_library_profile: Signal<String>,
    import_feedback: Signal<String>,
) -> Element {
    let snapshot = state.read().clone();
    let library_profiles = cnc_profile_library();

    rsx! {
        section { class: "setup-stage",
            div { class: "setup-stage-header",
                h2 { "CNC profiles" }
                p { "Add a machine from the built-in library or import a CNC profile YAML file." }
            }

            if !import_feedback.read().is_empty() {
                p { class: "diag-status", "{import_feedback.read()}" }
            }

            div { class: "setup-card-grid two-up",
                article { class: "setup-card",
                    h3 { "Add from library" }
                    div { class: "field",
                        label { "Library profile" }
                        select {
                            value: selected_library_profile.read().clone(),
                            onchange: move |evt| selected_library_profile.set(evt.value()),
                            for profile in library_profiles.iter() {
                                option { value: profile.key.clone(), "{profile.name}" }
                            }
                        }
                    }
                    button {
                        class: "btn btn-primary",
                        onclick: {
                            let profiles = library_profiles.clone();
                            move |_| {
                                let key = selected_library_profile.read().clone();
                                let selected = profiles.iter().find(|profile| profile.key == key).cloned();
                                if let Some(profile) = selected {
                                    state.with_mut(|s| s.add_machine_profile(profile.machine));
                                    import_feedback.set("CNC profile added from library".to_string());
                                } else {
                                    import_feedback
                                        .set("Unable to add CNC profile from library".to_string());
                                }
                            }
                        },
                        "Add CNC profile"
                    }
                }

                article { class: "setup-card",
                    h3 { "Import profile" }
                    p { "Supported file names must end in .cnc-profile.yaml or .cnc-profile.yml." }
                    button {
                        class: "btn btn-secondary",
                        onclick: move |_| {
                            let picked = FileDialog::new()
                                .set_title("Import CNC profile")
                                .add_filter("CNC profile YAML", &["yaml", "yml"])
                                .pick_file();

                            let Some(path) = picked else {
                                import_feedback.set("Import canceled".to_string());
                                return;
                            };

                            let file_name = path
                                .file_name()
                                .and_then(|name| name.to_str())
                                .unwrap_or_default()
                                .to_ascii_lowercase();
                            let valid_name = file_name.ends_with(".cnc-profile.yaml")
                                || file_name.ends_with(".cnc-profile.yml");
                            if !valid_name {
                                import_feedback
                                    .set(
                                        "CNC profile import failed: file name must end with .cnc-profile.yaml or .cnc-profile.yml"
                                            .to_string(),
                                    );
                                return;
                            }
                            let text = match fs::read_to_string(&path) {
                                Ok(text) => text,
                                Err(_) => {
                                    import_feedback
                                        .set("CNC profile import failed: file not readable".to_string());
                                    return;
                                }
                            };
                            let path_str = path.to_string_lossy().to_string();
                            let profile = match parse_machine_profile_yaml(&text, &path_str) {
                                Some(profile) => profile,
                                None => {
                                    import_feedback
                                        .set(
                                            "CNC profile import failed: file is missing required machine fields"
                                                .to_string(),
                                        );
                                    return;
                                }
                            };
                            state.with_mut(|s| s.add_machine_profile(profile));
                            import_feedback.set("CNC profile imported and selected".to_string());
                        },
                        "Import CNC profile"
                    }
                }
            }

            article { class: "setup-card setup-card-list",
                div { class: "panel-header",
                    h3 { "Installed profiles" }
                    span { class: "diag-status", "{snapshot.machines.len()} total" }
                }

                if snapshot.machines.is_empty() {
                    p { class: "diag-status", "No CNC profiles added yet." }
                } else {
                    div { class: "profile-list",
                        for machine in snapshot.machines.iter() {
                            div { class: if snapshot.selected_machine_id.as_ref() == Some(&machine.id) { "profile-list-item active" } else { "profile-list-item" },
                                div {
                                    div { class: "profile-list-title", "{machine.name}" }
                                    div { class: "profile-list-meta",
                                        "Fixture {machine.fixture_plate_max_x} x {machine.fixture_plate_max_y} mm · ATC {machine.atc_slot_count}"
                                    }
                                }
                                button {
                                    class: "btn btn-small",
                                    onclick: {
                                        let machine_id = machine.id.clone();
                                        move |_| {
                                            state.with_mut(|s| s.select_machine_profile_by_id(Some(machine_id.clone())))
                                        }
                                    },
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

#[component]
pub fn CatalogManagementPanel(state: Signal<UiState>, import_feedback: Signal<String>) -> Element {
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
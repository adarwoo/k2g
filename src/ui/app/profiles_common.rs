use super::super::model::CascadeDeleteImpact;
use dioxus::prelude::*;

pub fn format_impact_warning(prefix: &str, impact: &CascadeDeleteImpact) -> String {
    let mut lines = vec![prefix.to_string()];
    for item in impact.primary_profiles.iter() {
        lines.push(format!("- {}", item));
    }
    for item in impact.dependent_process_profiles.iter() {
        lines.push(format!("- {}", item));
    }
    for item in impact.deleted_live_projects.iter() {
        lines.push(format!("- {}", item));
    }
    lines.join("\n")
}

pub fn format_impact_summary(prefix: &str, impact: &CascadeDeleteImpact) -> String {
    format!(
        "{}: {} primary, {} dependent process profile(s), {} live project(s)",
        prefix,
        impact.primary_profiles.len(),
        impact.dependent_process_profiles.len(),
        impact.deleted_live_projects.len()
    )
}

pub fn slug_file_name(value: &str, fallback: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }

    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed
    }
}

#[component]
pub fn ProfileLifecycleToolbar(
    profile_type_label: String,
    profiles: Vec<(String, String)>,
    selected_profile_id: Option<String>,
    can_export: bool,
    on_select: EventHandler<String>,
    on_clone: EventHandler<MouseEvent>,
    on_export: EventHandler<MouseEvent>,
    on_delete: EventHandler<MouseEvent>,
    on_add: EventHandler<MouseEvent>,
    on_import: EventHandler<MouseEvent>,
) -> Element {
    let has_profiles = !profiles.is_empty();

    rsx! {
        div { class: "actions profile-actions",
            if has_profiles {
                select {
                    class: "stock-toolbar-select",
                    value: selected_profile_id.unwrap_or_default(),
                    onchange: move |evt| on_select.call(evt.value()),
                    for (id , name) in profiles.into_iter() {
                        option { value: "{id}", "{name}" }
                    }
                }
                button {
                    class: "btn btn-secondary",
                    title: "Clone selected profile",
                    onclick: move |evt| on_clone.call(evt),
                    "Clone"
                }
                if can_export {
                    button {
                        class: "btn btn-secondary",
                        title: "Export selected profile",
                        onclick: move |evt| on_export.call(evt),
                        "Export"
                    }
                }
                button {
                    class: "btn btn-danger",
                    title: "Delete selected profile",
                    onclick: move |evt| on_delete.call(evt),
                    "Delete"
                }
            }
        }
        div { class: "actions global-actions",
            button {
                class: "btn btn-primary",
                title: "Add a profile",
                onclick: move |evt| on_add.call(evt),
                "Add {profile_type_label}"
            }
            button {
                class: "btn btn-secondary",
                title: "Import profile from file",
                onclick: move |evt| on_import.call(evt),
                "Import"
            }
        }
    }
}

#[component]
pub fn ProfileNameDialog(
    title: String,
    name_label: String,
    name_value: String,
    template_options: Vec<(String, String)>,
    selected_template: String,
    on_name_change: EventHandler<String>,
    on_template_change: EventHandler<String>,
    on_cancel: EventHandler<()>,
    on_submit: EventHandler<()>,
) -> Element {
    let has_templates = !template_options.is_empty();

    rsx! {
        div { class: "wizard-overlay",
            div { class: "wizard-dialog",
                h2 { "{title}" }
                div { class: "field",
                    label { "{name_label}" }
                    input {
                        value: name_value,
                        autofocus: true,
                        onmounted: move |evt| async move {
                            let _ = evt.set_focus(true).await;
                        },
                        oninput: move |evt| on_name_change.call(evt.value()),
                        onkeydown: move |evt| {
                            let key = evt.key().to_string().to_ascii_lowercase();
                            if key == "escape" || key == "esc" {
                                on_cancel.call(());
                            }
                            if key == "enter" || key == "numpadenter" {
                                on_submit.call(());
                            }
                        },
                    }
                }

                if has_templates {
                    div { class: "field",
                        label { "Template" }
                        select {
                            value: selected_template,
                            onchange: move |evt| on_template_change.call(evt.value()),
                            for (id , label) in template_options.into_iter() {
                                option { value: "{id}", "{label}" }
                            }
                        }
                    }
                }

                div { class: "wizard-actions",
                    button {
                        class: "btn btn-secondary",
                        onclick: move |_| on_cancel.call(()),
                        "Cancel"
                    }
                    button {
                        class: "btn btn-primary",
                        onclick: move |_| on_submit.call(()),
                        "Add"
                    }
                }
            }
        }
    }
}

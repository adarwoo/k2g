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

#[allow(dead_code)]
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

fn sort_uuid_v7_strings(ids: &mut Vec<String>) {
    ids.sort();
    ids.dedup();
}

#[component]
pub fn BindingListSelector(
    label: String,
    options: Vec<(String, String)>,
    selected_ids: Vec<String>,
    default_id: String,
    on_change: EventHandler<(Vec<String>, String)>,
) -> Element {
    let mut is_open = use_signal(|| false);
    let requires_default = default_id.trim().is_empty();

    let mut ordered_options = options.clone();
    ordered_options.sort_by(|(a, _), (b, _)| a.cmp(b));

    let mut selected = selected_ids
        .iter()
        .filter(|id| ordered_options.iter().any(|(candidate, _)| candidate == *id))
        .cloned()
        .collect::<Vec<_>>();
    sort_uuid_v7_strings(&mut selected);

    let mut default_selected = default_id.clone();
    if !default_selected.is_empty() && !selected.iter().any(|id| id == &default_selected) {
        selected.push(default_selected.clone());
        sort_uuid_v7_strings(&mut selected);
    }
    if default_selected.is_empty() {
        default_selected = selected.last().cloned().unwrap_or_default();
    }

    let mut summary_rows = selected
        .iter()
        .filter_map(|id| {
            ordered_options
                .iter()
                .find(|(candidate, _)| candidate == id)
                .map(|(_, label)| (id.clone(), label.clone()))
        })
        .collect::<Vec<_>>();
    if let Some(idx) = summary_rows
        .iter()
        .position(|(id, _)| id == &default_selected)
    {
        let item = summary_rows.remove(idx);
        summary_rows.insert(0, item);
    }

    rsx! {
        if *is_open.read() {
            div {
                class: "binding-selector-backdrop",
                onclick: move |_| is_open.set(false),
            }
        }

        div { class: if *is_open.read() { "binding-selector open" } else { "binding-selector" },
            div { class: "binding-selector-header",
                label { "{label}" }
                button {
                    r#type: "button",
                    class: "btn btn-secondary btn-small",
                    onclick: move |_| {
                        let currently_open = *is_open.read();
                        is_open.set(!currently_open);
                    },
                    if *is_open.read() {
                        "Done"
                    } else {
                        "Edit"
                    }
                }
            }

            div {
                class: if *is_open.read() { if requires_default {
                    "binding-selector-body open pending"
                } else {
                    "binding-selector-body open"
                } } else if requires_default { "binding-selector-body pending" } else { "binding-selector-body" },
                tabindex: "0",
                onclick: move |_| {
                    if !*is_open.read() {
                        is_open.set(true);
                    }
                },

                if *is_open.read() {
                    div { class: "binding-selector-editor",
                        for (id , name) in ordered_options.iter() {
                            {
                                let option_id = id.clone();
                                let option_name = name.clone();
                                let is_selected = selected.iter().any(|existing| existing == &option_id);
                                let is_default = default_selected == option_id;
                                rsx! {
                                    div {
                                        key: "opt-{option_id}",
                                        class: if is_selected { if is_default {
                                            "binding-edit-row selected default"
                                        } else {
                                            "binding-edit-row selected"
                                        }
                                        } else {
                                        "binding-edit-row"
                                        },
                                        input {
                                            r#type: "checkbox",
                                            checked: is_selected,
                                            oninput: {
                                                let option_id = option_id.clone();
                                                let current_default = default_selected.clone();
                                                let current_selected = selected.clone();
                                                move |_| {
                                                    let mut next_selected = current_selected.clone();
                                                    let already_selected = next_selected
                                                        .iter()
                                                        .any(|existing| existing == &option_id);

                                                    if already_selected {
                                                        next_selected.retain(|existing| existing != &option_id);
                                                    } else {
                                                        next_selected.push(option_id.clone());
                                                    }
                                                    sort_uuid_v7_strings(&mut next_selected);

                                                    let mut next_default = current_default.clone();
                                                    if next_selected.is_empty() {
                                                        next_default.clear();
                                                    } else if !already_selected {
                                                        // Last selected item becomes the default.
                                                        next_default = option_id.clone();
                                                    } else if !next_selected
                                                        .iter()
                                                        .any(|existing| existing == &next_default)
                                                    {
                                                        next_default = next_selected
                                                            .last()
                                                            .cloned()
                                                            .unwrap_or_default();
                                                    }

                                                    on_change.call((next_selected, next_default));
                                                }
                                            },
                                        }
                                        span { class: if is_default { "binding-edit-row-default-tick default" } else { "binding-edit-row-default-tick" },
                                            if is_default {
                                                "\u{2713}"
                                            } else {
                                                ""
                                            }
                                        }
                                        span { class: if is_default { "binding-edit-row-label selected default" } else if is_selected { "binding-edit-row-label selected" } else { "binding-edit-row-label" },
                                            "{option_name}"
                                        }
                                    }
                                }
                            }
                        }
                    }
                } else if summary_rows.is_empty() {
                    div { class: "binding-selector-empty", "No selection" }
                } else {
                    div { class: "binding-selector-summary-list",
                        for (id , name) in summary_rows.iter() {
                            div {
                                key: "sum-{id}",
                                class: if id == &default_selected { "binding-list-row default" } else { "binding-list-row" },
                                span { class: if id == &default_selected { "binding-list-row-tick default" } else { "binding-list-row-tick" },
                                    if id == &default_selected {
                                        "\u{2713}"
                                    } else {
                                        ""
                                    }
                                }
                                span { class: if id == &default_selected { "binding-list-row-label default" } else { "binding-list-row-label" },
                                    "{name}"
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
                    for (idx , (id , name)) in profiles.into_iter().enumerate() {
                        option { key: "profile-opt-{idx}", value: "{id}", "{name}" }
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
                            for (idx , (id , label)) in template_options.into_iter().enumerate() {
                                option { key: "template-opt-{idx}", value: "{id}", "{label}" }
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

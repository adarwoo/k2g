use std::path::PathBuf;
use std::sync::Arc;

use crate::data::model::CascadeDeleteImpact;
use dioxus::desktop::tao::window::Window;
use dioxus::prelude::*;

/// The application window, to own a native dialog.
///
/// # Never open a blocking dialog from an event handler
///
/// `rfd::FileDialog` / `rfd::MessageDialog` (the non-`Async` types) run the platform's own
/// **modal message pump**. Called from inside a Dioxus event handler, that pump re-enters
/// tao's event loop → `App::poll_vdom` → `VirtualDom::render_immediate`, while dioxus-core
/// is still holding a borrow of the element arena for the event being dispatched. The
/// result is `RefCell already borrowed`, and then a second panic as the first unwinds
/// through the dialog component's props — an abort, mid-save.
///
/// It is not enough to avoid touching a signal beforehand. The removable-media watcher
/// bumps the UI wake channel every couple of seconds
/// ([`crate::runtime::removable`]), so `AppRoot` re-syncs and a render is pending
/// regardless — any dialog held open long enough is exposed.
///
/// So: always the `Async*` types, always driven by `spawn`. rfd runs those on a thread of
/// their own and wakes the future when they close, so the event loop is never inside a
/// nested pump, and `spawn` returns immediately so dispatch finishes and drops its borrows
/// before the dialog is even shown. The helpers below are the sanctioned way in;
/// `no_blocking_native_dialogs_in_the_screens` fails the build's tests if a blocking one
/// reappears.
pub fn dialog_parent() -> Arc<Window> {
    dioxus::desktop::window().window.clone()
}

/// A yes/no confirmation, owned by the application window.
pub async fn confirm(title: &str, body: &str) -> bool {
    rfd::AsyncMessageDialog::new()
        .set_parent(&*dialog_parent())
        .set_level(rfd::MessageLevel::Warning)
        .set_title(title)
        .set_description(body)
        .set_buttons(rfd::MessageButtons::YesNo)
        .show()
        .await
        == rfd::MessageDialogResult::Yes
}

/// "Where shall I read this from?" — `None` when cancelled.
pub async fn pick_import_file(title: &str, filter_label: &str) -> Option<PathBuf> {
    rfd::AsyncFileDialog::new()
        .set_parent(&*dialog_parent())
        .set_title(title)
        .add_filter(filter_label, &["yaml", "yml"])
        .pick_file()
        .await
        .map(|handle| handle.path().to_path_buf())
}

/// "Where shall I write this?" — `None` when cancelled.
pub async fn pick_export_file(
    title: &str,
    filter_label: &str,
    suggested_name: &str,
) -> Option<PathBuf> {
    rfd::AsyncFileDialog::new()
        .set_parent(&*dialog_parent())
        .set_title(title)
        .set_file_name(suggested_name)
        .add_filter(filter_label, &["yaml", "yml"])
        .save_file()
        .await
        .map(|handle| handle.path().to_path_buf())
}

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

/// The default name to suggest when adding a new profile: `My <type>`, or the next
/// free `My <type> N` when that base is taken (e.g. `My fixture`, then `My fixture 2`).
///
/// A profile whose required `name` is left blank is unusable and blocks generation, so
/// the add dialog is pre-filled with this instead of an empty field. `existing` is the
/// current profile names of that kind; matching is on the trimmed name.
pub fn suggested_profile_name(type_label: &str, existing: &[String]) -> String {
    let base = format!("My {}", type_label.to_lowercase());
    let taken = |candidate: &str| existing.iter().any(|name| name.trim() == candidate);
    if !taken(&base) {
        return base;
    }
    let mut n = 2;
    loop {
        let candidate = format!("{base} {n}");
        if !taken(&candidate) {
            return candidate;
        }
        n += 1;
    }
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
    let selected_id = selected_profile_id.unwrap_or_default();

    // Which option the list must show as chosen, decided here rather than in the markup.
    //
    // Dioxus does not reflect a `<select>`'s `value:` onto the rendered element, so an
    // option that does not say `selected:` for itself leaves the browser showing the
    // first entry — whatever the screen actually has open. That is how a screen opening
    // on the job's machining profile would still read as if the first profile were
    // loaded. (Same fix as the rack slot picker and the job sidebar's profile select.)
    let options = profiles
        .into_iter()
        .map(|(id, name)| {
            let is_selected = id == selected_id;
            (id, name, is_selected)
        })
        .collect::<Vec<_>>();

    rsx! {
        div { class: "actions profile-actions",
            if has_profiles {
                select {
                    class: "stock-toolbar-select",
                    value: "{selected_id}",
                    onchange: move |evt| on_select.call(evt.value()),
                    for (idx , (id , name , is_selected)) in options.into_iter().enumerate() {
                        option {
                            key: "profile-opt-{idx}",
                            value: "{id}",
                            selected: is_selected,
                            "{name}"
                        }
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

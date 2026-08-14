//! The Settings dialog: preferences that have no natural home elsewhere in the UI.
//!
//! A dialog rather than a screen, reached from the cog in the top bar and floating over
//! whatever is in view. These are things a user changes once and then leaves alone, so
//! they do not earn a permanent seat in the navigation rail beside the screens that make
//! up the actual work.
//!
//! The theme lives here. It had its own top-bar button until that button became the cog
//! that opens this dialog — a palette is picked once and forgotten, which is not worth
//! standing chrome. The *unit system* is the opposite case and stays in the top bar,
//! deliberately unmirrored here: it is changed constantly while reading a job, and a
//! preference with two homes has two places to be changed and two places to look wrong.
//!
//! The opt-out cards carry their regulatory rationale in prose, on screen, because
//! EU CRA Annex I (2)(c) and (2)(l) ask for a *clear* opt-out — a bare switch labelled
//! "updates" does not tell the user what they are turning off or what it costs them.

use dioxus::prelude::*;

use crate::runtime::kicad_integration::{
    self, IntegrationStatus, KicadInstall, KicadRunning,
};
use crate::ui::navigation::Theme;

/// One labelled switch with an explanation beneath it.
///
/// `detail` is not decoration: for every switch here it is the only place
/// the user learns what the setting actually does (what leaves the machine, what gets
/// written to disk). Kept as a separate element so it can be styled as secondary text
/// without becoming a tooltip nobody opens.
#[component]
fn ToggleRow(
    label: String,
    detail: String,
    checked: bool,
    on_toggle: EventHandler<bool>,
) -> Element {
    rsx! {
        div { class: "settings-toggle-row",
            label { class: "settings-toggle",
                input {
                    r#type: "checkbox",
                    checked,
                    onchange: move |evt| on_toggle.call(evt.checked()),
                }
                span { class: "settings-toggle-label", "{label}" }
            }
            p { class: "settings-toggle-detail", "{detail}" }
        }
    }
}

/// One detected KiCad version, with the two actions that apply to it.
///
/// Every action states the exact path it will touch before it is taken. That is the
/// point of the card: these are the only two things k2g does outside its own data
/// directory, and a user should never have to guess what a button will change in
/// another application's installation.
#[component]
fn KicadInstallRow(
    install: KicadInstall,
    status: IntegrationStatus,
    running: KicadRunning,
    on_changed: EventHandler<Result<String, String>>,
) -> Element {
    let version = install.version.clone();
    let common = install.common_file.display().to_string();
    let plugins = install.plugins_dir.display().to_string();

    // Enabling the API edits a file KiCad rewrites on exit, so it is refused outright
    // while KiCad is up. Registration only writes into the plugins directory, which
    // KiCad reads at startup, so it is safe at any time.
    let block_api_edit = running == KicadRunning::Yes;
    let api_note = match (status.api_enabled, running) {
        (true, _) => "The IPC API is on. k2g can talk to this KiCad.".to_string(),
        (false, KicadRunning::Yes) => {
            "The IPC API is off, and KiCad is running. Close KiCad first — it rewrites \
             its settings file when it exits, so a change made now would be discarded."
                .to_string()
        }
        (false, KicadRunning::Unknown) => {
            "The IPC API is off. k2g cannot tell whether KiCad is running on this \
             platform — close it before enabling, or the change will be overwritten \
             when it exits."
                .to_string()
        }
        (false, KicadRunning::No) => {
            "The IPC API is off, so no plugin can reach this KiCad. Enabling it sets \
             api.enable_server in the file below; everything else in it is left alone \
             and a .k2g-backup copy is written first."
                .to_string()
        }
    };

    let registration_note = if status.stale {
        "Registered, but pointing at a different k2g build. Re-register to fix it."
            .to_string()
    } else if status.registered {
        "Registered. KiCad shows a Create GCode button on the PCB editor toolbar.".to_string()
    } else {
        format!("Not registered. Registering creates {plugins}\\k2g and adds the toolbar button.")
    };

    let install_for_api = install.clone();
    let install_for_register = install.clone();
    let install_for_unregister = install.clone();
    let version_for_api = version.clone();
    let version_for_register = version.clone();
    let version_for_unregister = version.clone();

    rsx! {
        div { class: "kicad-install",
            div { class: "kicad-install-head",
                h3 { class: "kicad-install-title", "KiCad {version}" }
                span {
                    class: if status.api_enabled { "kicad-badge on" } else { "kicad-badge off" },
                    if status.api_enabled { "API on" } else { "API off" }
                }
                span {
                    class: if status.stale {
                        "kicad-badge warn"
                    } else if status.registered {
                        "kicad-badge on"
                    } else {
                        "kicad-badge off"
                    },
                    if status.stale {
                        "plugin stale"
                    } else if status.registered {
                        "plugin registered"
                    } else {
                        "plugin not registered"
                    }
                }
            }

            p { class: "kicad-install-note", "{api_note}" }
            p { class: "kicad-install-path mono", "{common}" }

            if !status.api_enabled {
                button {
                    class: "text-button",
                    disabled: block_api_edit,
                    onclick: move |_| {
                        let outcome = kicad_integration::set_api_enabled(&install_for_api, true)
                            .map(|()| {
                                format!(
                                    "Enabled KiCad {version_for_api}'s IPC API. Restart KiCad for it to take effect.",
                                )
                            })
                            .map_err(|err| err.to_string());
                        on_changed.call(outcome);
                    },
                    "Enable the KiCad API"
                }
            }

            p { class: "kicad-install-note", "{registration_note}" }

            div { class: "kicad-install-actions",
                button {
                    class: "text-button",
                    onclick: move |_| {
                        let outcome = kicad_integration::register(&install_for_register)
                            .map(|dir| {
                                format!(
                                    "Registered k2g with KiCad {version_for_register} at {}. Restart KiCad to see the button.",
                                    dir.display(),
                                )
                            })
                            .map_err(|err| err.to_string());
                        on_changed.call(outcome);
                    },
                    if status.registered { "Re-register" } else { "Register with KiCad" }
                }
                if status.registered {
                    button {
                        class: "text-button",
                        onclick: move |_| {
                            let outcome = kicad_integration::unregister(&install_for_unregister)
                                .map(|()| {
                                    format!(
                                        "Removed the k2g plugin from KiCad {version_for_unregister}. Restart KiCad to clear the button.",
                                    )
                                })
                                .map_err(|err| err.to_string());
                            on_changed.call(outcome);
                        },
                        "Unregister"
                    }
                }
            }
        }
    }
}

#[component]
pub fn SettingsDialog(
    state: Signal<crate::runtime::AppCtx>,
    on_close: EventHandler<()>,
) -> Element {
    let snapshot = state.read().clone();

    let update_checks_on = snapshot.update_check_enabled;
    let security_log_on = snapshot.security_log_enabled;

    // Surfacing the *state* of a postpone/skip matters as much as offering the action:
    // a user who skipped a version months ago and then wonders why they see nothing
    // has no other way to discover why, or to undo it.
    let skipped = snapshot.update_skipped_version.clone();
    let postponed = snapshot.update_postponed_until.clone();
    let last_check = snapshot
        .update_last_check
        .as_deref()
        .and_then(|stamp| chrono::DateTime::parse_from_rfc3339(stamp).ok())
        .map(|when| {
            when.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        });

    // Re-read the filesystem when `probe` is bumped rather than on every render:
    // detection walks two directory trees and enumerates processes, which is far too
    // much to repeat on an unrelated signal change.
    let mut probe = use_signal(|| 0u32);
    let installs = use_memo(move || {
        let _ = probe.read();
        let running = kicad_integration::kicad_is_running();
        let rows: Vec<(KicadInstall, IntegrationStatus)> = kicad_integration::detect_installs()
            .into_iter()
            .map(|install| {
                let status = kicad_integration::status(&install);
                (install, status)
            })
            .collect();
        (rows, running)
    });
    let (kicad_rows, kicad_running) = installs.read().clone();

    // Named in full in the danger card: a destructive action must say exactly what it
    // will remove, not "your data".
    let data_dir = crate::runtime::data_lifecycle::data_dir()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "(cannot locate the data directory)".to_string());

    rsx! {
        div {
            class: "wizard-overlay",
            // Dismissing on the backdrop is safe here in a way it would not be in a form
            // dialog: every control inside takes effect the moment it is used, so there
            // is no half-finished edit for a stray click to throw away.
            onclick: move |_| on_close.call(()),

            div {
                class: "wizard-dialog settings-dialog",
                "role": "dialog",
                "aria-modal": "true",
                "aria-label": "Settings",
                // Focused on mount so Escape has somewhere to land. Unlike
                // `ProfileNameDialog` there is no input here to hang the handler on, and
                // a keydown only reaches an element that contains the focus — without
                // this the key does nothing until something inside is clicked.
                tabindex: "-1",
                onmounted: move |evt| async move {
                    let _ = evt.set_focus(true).await;
                },
                onkeydown: move |evt| {
                    let key = evt.key().to_string().to_ascii_lowercase();
                    if key == "escape" || key == "esc" {
                        on_close.call(());
                    }
                },
                onclick: move |evt| evt.stop_propagation(),

                header { class: "settings-dialog-head",
                    h2 { class: "settings-title", "Settings" }
                    button {
                        class: "text-button",
                        r#type: "button",
                        onclick: move |_| on_close.call(()),
                        "Close"
                    }
                }

                div { class: "settings-dialog-body",

                    section { class: "settings-card",
                        h2 { class: "settings-card-title", "Appearance" }

                        div { class: "settings-toggle-row",
                            div { class: "unit-toggle",
                                button {
                                    class: if snapshot.theme == Theme::Light { "unit-toggle-btn active" } else { "unit-toggle-btn" },
                                    r#type: "button",
                                    onclick: move |_| super::mutate_ctx(state, |s| s.set_theme(Theme::Light)),
                                    "Light"
                                }
                                button {
                                    class: if snapshot.theme == Theme::Dark { "unit-toggle-btn active" } else { "unit-toggle-btn" },
                                    r#type: "button",
                                    onclick: move |_| super::mutate_ctx(state, |s| s.set_theme(Theme::Dark)),
                                    "Dark"
                                }
                            }
                            p { class: "settings-toggle-detail",
                                "Applies immediately and is remembered between runs. k2g does not "
                                "follow the desktop's own light/dark setting: a job runs for hours, "
                                "and a window that repaints itself at dusk mid-cut is a surprise "
                                "rather than a convenience."
                            }
                        }
                    }

                    section { class: "settings-card",
                        h2 { class: "settings-card-title", "KiCad integration" }

                        p { class: "settings-card-intro",
                            "k2g reads boards over KiCad's IPC API, which KiCad ships switched off. "
                            "Registering k2g as a plugin adds a Create GCode button to the PCB editor's "
                            "toolbar; pressing it opens k2g with that board already loaded. Both actions "
                            "below change files belonging to KiCad, so both are shown in full before they "
                            "run and both can be undone."
                        }

                        if kicad_rows.is_empty() {
                            p { class: "settings-empty",
                                "No KiCad installation found for this user. KiCad creates its configuration "
                                "directory the first time it runs — start KiCad once, then come back."
                            }
                        } else {
                            for (install , status) in kicad_rows.into_iter() {
                                KicadInstallRow {
                                    key: "{install.version}",
                                    install,
                                    status,
                                    running: kicad_running,
                                    on_changed: move |outcome: Result<String, String>| {
                                        match outcome {
                                            Ok(message) => super::mutate_ctx(state, |s| s.log_event(message)),
                                            Err(message) => {
                                                super::mutate_ctx(
                                                    state,
                                                    |s| s.log_event(format!("KiCad integration failed: {message}")),
                                                )
                                            }
                                        }
                                        probe.set(probe() + 1);
                                    },
                                }
                            }
                        }
                    }

                    section { class: "settings-card",
                        h2 { class: "settings-card-title", "Updates" }

                        ToggleRow {
                            label: "Check for updates automatically",
                            detail: "Once a day, k2g asks GitHub whether a newer release exists. \
                                     This is the only network request k2g ever makes — with it off, \
                                     the application talks to nothing but the local KiCad socket. \
                                     Performing the check tells GitHub this machine's IP address. \
                                     Nothing is ever downloaded or installed without your confirmation, \
                                     and every download is signature-checked before it runs.",
                            checked: update_checks_on,
                            on_toggle: move |enabled| {
                                super::mutate_ctx(state, |s| s.set_update_check_enabled(enabled));
                            },
                        }

                        if update_checks_on {
                            dl { class: "settings-facts",
                                div { class: "settings-fact",
                                    dt { "Last checked" }
                                    dd { class: "mono",
                                        match last_check.clone() {
                                            Some(when) => when,
                                            None => "never".to_string(),
                                        }
                                    }
                                }
                                if let Some(version) = skipped.clone() {
                                    div { class: "settings-fact",
                                        dt { "Skipped version" }
                                        dd {
                                            span { class: "mono", "{version}" }
                                            button {
                                                class: "text-button",
                                                onclick: move |_| {
                                                    super::mutate_ctx(
                                                        state,
                                                        |s| {
                                                            s.clear_update_suppressions();
                                                        },
                                                    );
                                                },
                                                "Stop skipping"
                                            }
                                        }
                                    }
                                }
                                if let Some(until) = postponed.clone() {
                                    div { class: "settings-fact",
                                        dt { "Reminders paused until" }
                                        dd {
                                            span { class: "mono", "{until}" }
                                            button {
                                                class: "text-button",
                                                onclick: move |_| {
                                                    super::mutate_ctx(
                                                        state,
                                                        |s| {
                                                            s.clear_update_suppressions();
                                                        },
                                                    );
                                                },
                                                "Resume reminders"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    section { class: "settings-card",
                        h2 { class: "settings-card-title", "Security recording" }

                        ToggleRow {
                            label: "Record security-relevant events",
                            detail: "Appends a line to logs/security.jsonl when something security-relevant \
                                     happens: update checks and installs, changes to these switches, KiCad \
                                     plugin registration and API-setting edits, rejected configuration files, \
                                     G-code written to disk, and resets. Nothing is transmitted anywhere — \
                                     the file stays on this machine, and home-directory paths are shortened \
                                     to ~ so it contains no personal data. The diagnostic log on the Logs \
                                     screen is separate and is never written to disk either way.",
                            checked: security_log_on,
                            on_toggle: move |enabled| {
                                super::mutate_ctx(state, |s| { s.set_security_log_enabled(enabled); });
                            },
                        }

                        // Offered only once recording is off. Beside a live switch it would
                        // read as "clear the log", which it is not — the record's value is
                        // its continuity, and the one moment deleting it is clearly the
                        // user's intent is just after they asked to stop keeping one.
                        if !security_log_on {
                            div { class: "kicad-install-actions",
                                button {
                                    class: "text-button",
                                    onclick: move |_| {
                                        let message = match crate::runtime::security_log::erase() {
                                            Ok(()) => "Deleted the security log.".to_string(),
                                            Err(err) => format!("Could not delete the security log: {err}"),
                                        };
                                        super::mutate_ctx(state, |s| s.log_event(message));
                                    },
                                    "Delete the log kept so far"
                                }
                            }
                        }
                    }

                    section { class: "settings-card settings-card-danger",
                        h2 { class: "settings-card-title", "Data and reset" }

                        p { class: "settings-card-intro",
                            "Everything k2g stores lives in one directory: "
                            span { class: "mono", "{data_dir}" }
                            ". Nothing is kept anywhere else and nothing is stored online. The one "
                            "exception is a KiCad plugin registration, which lives in KiCad's own "
                            "folders — remove that from the KiCad integration card above."
                        }

                        div { class: "settings-danger-row",
                            div { class: "settings-danger-copy",
                                strong { "Reset settings to defaults" }
                                p { class: "settings-toggle-detail",
                                    "Deletes your settings, profiles, stock and job, and puts the "
                                    "shipped defaults back straight away. Your tool catalogs and the "
                                    "security log are kept."
                                }
                            }
                            button {
                                class: "text-button danger",
                                onclick: move |_| {
                                    spawn(async move {
                                        if !super::profiles_common::confirm(
                                            "Reset settings",
                                            "Reset all settings, profiles, stock and the job to their shipped defaults?\n\nYour tool catalogs and the security log are kept. The board stays loaded.",
                                        ).await {
                                            return;
                                        }
                                        // Two halves, and the second is not optional. The
                                        // reset clears the files and re-seeds the store;
                                        // without adopting it here the screens would carry
                                        // on rendering the profiles and stock that are gone,
                                        // which reads as the button having done nothing.
                                        match crate::runtime::data_lifecycle::factory_reset() {
                                            Ok(_) => super::mutate_ctx(state, |ctx| {
                                                ctx.adopt_reset_configuration();
                                                ctx.log_event(
                                                    "Settings, profiles, stock and the job reset to their shipped defaults.".to_string(),
                                                );
                                            }),
                                            Err(err) => super::mutate_ctx(state, |ctx| {
                                                ctx.log_event(format!("Reset failed: {err}"))
                                            }),
                                        }
                                        probe.set(probe() + 1);
                                    });
                                },
                                "Reset settings"
                            }
                        }

                        div { class: "settings-danger-row",
                            div { class: "settings-danger-copy",
                                strong { "Delete all k2g data" }
                                p { class: "settings-toggle-detail",
                                    "Removes the whole directory above — settings, profiles, catalogs and "
                                    "logs. k2g closes, and the next start behaves like a fresh install. "
                                    "This cannot be undone."
                                }
                            }
                            button {
                                class: "text-button danger",
                                onclick: move |_| {
                                    spawn(async move {
                                        if !super::profiles_common::confirm(
                                            "Delete all k2g data",
                                            "Delete every k2g setting, profile, catalog and log?\n\nThis cannot be undone. k2g will close.",
                                        ).await {
                                            return;
                                        }
                                        match crate::runtime::data_lifecycle::delete_all_data() {
                                            Ok(_) => {
                                                // Quit rather than carry on: the in-memory store is
                                                // still holding everything just deleted, and its
                                                // background flush would write a good deal of it
                                                // straight back out.
                                                dioxus::desktop::window().close();
                                            }
                                            Err(err) => {
                                                super::mutate_ctx(
                                                    state,
                                                    |s| s.log_event(format!("Deletion failed: {err}")),
                                                );
                                            }
                                        }
                                    });
                                },
                                "Delete all data"
                            }
                        }
                    }
                }
            }
        }
    }
}

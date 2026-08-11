//! The Logs screen: two records that look similar and are not the same thing.
//!
//! **Diagnostics** is a live tail of the application's `tracing`/`log` output, held
//! in memory by [`crate::runtime::log_capture`] and gone when the process exits. It
//! is for working out what the run in front of you is doing.
//!
//! **Security** is the persisted record from [`crate::runtime::security_log`] — a
//! small, deliberately chosen set of events that survives across runs, written to
//! `logs/security.jsonl`. It is for answering "what did this application do to my
//! machine, and when", which is what EU CRA Annex I (2)(l) asks a product to be able
//! to show. It can be exported, and it can be switched off in Settings.
//!
//! Both are read fresh on every render; the view re-renders when the shell `state`
//! changes and on the Refresh button, since there is no background UI timer.

use dioxus::prelude::*;

use crate::runtime::log_capture::{clear, snapshot, LogEntry};
use crate::runtime::security_log;

/// Which record is on screen.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LogTab {
    Diagnostics,
    Security,
}

/// Minimum severity shown. Each level also includes everything more severe.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LogFilter {
    All,
    Info,
    Warn,
    Error,
}

impl LogFilter {
    /// The least-severe rank this filter admits (ERROR=0 … TRACE=4).
    fn max_rank(self) -> u8 {
        match self {
            Self::All => 4,
            Self::Info => 2,
            Self::Warn => 1,
            Self::Error => 0,
        }
    }
}

/// Severity rank for an entry's level word (ERROR most severe = 0).
fn level_rank(level: &str) -> u8 {
    match level {
        "ERROR" => 0,
        "WARN" => 1,
        "INFO" => 2,
        "DEBUG" => 3,
        _ => 4, // TRACE / unknown
    }
}

/// The row CSS class carrying the level's colour.
fn level_class(level: &str) -> &'static str {
    match level {
        "ERROR" => "log-level log-error",
        "WARN" => "log-level log-warn",
        "INFO" => "log-level log-info",
        "DEBUG" => "log-level log-debug",
        _ => "log-level log-trace",
    }
}

/// One security record line, flattened for display.
struct SecurityRow {
    time: String,
    kind: String,
    outcome: String,
    detail: String,
}

/// Flatten a stored JSON line into something displayable.
///
/// The stored form is the interface — an exported file is meant to be read by other
/// tools — so this only reshapes for the screen and never for the file.
fn security_rows() -> Vec<SecurityRow> {
    security_log::read_all()
        .into_iter()
        .map(|entry| {
            let get = |key: &str| {
                entry
                    .get(key)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            };
            // Local time for the display; the file keeps UTC.
            let time = chrono::DateTime::parse_from_rfc3339(&get("time"))
                .map(|when| {
                    when.with_timezone(&chrono::Local)
                        .format("%Y-%m-%d %H:%M:%S")
                        .to_string()
                })
                .unwrap_or_else(|_| get("time"));

            let detail = match entry.get("detail") {
                // Render the detail object as `key=value` pairs rather than raw JSON:
                // the braces and quotes are most of the width and none of the meaning.
                Some(serde_json::Value::Object(map)) => map
                    .iter()
                    .filter(|(_, value)| !value.is_null())
                    .map(|(key, value)| match value {
                        serde_json::Value::String(text) => format!("{key}={text}"),
                        other => format!("{key}={other}"),
                    })
                    .collect::<Vec<_>>()
                    .join("  "),
                Some(other) => other.to_string(),
                None => String::new(),
            };

            SecurityRow {
                time,
                kind: get("kind"),
                outcome: get("outcome"),
                detail,
            }
        })
        .rev() // newest first, matching the diagnostics tail
        .collect()
}

#[component]
pub fn LogsScreen(state: Signal<crate::runtime::AppCtx>) -> Element {
    // Subscribe to shell state so the tail refreshes alongside app activity.
    let recording = state.read().security_log_enabled;

    let mut tab = use_signal(|| LogTab::Diagnostics);
    let mut filter = use_signal(|| LogFilter::All);
    let mut tick = use_signal(|| 0u32);
    let _ = tick.read(); // re-render when Refresh bumps the tick

    let active_tab = *tab.read();
    let active = *filter.read();

    let tab_button = |this: LogTab, label: &str| {
        let is_active = active_tab == this;
        rsx! {
            button {
                class: if is_active { "log-filter-btn active" } else { "log-filter-btn" },
                onclick: move |_| tab.set(this),
                "{label}"
            }
        }
    };

    let filter_button = |this: LogFilter, label: &str| {
        let is_active = active == this;
        rsx! {
            button {
                class: if is_active { "log-filter-btn active" } else { "log-filter-btn" },
                onclick: move |_| filter.set(this),
                "{label}"
            }
        }
    };

    // Only the tab on screen is materialised — reading the security record touches
    // the disk, and doing that to render a hidden tab is pure waste.
    let (visible, security, total) = match active_tab {
        LogTab::Diagnostics => {
            let entries: Vec<LogEntry> = snapshot();
            let total = entries.len();
            let visible: Vec<LogEntry> = entries
                .into_iter()
                .rev()
                .filter(|entry| level_rank(entry.level) <= active.max_rank())
                .collect();
            (visible, Vec::new(), total)
        }
        LogTab::Security => {
            let rows = security_rows();
            let total = rows.len();
            (Vec::new(), rows, total)
        }
    };
    let shown = match active_tab {
        LogTab::Diagnostics => visible.len(),
        LogTab::Security => security.len(),
    };

    rsx! {
        div { class: "screen single logs-screen",
            header { class: "logs-toolbar",
                div { class: "logs-title-group",
                    h1 { class: "logs-title", "Logs" }
                    span { class: "logs-count", "{shown} / {total}" }
                }
                div { class: "logs-controls",
                    div { class: "log-filter-group",
                        {tab_button(LogTab::Diagnostics, "Diagnostics")}
                        {tab_button(LogTab::Security, "Security")}
                    }
                    if active_tab == LogTab::Diagnostics {
                        div { class: "log-filter-group",
                            {filter_button(LogFilter::All, "All")}
                            {filter_button(LogFilter::Info, "Info")}
                            {filter_button(LogFilter::Warn, "Warnings")}
                            {filter_button(LogFilter::Error, "Errors")}
                        }
                    }
                    button {
                        class: "text-button",
                        onclick: move |_| { tick.set(tick() + 1); },
                        "Refresh"
                    }
                    if active_tab == LogTab::Diagnostics {
                        button {
                            class: "text-button",
                            onclick: move |_| {
                                clear();
                                tick.set(tick() + 1);
                            },
                            "Clear"
                        }
                    } else {
                        // Export, not Clear. The security record is meant to be kept and
                        // handed to someone; a one-click wipe next to the thing whose
                        // value is its continuity would be the wrong affordance. It is
                        // erasable, but from Settings, alongside the opt-out.
                        button {
                            class: "text-button",
                            onclick: move |_| {
                                let outcome = export_security_log();
                                super::mutate_ctx(
                                    state,
                                    |s| match outcome {
                                        Some(Ok(path)) => s.log_event(format!("Security log exported to {path}")),
                                        Some(Err(message)) => s.log_event(format!("Export failed: {message}")),
                                        None => {}
                                    },
                                );
                            },
                            "Export…"
                        }
                    }
                }
            }

            if active_tab == LogTab::Security && !recording {
                div { class: "logs-empty",
                    "Security recording is switched off in Settings. Whatever was recorded "
                    "before it was switched off is still shown below."
                }
            }

            match active_tab {
                LogTab::Diagnostics => rsx! {
                    if visible.is_empty() {
                        div { class: "logs-empty",
                            if total == 0 {
                                "No log output captured yet."
                            } else {
                                "No entries match this filter."
                            }
                        }
                    } else {
                        div { class: "logs-list",
                            for (idx , entry) in visible.iter().enumerate() {
                                div { key: "{idx}-{entry.timestamp}", class: "log-row",
                                    span { class: "log-time mono", "{entry.timestamp}" }
                                    span { class: level_class(entry.level), "{entry.level}" }
                                    span { class: "log-target mono", "{entry.target}" }
                                    span { class: "log-message", "{entry.message}" }
                                }
                            }
                        }
                    }
                },
                LogTab::Security => rsx! {
                    if security.is_empty() {
                        div { class: "logs-empty", "Nothing recorded yet." }
                    } else {
                        div { class: "logs-list",
                            for (idx , row) in security.iter().enumerate() {
                                div { key: "{idx}-{row.time}", class: "log-row",
                                    span { class: "log-time mono", "{row.time}" }
                                    span {
                                        class: if row.outcome == "failed" {
                                            "log-level log-error"
                                        } else {
                                            "log-level log-info"
                                        },
                                        "{row.outcome}"
                                    }
                                    span { class: "log-target mono", "{row.kind}" }
                                    span { class: "log-message mono", "{row.detail}" }
                                }
                            }
                        }
                    }
                },
            }
        }
    }
}

/// Write the security record out as JSON Lines, wherever the user picks.
///
/// Exported verbatim — the same lines that are on disk, not the flattened display
/// form. The point of the export is to hand the record to someone else's tooling,
/// and re-rendering it for the screen on the way out would defeat that.
///
/// `None` when the user cancelled the dialog.
fn export_security_log() -> Option<Result<String, String>> {
    let default_name = format!(
        "k2g-security-{}.jsonl",
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    );
    let target = rfd::FileDialog::new()
        .set_file_name(&default_name)
        .add_filter("JSON Lines", &["jsonl"])
        .save_file()?;

    let body: String = security_log::read_all()
        .iter()
        .map(|entry| format!("{entry}\n"))
        .collect();

    Some(
        std::fs::write(&target, body)
            .map(|()| target.display().to_string())
            .map_err(|err| err.to_string()),
    )
}

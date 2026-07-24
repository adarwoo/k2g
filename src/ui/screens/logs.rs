//! The Logs screen: a live tail of the application's `tracing`/`log` output,
//! captured in-memory by [`crate::runtime::log_capture`].
//!
//! Entries are shown newest-first (so the latest line is visible without
//! scrolling), filterable by minimum severity, with clear/refresh controls. The
//! view re-reads the ring buffer on every render; it re-renders when the shell
//! `state` changes (most log lines accompany a state change) and on the manual
//! Refresh button, since there is no background UI timer.

use dioxus::prelude::*;

use crate::runtime::log_capture::{clear, snapshot, LogEntry};

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

#[component]
pub fn LogsScreen(state: Signal<crate::runtime::AppCtx>) -> Element {
    // Subscribe to shell state so the tail refreshes alongside app activity.
    let _ = state.read();

    let mut filter = use_signal(|| LogFilter::All);
    let mut tick = use_signal(|| 0u32);
    let _ = tick.read(); // re-render when Refresh bumps the tick

    let active = *filter.read();
    let entries: Vec<LogEntry> = snapshot();
    let total = entries.len();
    let visible: Vec<LogEntry> = entries
        .into_iter()
        .rev() // newest first
        .filter(|entry| level_rank(entry.level) <= active.max_rank())
        .collect();
    let shown = visible.len();

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

    rsx! {
        div { class: "screen single logs-screen",
            header { class: "logs-toolbar",
                div { class: "logs-title-group",
                    h1 { class: "logs-title", "Logs" }
                    span { class: "logs-count", "{shown} / {total}" }
                }
                div { class: "logs-controls",
                    div { class: "log-filter-group",
                        {filter_button(LogFilter::All, "All")}
                        {filter_button(LogFilter::Info, "Info")}
                        {filter_button(LogFilter::Warn, "Warnings")}
                        {filter_button(LogFilter::Error, "Errors")}
                    }
                    button {
                        class: "text-button",
                        onclick: move |_| { tick.set(tick() + 1); },
                        "Refresh"
                    }
                    button {
                        class: "text-button",
                        onclick: move |_| {
                            clear();
                            tick.set(tick() + 1);
                        },
                        "Clear"
                    }
                }
            }

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
        }
    }
}

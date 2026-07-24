//! Job "Code" view — the generated G-code program, syntax-highlighted, with a
//! line-number gutter and a program-statistics strip.
//!
//! The program is read-only here: each generation replaces `gcode` wholesale (no
//! history), and this view exists to read/verify that output. Highlighting is
//! done in Rust by [`super::gcode_highlight`]; colours come from theme CSS
//! variables so light/dark both work.

use dioxus::prelude::*;
use units::Length;

use super::gcode_highlight::highlight_program;
use crate::runtime::{AppCtx, STATUS_KEY_GENERATION_NOGO_REASONS};
use crate::ui::navigation::GenerationState;
use units::user_format as unit_format;

/// The G-code program view: a highlighted, scrollable listing plus a stat strip.
#[component]
pub fn CodeView(state: Signal<AppCtx>) -> Element {
    let snapshot = state.read().clone();
    let board_thickness_pcb_label = snapshot
        .board
        .as_ref()
        .and_then(|board| board.thickness.as_ref())
        .map(|thickness| {
            unit_format::format_length_display(
                Length::from_mm(thickness.as_mm()),
                snapshot.unit_system,
            )
        });

    let is_empty = snapshot.gcode.trim().is_empty();
    let highlighted = highlight_program(&snapshot.gcode);
    let line_count = snapshot.gcode.lines().count();
    let char_count = snapshot.gcode.len();

    // When there is no program, explain *why*: the readiness gate's no-go reasons
    // (why generation hasn't run) rather than a generic message. These are kept
    // current by the orchestration layer (launch + every mutation).
    let nogo_reasons: Vec<String> = snapshot
        .status
        .get(STATUS_KEY_GENERATION_NOGO_REASONS)
        .map(|raw| {
            raw.split(" | ")
                .filter(|reason| !reason.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    rsx! {
        div { class: "screen single",
            if is_empty {
                div { class: "gcode-empty",
                    match snapshot.generation_state {
                        GenerationState::Running => rsx! {
                            div { class: "gcode-empty-title", "Generating…" }
                        },
                        GenerationState::Failed => rsx! {
                            div { class: "gcode-empty-block",
                                div { class: "gcode-empty-title", "Generation failed" }
                                div { "See the Logs screen for the error." }
                            }
                        },
                        GenerationState::Idle if !nogo_reasons.is_empty() => rsx! {
                            div { class: "gcode-empty-block",
                                div { class: "gcode-empty-title", "No program yet — the job isn't ready:" }
                                ul { class: "gcode-empty-reasons",
                                    for reason in nogo_reasons.iter() {
                                        li { key: "{reason}", "{reason}" }
                                    }
                                }
                            }
                        },
                        GenerationState::Idle => rsx! {
                            div { class: "gcode-empty-title", "No program generated yet." }
                        },
                    }
                }
            } else {
                div { class: "gcode-view",
                    for (idx , spans) in highlighted.iter().enumerate() {
                        div { key: "{idx}", class: "gcode-line",
                            span { class: "gcode-lineno", "{idx + 1}" }
                            code { class: "gcode-line-content",
                                for (sidx , span) in spans.iter().enumerate() {
                                    if span.class.is_empty() {
                                        span { key: "{sidx}", "{span.text}" }
                                    } else {
                                        span { key: "{sidx}", class: span.class, "{span.text}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            div { class: "program-stats",
                span { "Lines: {line_count}" }
                span { "Characters: {char_count}" }
                span {
                    if let Some(v) = board_thickness_pcb_label.as_ref() {
                        "Board thickness (PCB): {v}"
                    } else {
                        "Board thickness (PCB): unavailable"
                    }
                }
            }
        }
    }
}

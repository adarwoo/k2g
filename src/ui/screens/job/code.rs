//! Job "Code" view — the generated G-code program, syntax-highlighted, with a
//! line-number gutter and a program-statistics strip.
//!
//! The program is read-only here: each generation replaces `gcode` wholesale (no
//! history), and this view exists to read/verify that output. Highlighting is
//! done in Rust by [`super::gcode_highlight`]; colours come from theme CSS
//! variables so light/dark both work.

use std::sync::Arc;

use dioxus::prelude::*;
use units::Length;

use super::gcode_highlight::{highlight_program, Span};
use crate::runtime::{AppCtx, STATUS_KEY_GENERATION_NOGO_REASONS};
use crate::ui::navigation::GenerationState;
use units::user_format as unit_format;

/// Height of one listing row, in CSS pixels.
///
/// Pinned here **and** in `.gcode-line`, and the two must agree exactly: the listing is
/// virtualised, so this number is what converts a scroll offset into a line index. A row
/// a pixel taller than this and the gutter drifts away from the code by a line every
/// few hundred rows.
const LINE_HEIGHT_PX: f64 = 18.0;

/// Rows rendered above and below the viewport.
///
/// Enough that a flick of the wheel lands on rows that already exist. Scrolling faster
/// than this reveals blank space for one frame, which is the usual bargain.
const OVERSCAN_ROWS: usize = 16;

/// Viewport height assumed before the first scroll event reports the real one.
///
/// Deliberately generous: too small leaves the bottom of a tall panel empty until the
/// first scroll, and rendering a few hundred rows that turn out not to be needed costs
/// nothing next to the 400,000 nodes this exists to avoid.
const ASSUMED_VIEWPORT_PX: f64 = 1600.0;

/// Which rows the listing must actually render, given where it is scrolled to.
///
/// Pure, and separated out because getting it wrong is not a crash but a drift: the
/// gutter and the code slide apart by a line, and only after a few hundred rows, which is
/// exactly the kind of fault that reaches an operator rather than a test.
///
/// `scroll_top` is clamped at zero because a rubber-band overscroll reports a negative
/// offset on some platforms, and the end is clamped to the program because the assumed
/// viewport is deliberately larger than most panels.
fn visible_rows(scroll_top: f64, viewport_px: f64, total_rows: usize) -> std::ops::Range<usize> {
    let first = (scroll_top.max(0.0) / LINE_HEIGHT_PX).floor() as usize;
    let first = first.saturating_sub(OVERSCAN_ROWS).min(total_rows);
    let span = (viewport_px.max(0.0) / LINE_HEIGHT_PX).ceil() as usize + 2 * OVERSCAN_ROWS;
    first..(first + span).min(total_rows)
}

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

    // One step's program, not the job's — a step names its own CNC, so the steps of one
    // job are separate programs rather than sections of one.
    let selected = snapshot.selected_program();
    let program = selected.and_then(|step| step.program());
    let step_failure = selected.and_then(|step| step.failure()).map(str::to_string);
    let text = program.map(|p| p.text.as_str()).unwrap_or_default();

    let is_empty = text.trim().is_empty();

    // Highlighting is the one expensive thing here — 178ms on a 44,000-line program — and
    // it must not happen again on every scroll frame. Cached against the context revision
    // and the step, which together decide the program: the pair changes when the program
    // does and at no other time, so the pass runs once per new program.
    //
    // Whole-program rather than per-window, even though only a window is drawn. A
    // parenthesised comment is a *bounded* rule and may in principle run past a newline,
    // so highlighting a window in isolation could paint it differently depending on where
    // the reader had scrolled to.
    let mut cache = use_signal(|| (u64::MAX, 0usize, Arc::new(Vec::<Vec<Span>>::new())));
    let key = (snapshot.revision, snapshot.selected_step);
    if (cache.read().0, cache.read().1) != key {
        cache.set((key.0, key.1, Arc::new(highlight_program(text))));
    }
    let highlighted = cache.read().2.clone();

    // Where the listing is scrolled to, and how much of it is on screen. Both come from
    // the scroll event; until one arrives the viewport is assumed generous.
    let mut scroll_top = use_signal(|| 0.0_f64);
    let mut viewport_px = use_signal(|| ASSUMED_VIEWPORT_PX);

    let total_rows = highlighted.len();
    let window = visible_rows(*scroll_top.read(), *viewport_px.read(), total_rows);
    let (first_row, last_row) = (window.start, window.end);
    let listing_px = total_rows as f64 * LINE_HEIGHT_PX;
    let offset_px = first_row as f64 * LINE_HEIGHT_PX;

    let line_count = text.lines().count();
    let char_count = text.len();
    // Named only when there is more than one step: with a single step the CNC is the
    // job's CNC and stating it here would be noise.
    let cnc_name = selected
        .filter(|_| snapshot.programs.len() > 1)
        .map(|step| step.cnc_name.clone())
        .filter(|name| !name.is_empty());

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
            // No Save button here: it lives in the top bar, beside the readiness pill, so
            // the program can be saved from whichever screen the user is on rather than
            // only from this tab.
            if is_empty {
                div { class: "gcode-empty",
                    match snapshot.generation_state {
                        GenerationState::Running => rsx! {
                            div { class: "gcode-empty-title", "Generating…" }
                        },
                        // The step's own reason, not a pointer to the Logs screen: a step
                        // fails alone now, so the failure belongs beside the step it
                        // belongs to rather than as a whole-job diagnostic elsewhere.
                        _ if step_failure.is_some() => rsx! {
                            div { class: "gcode-empty-block",
                                div { class: "gcode-empty-title", "This step produced no program" }
                                div { "{step_failure.clone().unwrap_or_default()}" }
                            }
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
                // Only the rows on screen are in the DOM. A 44,000-line program is around
                // 434,000 nodes rendered whole — ten per line, and five seconds before the
                // view appears — where the reader can see eighty of them. The spacer below
                // carries the full height so the scrollbar is honest about the program's
                // length, and the window is offset into place.
                div {
                    class: "gcode-view",
                    onscroll: move |event| {
                        scroll_top.set(event.data().scroll_top());
                        let height = event.data().client_height() as f64;
                        if height > 0.0 {
                            viewport_px.set(height);
                        }
                    },
                    div { class: "gcode-listing", style: "height: {listing_px}px;",
                        div {
                            class: "gcode-window",
                            style: "transform: translateY({offset_px}px);",
                            for (row , spans) in highlighted[first_row..last_row].iter().enumerate() {
                                {
                                    // Keyed by line number, not by position in the window:
                                    // the window slides, and a key that slid with it would
                                    // make every row look changed on every scroll.
                                    let line_no = first_row + row + 1;
                                    rsx! {
                                        div { key: "{line_no}", class: "gcode-line",
                                            span { class: "gcode-lineno", "{line_no}" }
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
                if let Some(name) = cnc_name.as_ref() {
                    span { "CNC: {name}" }
                }
            }
        }
    }
}

#[cfg(test)]
mod window_tests {
    use super::*;

    /// A program shorter than the viewport is drawn whole — there is nothing to virtualise
    /// and no scrollbar to be honest about.
    #[test]
    fn a_short_program_is_all_on_screen() {
        assert_eq!(visible_rows(0.0, 600.0, 40), 0..40);
        assert_eq!(visible_rows(0.0, 600.0, 0), 0..0);
    }

    /// At rest the window starts at the top, however much overscan is asked for: there are
    /// no rows above row one to render.
    #[test]
    fn the_top_of_the_listing_starts_at_the_first_line() {
        let rows = visible_rows(0.0, 360.0, 10_000);
        assert_eq!(rows.start, 0);
        assert_eq!(rows.end, 20 + 2 * OVERSCAN_ROWS, "a 360px viewport is 20 rows");
    }

    /// The whole point: a 44,000-line program renders a window, not a program.
    #[test]
    fn a_long_program_renders_only_a_window() {
        let rows = visible_rows(180_000.0, 720.0, 44_000);
        assert_eq!(rows.start, 10_000 - OVERSCAN_ROWS, "180,000px is 10,000 rows down");
        assert!(rows.len() < 100, "rendered {} rows of 44,000", rows.len());
    }

    /// The end of the listing must not ask for rows past the end of the program, and an
    /// overscroll must not either.
    #[test]
    fn the_window_never_runs_past_the_program() {
        let rows = visible_rows(44_000.0 * LINE_HEIGHT_PX, 720.0, 44_000);
        assert_eq!(rows.end, 44_000);
        assert!(rows.start <= rows.end);

        let past = visible_rows(1e9, 720.0, 44_000);
        assert_eq!(past, 44_000..44_000, "scrolled past the end, nothing to draw");
    }

    /// Some platforms report a negative offset while the scroll rubber-bands at the top.
    /// Cast to `usize` unguarded that is a very large number, and the listing would jump
    /// to its end for the duration of the bounce.
    #[test]
    fn a_rubber_band_overscroll_does_not_wrap_around() {
        assert_eq!(visible_rows(-240.0, 600.0, 10_000).start, 0);
    }
}

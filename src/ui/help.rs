//! In-context help: embedded Markdown reference docs shown in a modal overlay.
//!
//! Help pages are authored as Markdown under `assets/help/` and compiled into
//! the binary with `include_str!`, so they ship as read-only reference (they are
//! not copied to the user's data directory like editable assets). A [`HelpDoc`]
//! pairs a page with the label of the button that opens it; drop a
//! [`HelpButton`] next to any feature to expose its page.
//!
//! Markdown is rendered to HTML with `pulldown-cmark` and injected via
//! `dangerous_inner_html`. This is safe here because the source is entirely
//! build-time content — never user input.

use dioxus::prelude::*;
use pulldown_cmark::{html, Options, Parser};

/// A single embedded help page and the trigger-button label that opens it.
///
/// `markdown` is the raw page source (embedded at build time); `title` is the
/// modal heading; `button_label` is the short text on the button that opens it.
#[derive(Clone, Copy, PartialEq)]
pub struct HelpDoc {
    pub button_label: &'static str,
    pub title: &'static str,
    pub markdown: &'static str,
}

/// GCode Template Language reference, shown from the CNC primitive editor.
pub const GTL: HelpDoc = HelpDoc {
    button_label: "Template syntax",
    title: "GCode template syntax",
    markdown: include_str!("../../assets/help/gtl.md"),
};

/// Renders Markdown page source to an HTML fragment.
///
/// Tables and strikethrough are enabled because the help pages use them; all
/// other CommonMark defaults apply. Output is styled by the `.help-markdown`
/// rules in the theme.
fn render_markdown(markdown: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);

    let parser = Parser::new_ext(markdown, options);
    let mut out = String::with_capacity(markdown.len() * 2);
    html::push_html(&mut out, parser);
    out
}

/// A help trigger: a small button that opens [`doc`] in a modal overlay.
///
/// Self-contained — it owns the open/closed state, so a caller just places
/// `HelpButton { doc: help::GTL }` wherever the affordance belongs.
#[component]
pub fn HelpButton(doc: HelpDoc) -> Element {
    let mut open = use_signal(|| false);

    rsx! {
        button {
            class: "btn btn-secondary btn-small help-trigger",
            title: "{doc.title}",
            onclick: move |_| open.set(true),
            // Information glyph + label.
            "\u{2139}\u{fe0e} {doc.button_label}"
        }
        if *open.read() {
            HelpOverlay { doc, on_close: move |_| open.set(false) }
        }
    }
}

/// The modal overlay that displays a rendered help page. Clicking the backdrop
/// or the close button dismisses it; clicks inside the panel are swallowed so
/// they do not reach the backdrop.
#[component]
fn HelpOverlay(doc: HelpDoc, on_close: EventHandler<()>) -> Element {
    let rendered = render_markdown(doc.markdown);

    rsx! {
        div {
            class: "help-overlay",
            onclick: move |_| on_close.call(()),
            div {
                class: "help-panel",
                onclick: move |evt| evt.stop_propagation(),
                div { class: "help-panel-head",
                    h2 { "{doc.title}" }
                    button {
                        class: "btn btn-secondary btn-small",
                        onclick: move |_| on_close.call(()),
                        "Close"
                    }
                }
                div {
                    class: "help-markdown",
                    dangerous_inner_html: "{rendered}",
                }
            }
        }
    }
}

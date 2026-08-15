//! The Manual screen: the user manual, read inside the application.
//!
//! The same document that ships as `docs/user-manual.md` and is read on GitHub — embedded
//! at build time, so it is available to an operator standing at a machine with no browser
//! and no network. Rendering (and the reason nothing in it is clickable) lives in
//! [`crate::ui::help`].
//!
//! The page is laid out as a document with a sticky contents rail: the manual is long
//! enough that scrolling to a section is the main navigation, and its own Markdown table
//! of contents is dropped in favour of this one, which stays on screen.

use dioxus::prelude::*;

use crate::ui::help;

/// The documents that sit beside the manual, as `(label, path in the repository)`.
///
/// The manual links to each of these in prose, and those links are flattened by the
/// renderer — an `href` in the WebView navigates the application window itself (see
/// [`crate::ui::help`]). So they come back here as real buttons that hand the URL to the
/// system browser, which is the same bargain the About screen makes.
///
/// Paths rather than URLs: the repository is `CARGO_PKG_REPOSITORY`, so a fork or a move
/// carries them with it.
const COMPANIONS: &[(&str, &str)] = &[
    ("Install & security", "docs/install-and-security.md"),
    ("Privacy", "PRIVACY.md"),
    ("GCode template language", "schemas/docs/gcode-template-language.md"),
];

/// Where a companion document is published.
///
/// `main` rather than the running version's tag: a released build stays installed for
/// months, and the branch is the copy that is still being corrected.
fn companion_url(path: &str) -> String {
    format!("{}/blob/main/{path}", env!("CARGO_PKG_REPOSITORY"))
}

/// Scrolls the section with `id` into view.
///
/// Done in the page rather than with an anchor link for the reason the renderer strips
/// links at all: `<a href="#…">` is a navigation, and a navigation in this window is one
/// mistake away from replacing the application. Interpolating `id` into the script is
/// sound because these ids are the renderer's own slugs — lower-case letters, digits and
/// hyphens, and nothing that can close a quote (`help::slug`).
fn scroll_to_section(id: &str) {
    let script = format!(
        "document.getElementById('{id}')?.scrollIntoView({{ behavior: 'smooth', block: 'start' }});"
    );
    spawn(async move {
        if let Err(err) = document::eval(&script).await {
            log::debug!("could not scroll to a manual section: {err}");
        }
    });
}

#[component]
pub fn ManualScreen(state: Signal<crate::runtime::AppCtx>) -> Element {
    // Present but unused: the manual is static. Touched so the prop is not flagged, the
    // same way About does it.
    let _ = state;

    // Parsed and rendered once for as long as the screen is mounted. The manual is some
    // seven hundred lines of Markdown, and the shell re-renders on every context change —
    // a toast, a generation finishing — none of which changes a word of it.
    let doc = use_memo(|| help::render_doc(help::MANUAL.markdown));
    let rendered = doc.read();

    rsx! {
        div { class: "screen single manual-screen",
            aside { class: "manual-toc",
                nav { class: "manual-toc-nav",
                    div { class: "manual-toc-title", "Contents" }
                    for section in rendered.sections.iter() {
                        button {
                            key: "{section.id}",
                            class: "manual-toc-link",
                            r#type: "button",
                            onclick: {
                                let id = section.id.clone();
                                move |_| scroll_to_section(&id)
                            },
                            "{section.title}"
                        }
                    }
                }

                div { class: "manual-toc-companions",
                    div { class: "manual-toc-title", "More documentation" }
                    for (label , path) in COMPANIONS.iter() {
                        button {
                            key: "{path}",
                            class: "manual-toc-link is-external",
                            r#type: "button",
                            title: "Opens {companion_url(path)} in your browser",
                            onclick: move |_| {
                                let url = companion_url(path);
                                if let Err(err) = open::that_detached(&url) {
                                    log::warn!("Could not open {url} in a browser: {err}");
                                }
                            },
                            "{label}"
                        }
                    }
                }
            }

            article { class: "manual-doc help-markdown",
                dangerous_inner_html: "{rendered.html}",
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every companion is a real file in the repository, so no button opens a 404.
    ///
    /// Checked by asking the compiler for the file rather than by walking the filesystem
    /// at test time: `include_str!` fails the build if a path is wrong, which is a better
    /// moment to find out than a test run.
    #[test]
    fn every_companion_document_exists() {
        const FILES: &[&str] = &[
            include_str!("../../../docs/install-and-security.md"),
            include_str!("../../../PRIVACY.md"),
            include_str!("../../../schemas/docs/gcode-template-language.md"),
        ];
        assert_eq!(
            FILES.len(),
            COMPANIONS.len(),
            "a companion was added to the sidebar without being proved to exist here"
        );
        assert!(FILES.iter().all(|text| !text.trim().is_empty()));
    }

    /// The URL is built from the crate's own repository field, so a fork's manual points
    /// at the fork.
    #[test]
    fn a_companion_url_points_at_the_repository() {
        assert_eq!(
            companion_url("PRIVACY.md"),
            "https://github.com/adarwoo/k2g/blob/main/PRIVACY.md"
        );
    }
}

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
//!
//! # Searching
//!
//! Sixteen sections is more than a reader wants to scroll when they arrived with a word
//! in mind, so the rail opens with a search field. Typing marks every occurrence in the
//! document and narrows the rail to the sections that contain one, each with its count;
//! Enter walks the matches, shift+Enter walks them back, Escape clears. When nothing
//! matches, the rail returns to the full contents rather than emptying — a search that
//! finds nothing should not also take away the way of looking manually.
//!
//! The matching itself is [`help::render_doc_matching`]'s, not this module's, and for a
//! reason worth keeping in view: the query is compared against text while the page is
//! built, so it never reaches `document::eval`. What this screen sends to the WebView is
//! only ever an id it generated — a heading slug, or a match number.

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
    ("GCode template language", "docs/design/gcode-template-language.md"),
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

/// Brings match number `index` into view and makes it the current one.
///
/// The same bargain as [`scroll_to_section`], and safer still: `index` is a `usize` this
/// module counted, never anything the user typed. The query itself does not appear in the
/// script at all — the matches were already marked while the page was rendered
/// ([`help::render_doc_matching`]), so all that is left to do here is point at one.
///
/// The current mark is distinguished by a class rather than by re-rendering the document,
/// which would rebuild forty kilobytes of HTML to recolour one word.
fn focus_hit(index: usize) {
    let script = format!(
        "document.querySelectorAll('.manual-hit.is-current')\
           .forEach(hit => hit.classList.remove('is-current'));\
         const hit = document.getElementById('manual-hit-{index}');\
         if (hit) {{ hit.classList.add('is-current');\
           hit.scrollIntoView({{ behavior: 'smooth', block: 'center' }}); }}"
    );
    spawn(async move {
        if let Err(err) = document::eval(&script).await {
            log::debug!("could not scroll to a manual match: {err}");
        }
    });
}

/// A count of matches with its noun agreeing with it — "1 match", "12 matches".
///
/// Used by both the counter above the rail and each result's tooltip, so the two cannot
/// come to disagree about how a number is written.
fn match_count(n: usize) -> String {
    if n == 1 {
        "1 match".to_string()
    } else {
        format!("{n} matches")
    }
}

#[component]
pub fn ManualScreen(state: Signal<crate::runtime::AppCtx>) -> Element {
    // Present but unused: the manual is static. Touched so the prop is not flagged, the
    // same way About does it.
    let _ = state;

    let mut query = use_signal(String::new);
    // Which match the ◀ ▶ buttons are sitting on. Reset whenever the query changes, since
    // "match 7 of 12" means nothing once the twelve are a different twelve.
    let mut at_hit = use_signal(|| 0usize);
    // Whether the reader has stepped into the matches yet. Typing highlights them all but
    // singles out none, so until the first step there is no "current" match to be on —
    // and a counter reading "1 of 103" beside a page with no ring drawn on it would be
    // describing a state the document is not in.
    let mut stepped = use_signal(|| false);

    // Re-rendered only when the query changes: `use_memo` tracks the signals read inside
    // it, and the shell re-renders on every context change — a toast, a generation
    // finishing — none of which changes a word of the manual. Searching costs a parse of
    // some seven hundred lines of Markdown, which is a few milliseconds on a keystroke and
    // buys a search that never puts what the user typed into a script; see
    // `help::render_doc_matching`.
    let doc = use_memo(move || help::render_doc_matching(help::MANUAL.markdown, &query.read()));
    let rendered = doc.read();

    let total = rendered.total_hits;
    // A query too short to search reads as "not searching" rather than "no matches", or
    // the first keystroke of every search would flash "Nothing found". The threshold is
    // the renderer's, not a second opinion about it.
    let searched = query.read().trim().chars().count() >= help::MIN_QUERY;

    // Built here rather than in the markup so the three states are visibly the three
    // states, and so the singular case reads as English rather than "1 matches".
    let counter = if total == 0 {
        "Nothing found".to_string()
    } else if *stepped.read() {
        // One-based: `at_hit` counts matches from zero, and a reader counts from one.
        format!("{} of {total}", *at_hit.read() + 1)
    } else {
        match_count(total)
    };

    // Steps the current match, wrapping at both ends the way a find field does — the last
    // match is next to the first, and there is no dead end to back out of.
    let mut step = move |forward: bool| {
        let total = doc.read().total_hits;
        if total == 0 {
            return;
        }
        let next = if *stepped.read() {
            let now = *at_hit.read();
            if forward {
                (now + 1) % total
            } else {
                (now + total - 1) % total
            }
        } else {
            // The first step moves *into* the matches rather than off an index nothing is
            // drawn on yet: forwards lands on the first, backwards on the last.
            stepped.set(true);
            if forward {
                0
            } else {
                total - 1
            }
        };
        at_hit.set(next);
        focus_hit(next);
    };

    rsx! {
        div { class: "screen single manual-screen",
            aside { class: "manual-toc",
                div { class: "manual-search",
                    input {
                        class: "manual-search-input",
                        r#type: "search",
                        value: "{query}",
                        placeholder: "Search the manual",
                        oninput: move |evt| {
                            query.set(evt.value());
                            at_hit.set(0);
                            stepped.set(false);
                        },
                        // Enter walks the matches, shift+Enter walks them backwards, and
                        // Escape clears — the three keys a find field is expected to have.
                        onkeydown: move |evt| {
                            match evt.key() {
                                Key::Enter => {
                                    evt.prevent_default();
                                    step(!evt.modifiers().shift());
                                }
                                Key::Escape => {
                                    evt.prevent_default();
                                    query.set(String::new());
                                    at_hit.set(0);
                                    stepped.set(false);
                                }
                                _ => {}
                            }
                        },
                    }

                    if searched {
                        div { class: "manual-search-status",
                            if total == 0 {
                                span { class: "manual-search-count is-empty", "{counter}" }
                            } else {
                                span { class: "manual-search-count", "{counter}" }
                                div { class: "manual-search-steps",
                                    button {
                                        class: "manual-search-step",
                                        r#type: "button",
                                        title: "Previous match (shift+Enter)",
                                        onclick: move |_| step(false),
                                        "\u{25c0}"
                                    }
                                    button {
                                        class: "manual-search-step",
                                        r#type: "button",
                                        title: "Next match (Enter)",
                                        onclick: move |_| step(true),
                                        "\u{25b6}"
                                    }
                                }
                            }
                        }
                    }
                }

                nav { class: "manual-toc-nav",
                    // The rail lists the sections a search matched while one is running,
                    // and the whole contents when none is: the same list, narrowed. A
                    // second list would leave the reader deciding which of two to use.
                    if searched && total > 0 {
                        div { class: "manual-toc-title", "Matching sections" }
                        for hit in rendered.hits.iter() {
                            button {
                                key: "{hit.id}",
                                class: "manual-toc-link is-hit",
                                r#type: "button",
                                title: "{match_count(hit.hits)} in this section",
                                onclick: {
                                    let id = hit.id.clone();
                                    move |_| scroll_to_section(&id)
                                },
                                span { class: "manual-toc-link-text", "{hit.title}" }
                                span { class: "manual-toc-hit-count", "{hit.hits}" }
                            }
                        }
                    } else {
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
            include_str!("../../../docs/design/gcode-template-language.md"),
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

    /// Every relative link in the documentation points at a file that exists.
    ///
    /// The one real argument against moving a document is the links that break silently
    /// when it moves — a `README` link to a renamed file is a 404 nobody sees until a
    /// reader hits it, and nothing here checked one. The design documents moving out of
    /// `schemas/` is exactly that risk, so it comes with this.
    ///
    /// Reads the repository at test time rather than embedding it: the point is the set
    /// of files as they are on disk, which is what `include_str!` cannot ask about.
    #[test]
    fn every_relative_documentation_link_resolves() {
        use std::path::{Path, PathBuf};

        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        // Every Markdown file at the root, not just the README: CONTRIBUTING and the
        // CHANGELOG link into `docs/`, `assets/` and each other, and a root document added
        // later should be covered without anyone remembering to add it here.
        let mut pages: Vec<PathBuf> = std::fs::read_dir(&root)
            .expect("the repository root is readable")
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_file() && path.extension().is_some_and(|e| e == "md"))
            .collect();
        let mut stack = vec![root.join("docs")];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("docs/ is readable").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "md") {
                    pages.push(path);
                }
            }
        }
        assert!(pages.len() > 5, "the documentation set should not be empty");

        // `[text](target)`, without a regex crate: the closing paren of a markdown link
        // is the first one, since a target containing parens would have to be angled.
        let mut broken: Vec<String> = Vec::new();
        let mut checked = 0;
        for page in &pages {
            let text = std::fs::read_to_string(page).expect("a listed page is readable");
            let here = page.parent().unwrap_or(Path::new("."));
            let mut rest = text.as_str();
            while let Some(open) = rest.find("](") {
                let after = &rest[open + 2..];
                let Some(close) = after.find(')') else { break };
                let target = after[..close].trim();
                rest = &after[close..];

                // External, in-page, and reference-style links are somebody else's
                // problem — this checks the ones that name a file in this repository.
                if target.is_empty()
                    || target.starts_with('#')
                    || target.contains("://")
                    || target.starts_with("mailto:")
                {
                    continue;
                }
                let path = target.split('#').next().unwrap_or(target);
                if path.is_empty() {
                    continue;
                }
                checked += 1;
                if !here.join(path).exists() {
                    broken.push(format!(
                        "{}: [..]({target})",
                        page.strip_prefix(&root).unwrap_or(page).display()
                    ));
                }
            }
        }

        assert!(checked > 20, "only {checked} links found — the scan is not working");
        assert!(
            broken.is_empty(),
            "documentation links point at files that do not exist:\n  {}",
            broken.join("\n  ")
        );
    }

    /// The counter reads as a sentence at one match as well as at many. Trivial, and the
    /// reason it is written down is that "1 matches" is the kind of thing that ships.
    #[test]
    fn a_match_count_agrees_with_its_number() {
        assert_eq!(match_count(0), "0 matches");
        assert_eq!(match_count(1), "1 match");
        assert_eq!(match_count(12), "12 matches");
    }

    /// The manual is worth searching in the first place: enough sections that the rail is
    /// a scroll, which is what the field is for. Guards the case where the document is
    /// gutted and the feature quietly becomes furniture.
    #[test]
    fn the_manual_is_long_enough_to_need_searching() {
        let rendered = help::render_doc(help::MANUAL.markdown);
        assert!(
            rendered.sections.len() > 10,
            "found {} sections",
            rendered.sections.len()
        );
    }

    /// The sidebar's paths are the ones proved to exist above.
    ///
    /// `COMPANIONS` and the `include_str!` list are two independent sets of string
    /// literals kept in step by hand: the compiler proves the *included* paths exist and
    /// says nothing about the ones the buttons actually use. This is the half that was
    /// missing — a companion could point at a moved file and still build.
    #[test]
    fn every_companion_path_exists_in_the_repository() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        for (label, path) in COMPANIONS {
            assert!(
                root.join(path).exists(),
                "the '{label}' button points at '{path}', which is not in the repository"
            );
        }
    }
}

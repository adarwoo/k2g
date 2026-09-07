//! In-context help: embedded Markdown reference docs, rendered inside the application.
//!
//! Two consumers, one renderer. [`HelpButton`] drops a page into a modal overlay beside
//! the feature it explains (the CNC editor's template syntax); [`MANUAL`] is the whole
//! user manual, shown as its own screen from the navigation rail
//! ([`crate::ui::screens`]).
//!
//! Pages are compiled into the binary with `include_str!`, so they ship as read-only
//! reference and are never copied into the user's data directory the way editable assets
//! are. The manual is included straight from `docs/` rather than from a copy under
//! `assets/help/`: it is the repository's own documentation, read on GitHub as well as
//! here, and two copies of it would be two manuals inside one release.
//!
//! # Why the renderer strips links
//!
//! Markdown is rendered to HTML and injected with `dangerous_inner_html`. That is safe as
//! to *content* — the source is build-time text, never user input — but an `<a href>` in a
//! WebView is a different hazard: following one replaces k2g with a web page in a window
//! that has no address bar, no Back button and no tabs. The About screen states the rule
//! and shows the alternative (`about::ExternalLink` — a button that hands the URL to the
//! system browser), and [`render_doc`] applies the same rule to prose: every link is
//! flattened to its own text, so nothing inside a rendered page can navigate anywhere.
//!
//! The two things that would otherwise be lost come back as real controls the screen owns:
//! the manual's own table of contents becomes [`RenderedDoc::sections`], and its links to
//! sibling documents become buttons in the Manual screen's sidebar.

use dioxus::prelude::*;
use pulldown_cmark::{html, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

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

/// The user manual, shown by the Manual screen.
pub const MANUAL: HelpDoc = HelpDoc {
    button_label: "User manual",
    title: "User manual",
    markdown: include_str!("../../docs/user-manual.md"),
};

/// One top-level section of a rendered page: an `id` on its heading, and the heading's
/// own words. This is what a contents list is built from.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub id: String,
    pub title: String,
}

/// One section that a search matched, and how many times it did.
///
/// Carries the heading's own `id` so a result is clicked the same way a contents entry
/// is — by scrolling to the anchor, never by following a link.
#[derive(Clone, Debug, PartialEq)]
pub struct SectionHits {
    pub id: String,
    pub title: String,
    pub hits: usize,
}

/// A rendered page: the HTML to inject, the sections a contents list can jump to, and —
/// when the page was rendered for a search — where the matches are.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderedDoc {
    pub html: String,
    pub sections: Vec<Section>,
    /// Sections containing at least one match, in document order. Empty when nothing was
    /// searched for.
    pub hits: Vec<SectionHits>,
    /// Every match in the page, including any above the first level-2 heading — so this
    /// can exceed the sum over [`hits`](Self::hits), and it is what the match counter and
    /// the `manual-hit-{n}` ids are numbered against.
    pub total_hits: usize,
}

/// How many characters a query needs before it highlights anything.
///
/// One is not a useful search of a manual: "e" marks several thousand places, which is
/// no more informative than not searching and costs a full re-render on the keystroke
/// that opens every session. Two is the shortest query that says something — and it
/// keeps `Z`, `G` and the other single letters an operator might try from producing a
/// page of noise instead of an answer.
pub const MIN_QUERY: usize = 2;

/// Renders a help page to HTML, with heading anchors and no navigable links.
///
/// Three things happen beyond plain CommonMark, each because the page is being read
/// *inside the application* rather than on a hosting service:
///
/// 1. **Every heading gets an id**, slugged the way GitHub slugs one, so the same document
///    anchors identically in both places and the screen can scroll to a section.
/// 2. **Links are flattened to their text.** See the module note — an `href` here would
///    navigate the application window itself.
/// 3. **A table of contents in the source is dropped**, because the screen renders
///    [`RenderedDoc::sections`] as a live sidebar and two contents lists on one page is one
///    too many. "A heading called Contents introduces a table of contents" is the whole
///    rule; everything down to the next heading of that level or above goes with it.
///
/// GFM alerts (`> [!WARNING]`) are enabled, so the manual's safety warning arrives as a
/// blockquote carrying `markdown-alert-warning` rather than as a paragraph beginning with
/// a literal `[!WARNING]`.
pub fn render_doc(markdown: &str) -> RenderedDoc {
    render(markdown, "")
}

/// [`render_doc`], with every occurrence of `query` wrapped in a `<mark>` and counted.
///
/// Matching is case-insensitive and by substring, which is what a reader expects of a
/// find field: typing "feed" finds "Feedrate". A query shorter than [`MIN_QUERY`]
/// searches for nothing, so the page comes back exactly as [`render_doc`] renders it.
///
/// # Why the search runs here and not in the WebView
///
/// The obvious implementation is a few lines of JavaScript through `document::eval` that
/// walk the text nodes and wrap the matches. It is also the one place on this screen
/// where text typed by the user would be interpolated into a script — the hazard the
/// module note describes, arrived at from the other direction. Marking the matches while
/// the page is built keeps the query as *data*: it is compared against text and never
/// becomes part of a program, and the only thing that reaches `eval` is an integer
/// counter generated here (see `manual::focus_hit`).
///
/// The cost is that a keystroke re-renders the page rather than touching the DOM. The
/// manual is some forty kilobytes and this sits behind a `use_memo`, so it costs a few
/// milliseconds when the query changes and nothing at all on any other re-render.
///
/// Each mark carries `id="manual-hit-{n}"`, numbered across the whole page in document
/// order, which is what lets the screen step from one match to the next.
pub fn render_doc_matching(markdown: &str, query: &str) -> RenderedDoc {
    render(markdown, query)
}

/// The renderer both entry points share. An empty `query` — or one shorter than
/// [`MIN_QUERY`] — highlights nothing, which is the plain-rendering case.
fn render(markdown: &str, query: &str) -> RenderedDoc {
    // Held as `char`s rather than a lowercased `String`, because a match has to be sliced
    // back out of the *original* text to keep its own capitals, and lowercasing is not
    // length-preserving — offsets found in a lowered copy can land mid-character in the
    // original.
    let trimmed = query.trim();
    let needle: Vec<char> = if trimmed.chars().count() >= MIN_QUERY {
        trimmed.chars().collect()
    } else {
        Vec::new()
    };

    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_GFM);

    let events: Vec<Event> = Parser::new_ext(markdown, options).collect();
    let headings = heading_spans(&events);
    let dropped = contents_range(&headings, events.len());

    let mut sections = Vec::new();
    let mut used_slugs: Vec<String> = Vec::new();
    let mut anchors: Vec<(usize, String)> = Vec::new();
    for heading in &headings {
        if dropped.as_ref().is_some_and(|range| range.contains(&heading.start)) {
            continue;
        }
        let id = unique_slug(&heading.text, &mut used_slugs);
        if heading.level == HeadingLevel::H2 {
            sections.push(Section { id: id.clone(), title: heading.text.clone() });
        }
        anchors.push((heading.start, id));
    }

    let anchor_for = |at: usize| {
        anchors
            .iter()
            .find(|(start, _)| *start == at)
            .map(|(_, id)| id.clone())
    };

    // Matches per section, parallel to `sections`. `walking` is the section being read
    // now, and stays `None` until the first level-2 heading: the title and any preamble
    // above it belong to no section, so a match there counts in the total and is
    // attributed to nothing — which is why the two figures need not agree.
    let mut counts = vec![0usize; sections.len()];
    let mut walking: Option<usize> = None;
    let mut entered = 0usize;
    let mut next_hit = 0usize;

    let mut kept: Vec<Event> = Vec::with_capacity(events.len());
    for (at, event) in events.into_iter().enumerate() {
        if dropped.as_ref().is_some_and(|range| range.contains(&at)) {
            continue;
        }
        match event {
            Event::Start(Tag::Heading { level, classes, attrs, .. }) => {
                if level == HeadingLevel::H2 {
                    walking = Some(entered);
                    entered += 1;
                }
                kept.push(Event::Start(Tag::Heading {
                    level,
                    id: anchor_for(at).map(Into::into),
                    classes,
                    attrs,
                }));
            }
            // The link's *text* is the events between these two, and they are kept — so
            // "see [Privacy](../PRIVACY.md)" reads as "see Privacy" and clicks nowhere.
            Event::Start(Tag::Link { .. }) | Event::End(TagEnd::Link) => {}

            // Prose, table cells and the inside of fenced blocks all arrive as `Text`. A
            // match turns the run into raw HTML, so the escaping this event would have
            // been given by the renderer has to be done by hand — see `mark_matches`.
            Event::Text(text) if !needle.is_empty() => {
                let mut marked = String::new();
                let found = mark_matches(&text, &needle, &mut next_hit, &mut marked);
                if found == 0 {
                    kept.push(Event::Text(text));
                } else {
                    if let Some(section) = walking {
                        counts[section] += found;
                    }
                    kept.push(Event::InlineHtml(marked.into()));
                }
            }

            // Inline code is its own event, and a manual keeps a lot of its nouns in
            // backticks — `G01`, `drill`. Leaving it out would have a reader searching in
            // vain for the very terms the document sets apart. The `<code>` the renderer
            // would have written is written here instead, because the marks go inside it.
            Event::Code(text) if !needle.is_empty() => {
                let mut marked = String::new();
                let found = mark_matches(&text, &needle, &mut next_hit, &mut marked);
                if found == 0 {
                    kept.push(Event::Code(text));
                } else {
                    if let Some(section) = walking {
                        counts[section] += found;
                    }
                    kept.push(Event::InlineHtml(format!("<code>{marked}</code>").into()));
                }
            }

            other => kept.push(other),
        }
    }

    let hits = sections
        .iter()
        .zip(&counts)
        .filter(|(_, &found)| found > 0)
        .map(|(section, &found)| SectionHits {
            id: section.id.clone(),
            title: section.title.clone(),
            hits: found,
        })
        .collect();

    let mut out = String::with_capacity(markdown.len() * 2);
    html::push_html(&mut out, kept.into_iter());
    RenderedDoc { html: out, sections, hits, total_hits: next_hit }
}

/// Writes `text` into `out` as HTML, wrapping each match of `needle` in a numbered
/// `<mark>`; returns how many it wrapped.
///
/// Everything that is not a match is escaped on the way through, which is not optional:
/// the result is handed back as raw HTML, so a `<` in the manual's prose that the
/// renderer would have escaped has to be escaped here or it becomes a tag.
fn mark_matches(text: &str, needle: &[char], next_hit: &mut usize, out: &mut String) -> usize {
    let mut found = 0;
    let mut at = 0;
    while let Some((start, end)) = find_ignoring_case(text, needle, at) {
        escape_html(&text[at..start], out);
        out.push_str("<mark class=\"manual-hit\" id=\"manual-hit-");
        out.push_str(&next_hit.to_string());
        out.push_str("\">");
        // Sliced out of the original rather than echoing the query back, so the page
        // keeps its own capitals: a search for "gcode" marks the document's "GCode".
        escape_html(&text[start..end], out);
        out.push_str("</mark>");
        *next_hit += 1;
        found += 1;
        at = end;
    }
    escape_html(&text[at..], out);
    found
}

/// The first match of `needle` in `haystack` at or after the byte offset `from`, given
/// back as byte offsets into `haystack`.
///
/// Written out rather than `to_lowercase().find()` because those offsets have to index
/// the *original* string: lowercasing can change a string's length, so a position found
/// in a lowered copy may slice the original in the wrong place — at best the wrong
/// characters, at worst a panic in the middle of one.
fn find_ignoring_case(haystack: &str, needle: &[char], from: usize) -> Option<(usize, usize)> {
    if needle.is_empty() || from >= haystack.len() {
        return None;
    }
    'start: for (offset, _) in haystack[from..].char_indices() {
        let start = from + offset;
        let mut at = start;
        for &wanted in needle {
            // Out of text: no later start can match either, so this is the end of it.
            let Some(got) = haystack[at..].chars().next() else {
                return None;
            };
            if !eq_ignoring_case(got, wanted) {
                continue 'start;
            }
            at += got.len_utf8();
        }
        return Some((start, at));
    }
    None
}

/// Two characters compared without regard to case.
///
/// Compares the full lowercase mappings rather than `to_ascii_lowercase`, so an accented
/// word matches a query typed in either case. A character whose lowering changes its
/// *length* (`ß` to "ss") will not match, which no page in this application contains.
fn eq_ignoring_case(a: char, b: char) -> bool {
    a == b || a.to_lowercase().eq(b.to_lowercase())
}

/// The characters that cannot be written literally into HTML text.
fn escape_html(text: &str, out: &mut String) {
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
}

/// One heading found in the event stream: where it starts, what level it is, and the
/// words in it.
///
/// The text is needed to slug the heading, and it arrives *after* the start event — which
/// is why the whole stream is collected up front rather than rendered as it is parsed.
struct HeadingSpan {
    start: usize,
    level: HeadingLevel,
    text: String,
}

/// Every heading in `events`, in document order.
fn heading_spans(events: &[Event]) -> Vec<HeadingSpan> {
    let mut spans = Vec::new();
    let mut current: Option<HeadingSpan> = None;

    for (at, event) in events.iter().enumerate() {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                current = Some(HeadingSpan { start: at, level: *level, text: String::new() });
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some(span) = current.take() {
                    spans.push(span);
                }
            }
            // `Code` as well as `Text`, so a heading like "The `drill` primitive" slugs
            // from what the reader sees rather than losing the word in backticks.
            Event::Text(text) | Event::Code(text) => {
                if let Some(span) = current.as_mut() {
                    span.text.push_str(text);
                }
            }
            _ => {}
        }
    }

    spans
}

/// The events belonging to a table of contents, if the page has one: from its heading up
/// to the next heading of the same level or above (so the rule is nesting, not a fixed
/// number of blocks — the trailing rule under the manual's list goes with it).
fn contents_range(headings: &[HeadingSpan], total: usize) -> Option<std::ops::Range<usize>> {
    let at = headings
        .iter()
        .position(|heading| heading.text.trim().eq_ignore_ascii_case("contents"))?;
    let contents = &headings[at];
    let end = headings[at + 1..]
        .iter()
        .find(|heading| heading.level <= contents.level)
        .map(|heading| heading.start)
        .unwrap_or(total);
    Some(contents.start..end)
}

/// GitHub's heading slug: lower-cased, punctuation dropped, spaces to hyphens.
///
/// Deliberately *not* collapsing the runs of hyphens that leaves behind — "Quick start —
/// your first board" slugs to `quick-start--your-first-board`, with two, because the em
/// dash is dropped from between two spaces. Matching GitHub exactly is what lets one
/// document carry one set of anchors and work in both places.
fn slug(text: &str) -> String {
    text.chars()
        .filter_map(|ch| match ch {
            ' ' | '-' => Some('-'),
            ch if ch.is_alphanumeric() => Some(ch.to_ascii_lowercase()),
            _ => None,
        })
        .collect()
}

/// [`slug`], made unique within one page by suffixing a repeat — again as GitHub does.
///
/// Two headings with the same words are perfectly reasonable ("Notes" under each of three
/// sections). Without this they would share an id, and every contents entry but the first
/// would scroll to the wrong place — a fault that looks like the button being broken.
fn unique_slug(text: &str, used: &mut Vec<String>) -> String {
    let base = slug(text);
    let mut candidate = base.clone();
    let mut repeat = 0;
    while used.contains(&candidate) {
        repeat += 1;
        candidate = format!("{base}-{repeat}");
    }
    used.push(candidate.clone());
    candidate
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
    let rendered = render_doc(doc.markdown).html;

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

#[cfg(test)]
mod tests {
    use super::*;

    /// The rule the module exists to keep: nothing in a rendered page can navigate the
    /// WebView. Asserted over the **shipping** manual rather than a sample, because the
    /// manual is the page with links in it and the next edit to it is where this would
    /// come back.
    #[test]
    fn a_rendered_page_contains_no_navigable_link() {
        for doc in [MANUAL, GTL] {
            let html = render_doc(doc.markdown).html;
            assert!(
                !html.contains("<a "),
                "{} rendered an anchor element — an href in the WebView replaces the \
                 application with a web page the user cannot leave (see about.rs)",
                doc.title,
            );
        }
    }

    /// A flattened link keeps its words: the sentence around it has to survive.
    #[test]
    fn a_link_is_replaced_by_its_own_text() {
        let rendered = render_doc("See [the privacy note](../PRIVACY.md) for detail.");
        assert!(rendered.html.contains("See the privacy note for detail."), "{}", rendered.html);
        assert!(!rendered.html.contains("PRIVACY.md"), "and the destination is gone");
    }

    /// Headings anchor the way GitHub anchors them, so one document has one set of
    /// anchors wherever it is read.
    #[test]
    fn headings_slug_the_way_github_does() {
        assert_eq!(slug("Contents"), "contents");
        assert_eq!(slug("6. CNC profiles"), "6-cnc-profiles");
        assert_eq!(slug("Profiles: the shared rules"), "profiles-the-shared-rules");
        // The em dash goes, and the spaces either side of it do not — two hyphens, which
        // is what the anchor in the source document says too.
        assert_eq!(slug("Quick start — your first board"), "quick-start--your-first-board");
    }

    /// Repeated headings must not share an id, or every contents entry but the first
    /// scrolls to the wrong section.
    #[test]
    fn a_repeated_heading_gets_its_own_anchor() {
        let rendered = render_doc("## Notes\ntext\n\n## Notes\nmore\n");
        let ids: Vec<&str> = rendered.sections.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["notes", "notes-1"]);
    }

    /// The sections are what the sidebar lists: level-2 headings, in document order, and
    /// not the contents list itself.
    #[test]
    fn the_sections_are_the_level_two_headings() {
        let rendered = render_doc(
            "# Title\n\n## Contents\n\n1. [One](#one)\n2. [Two](#two)\n\n---\n\n\
             ## One\n\n### Deeper\n\ntext\n\n## Two\n\ntext\n",
        );
        let titles: Vec<&str> = rendered.sections.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, ["One", "Two"], "h1 and h3 are not sections; Contents is gone");
    }

    /// A contents list in the source is dropped whole — heading, list and the rule under
    /// it — because the screen shows a live one beside the page.
    #[test]
    fn a_table_of_contents_is_dropped_with_everything_under_it() {
        let rendered = render_doc(
            "## Contents\n\n1. [One](#one)\n\n---\n\n## One\n\nthe body\n",
        );
        assert!(!rendered.html.contains("Contents"), "{}", rendered.html);
        assert!(!rendered.html.contains("<hr"), "the rule under the list goes too");
        assert!(rendered.html.contains("the body"), "and the document itself stays");
    }

    /// Only a heading actually called "Contents" triggers it. A page without one renders
    /// entire — which is every help page but the manual.
    #[test]
    fn a_page_without_a_contents_heading_is_untouched() {
        let rendered = render_doc("## Setup\n\nfirst\n\n## Usage\n\nsecond\n");
        assert!(rendered.html.contains("first") && rendered.html.contains("second"));
        assert_eq!(rendered.sections.len(), 2);
    }

    /// The manual's safety warning is a GFM alert. Without `ENABLE_GFM` it would render as
    /// a blockquote opening with a literal `[!WARNING]`, which reads as a typo.
    #[test]
    fn a_gfm_alert_renders_as_an_alert() {
        let rendered = render_doc("> [!WARNING]\n> Mind the spindle.\n");
        assert!(rendered.html.contains("markdown-alert-warning"), "{}", rendered.html);
        assert!(!rendered.html.contains("[!WARNING]"), "the marker is consumed");
    }

    /// The shipping manual must actually render: sections in it, and the safety warning
    /// still an alert. This is the test that fails if the file is moved or gutted.
    #[test]
    fn the_shipping_manual_renders_with_its_sections() {
        let rendered = render_doc(MANUAL.markdown);
        assert!(
            rendered.sections.len() > 10,
            "the manual has 16 numbered sections, found {}",
            rendered.sections.len()
        );
        assert_eq!(rendered.sections[0].title, "1. How k2g is put together");
        assert!(rendered.html.contains("markdown-alert-warning"), "the safety warning");
    }

    /// Searching marks what it finds, whatever case either side is written in, and counts
    /// it against the section it is in.
    #[test]
    fn a_search_marks_and_counts_its_matches() {
        // Neither heading contains the query, so the count is the body's alone — the
        // heading case has its own test below.
        let rendered = render_doc_matching(
            "## Cutting\n\nThe feedrate and the FEED per tooth.\n\n## Speeds\n\nNo match here.\n",
            "feed",
        );
        assert_eq!(rendered.total_hits, 2);
        assert_eq!(rendered.hits.len(), 1, "only one section matched: {:?}", rendered.hits);
        assert_eq!(rendered.hits[0].title, "Cutting");
        assert_eq!(rendered.hits[0].hits, 2);
        // The page keeps its own capitals; the query does not overwrite them.
        assert!(rendered.html.contains(">feed</mark>"), "{}", rendered.html);
        assert!(rendered.html.contains(">FEED</mark>"), "{}", rendered.html);
    }

    /// The ids run in document order across the whole page, because stepping between
    /// matches is `manual-hit-{n}` counted up and down.
    #[test]
    fn matches_are_numbered_across_the_page() {
        let rendered = render_doc_matching("## One\n\naa\n\n## Two\n\naa aa\n", "aa");
        assert_eq!(rendered.total_hits, 3);
        for n in 0..3 {
            assert!(
                rendered.html.contains(&format!("id=\"manual-hit-{n}\"")),
                "missing hit {n}: {}",
                rendered.html
            );
        }
    }

    /// A manual keeps its nouns in backticks, so inline code has to be searchable too —
    /// and the `<code>` element has to survive being rewritten.
    #[test]
    fn inline_code_is_searchable() {
        let rendered = render_doc_matching("## Codes\n\nUse the `drill` primitive.\n", "drill");
        assert_eq!(rendered.total_hits, 1);
        assert!(rendered.html.contains("<code><mark"), "{}", rendered.html);
        assert!(rendered.html.contains("</mark></code>"), "{}", rendered.html);
    }

    /// Rewriting a run into raw HTML means this module owns the escaping. A page whose
    /// prose contains a `<` must not have it become a tag because something nearby matched.
    #[test]
    fn a_marked_run_still_escapes_its_html() {
        let rendered = render_doc_matching("## S\n\nfeed rate < 200 and a & b\n", "feed");
        assert!(rendered.html.contains("&lt; 200"), "{}", rendered.html);
        assert!(rendered.html.contains("a &amp; b"), "{}", rendered.html);
    }

    /// Too short to be a search: the page comes back exactly as it renders unsearched, so
    /// the first keystroke does not mark several thousand places.
    #[test]
    fn a_query_below_the_minimum_marks_nothing() {
        let source = "## Feeds\n\nThe feedrate.\n";
        for query in ["", " ", "f"] {
            let rendered = render_doc_matching(source, query);
            assert_eq!(rendered.total_hits, 0, "query {query:?}");
            assert_eq!(rendered.html, render_doc(source).html, "query {query:?}");
        }
    }

    /// A match in a heading belongs to that heading's own section, not the one before it.
    #[test]
    fn a_heading_match_counts_against_its_own_section() {
        let rendered = render_doc_matching("## Drilling\n\nbody\n\n## Routing\n\nbody\n", "drill");
        assert_eq!(rendered.hits.len(), 1);
        assert_eq!(rendered.hits[0].title, "Drilling");
        assert_eq!(rendered.hits[0].id, "drilling", "clicked by anchor, not by link");
    }

    /// The rule the module exists to keep, restated for the search: highlighting adds
    /// `<mark>` and must never add a way out of the application.
    #[test]
    fn searching_the_manual_adds_no_navigable_link() {
        let rendered = render_doc_matching(MANUAL.markdown, "board");
        assert!(rendered.total_hits > 0, "the manual talks about boards");
        assert!(!rendered.html.contains("<a "), "an href here replaces the application");
    }
}


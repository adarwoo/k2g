//! The About screen: identity, version, authorship and the third-party stack.
//!
//! Static content — it takes the shell `state` only to match the screen-dispatch
//! signature in [`super`]. Package facts come from Cargo's `CARGO_PKG_*` build
//! environment so they never drift from `Cargo.toml`.

use dioxus::prelude::*;
use std::sync::OnceLock;

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;

/// The ARex logo as a self-contained data URL (embedded at build time so it needs
/// no filesystem access at runtime). Replace `assets/icons/arex.png` to change it.
fn arex_logo_data_url() -> &'static str {
    static LOGO: OnceLock<String> = OnceLock::new();
    LOGO.get_or_init(|| {
        let bytes = include_bytes!("../../../assets/icons/arex.png");
        format!("data:image/png;base64,{}", BASE64_STANDARD.encode(bytes))
    })
}

/// Splits `CARGO_PKG_AUTHORS`' first entry into `(name, email)`; the email is
/// `None` when the `Name <email>` form is not used.
fn primary_author() -> (&'static str, Option<&'static str>) {
    let first = env!("CARGO_PKG_AUTHORS").split(':').next().unwrap_or("");
    match first.split_once('<') {
        Some((name, rest)) => (name.trim(), rest.strip_suffix('>').map(str::trim)),
        None => (first.trim(), None),
    }
}

#[component]
pub fn AboutScreen(state: Signal<crate::runtime::AppCtx>) -> Element {
    // Present but unused: About is static. Touch it so the prop is not flagged.
    let _ = state;

    let version = env!("CARGO_PKG_VERSION");
    let description = env!("CARGO_PKG_DESCRIPTION");
    let repository = env!("CARGO_PKG_REPOSITORY");
    let (author_name, author_email) = primary_author();

    rsx! {
        div { class: "screen single about-screen",
            section { class: "about-card",
                img { class: "about-logo", src: arex_logo_data_url(), alt: "ARex logo" }

                div { class: "about-headline",
                    h1 { class: "about-title", "K2G" }
                    p { class: "about-tagline", "KiCad → GCode — CAM for machining PCBs" }
                    span { class: "about-codename", "“ARex”" }
                }

                dl { class: "about-facts",
                    div { class: "about-fact",
                        dt { "Version" }
                        dd { class: "mono", "{version}" }
                    }
                    div { class: "about-fact",
                        dt { "Author" }
                        dd {
                            "{author_name}"
                            if let Some(email) = author_email {
                                a { class: "about-link", href: "mailto:{email}", " · {email}" }
                            }
                        }
                    }
                    div { class: "about-fact",
                        dt { "Purpose" }
                        dd { "{description}" }
                    }
                    div { class: "about-fact",
                        dt { "Source" }
                        dd { a { class: "about-link", href: "{repository}", "{repository}" } }
                    }
                }

                p { class: "about-note",
                    "“ARex” is a Tyrannosaurus rex — "
                    strong { "A. Rex" }
                    ", for Arreckx."
                }

                div { class: "about-credits",
                    h2 { class: "about-credits-title", "Built with" }
                    ul { class: "about-credits-list",
                        li { "Rust — the whole application" }
                        li { "Dioxus — the desktop UI" }
                        li { "KiCad IPC API — PCB acquisition" }
                        li { "Rhai — the GCode template language" }
                        li { "Clipper2 — board-outline stitching" }
                    }
                }
            }
        }
    }
}

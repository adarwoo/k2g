//! The Save control in the top bar: write the generated program to disk, or write it to
//! a removable medium and eject that in one action.
//!
//! It lives in the top bar rather than on the Job → Code tab because the program is the
//! thing the whole app produces, and having to navigate to one tab to get it made the
//! last step the least reachable. Being global also means the save status cannot be shown
//! inline any more — it goes out as a toast, which is visible from wherever the user
//! happened to be.
//!
//! ## The split button
//!
//! The second, narrow button appears **only when a removable medium is plugged in**. That
//! is deliberate: its appearance is itself the signal that the stick is ready, and a
//! permanently greyed USB button on a machine that never has one is noise. The main Save
//! button, by contrast, is always visible and merely disabled when there is no program —
//! a control that vanishes is a control the user cannot find again.

use std::fs;
use std::path::{Path, PathBuf};

use dioxus::prelude::*;
use rfd::FileDialog;

use crate::runtime::removable::{self, SaveTarget};
use crate::runtime::{with_ctx_mut, AppCtx, GCODE_FILE_EXTENSION};

/// The Save control: an ordinary save, plus a save-to-removable-media button when there
/// is somewhere to save to.
#[component]
pub fn SaveProgramButton(state: Signal<AppCtx>) -> Element {
    let snapshot = state.read().clone();
    let has_program = !snapshot.gcode.trim().is_empty();

    // Read during render, which is sound only because the watcher bumps the shared UI
    // wake channel on every material change and `AppRoot` re-renders this subtree from
    // it — see `runtime::removable::removable_media`. There is no signal to subscribe to.
    let media = removable::removable_media();
    let target = removable::save_target(&media, snapshot.last_removable_media_path.as_deref());

    rsx! {
        div { class: "topbar-save",
            button {
                class: "btn btn-primary",
                r#type: "button",
                disabled: !has_program,
                title: if has_program { "Save the G-code program" } else { "No program to save yet" },
                onclick: move |_| save_anywhere(state),
                "Save…"
            }

            if let Some(target) = target {
                button {
                    class: "btn btn-primary topbar-save-media",
                    r#type: "button",
                    disabled: !has_program,
                    title: "Save to {target.medium.display_name()} and eject",
                    "aria-label": "Save to {target.medium.display_name()} and eject",
                    onclick: {
                        let target = target.clone();
                        move |_| save_to_medium(state, target.clone())
                    },
                    UsbIcon {}
                }
            }
        }
    }
}

/// A USB stick lying on its side: a body with the metal connector off one end.
///
/// It names the *destination*, not the action — the button it is attached to already says
/// "Save", so the pair reads "Save… to [this]". An eject symbol would suggest a button
/// that only unmounts, and an arrow would just repeat the word next to it.
///
/// Two plain rectangles and nothing inside them, because at 16 px anything finer merges —
/// a draft with the plug contacts and an arrow drawn in read as a padlock. Lying down for
/// the same reason: a small shape centred *above* a rounded body is a padlock shackle no
/// matter what it is meant to be, while nothing else in a toolbar is a horizontal bar with
/// a smaller tab off one end.
#[component]
fn UsbIcon() -> Element {
    rsx! {
        svg {
            class: "topbar-save-media-svg",
            view_box: "0 0 24 24",
            "aria-hidden": "true",
            rect { x: "2.5", y: "7.5", width: "13", height: "9", rx: "1.8" }
            rect { x: "15.5", y: "9.75", width: "6", height: "4.5", rx: "0.6" }
        }
    }
}

/// The ordinary Save: a dialog anywhere, no eject.
fn save_anywhere(state: Signal<AppCtx>) {
    let snapshot = state.read().clone();
    let Some(path) = run_save_dialog(
        state,
        &snapshot.gcode,
        &snapshot.gcode_save_directory_or_default(),
        &snapshot.gcode_default_file_name(),
    ) else {
        return; // cancelled, or reported already
    };

    if let Some(directory) = path.parent() {
        with_ctx_mut(|ctx| ctx.app.remember_gcode_save_directory(directory));
    }
    report(state, format!("Saved {}", file_name_of(&path)));
}

/// Save to removable media, then eject it.
///
/// The dialog still opens — the user may want a sub-folder, or a different name — it just
/// starts on the medium. Which medium gets ejected is decided by where the file *actually*
/// landed rather than by `target`, because the dialog lets the user navigate anywhere and
/// ejecting a drive the program was not written to would be both useless and alarming.
fn save_to_medium(state: Signal<AppCtx>, target: SaveTarget) {
    let snapshot = state.read().clone();
    let Some(path) = run_save_dialog(
        state,
        &snapshot.gcode,
        &target.directory,
        &snapshot.gcode_default_file_name(),
    ) else {
        return;
    };

    if let Some(directory) = path.parent() {
        with_ctx_mut(|ctx| ctx.app.remember_removable_media_path(directory));
    }

    // Re-read the media: the dialog was open for as long as the user wanted, and the
    // stick may well have been pulled in the meantime.
    let media = removable::removable_media();
    match removable::medium_for_path(&media, &path) {
        Some(medium) => {
            // Fire-and-forget: the eject waits on a volume lock for up to several seconds,
            // and this is the WebView thread. The outcome arrives as its own toast from
            // the watcher.
            report(state, format!("Saved {} — ejecting {}…", file_name_of(&path), medium.display_name()));
            removable::request_eject(medium);
        }
        None => {
            // Saved somewhere that is not removable media (the user navigated away, or the
            // stick went). Nothing to eject, and nothing has gone wrong.
            report(state, format!("Saved {}", file_name_of(&path)));
        }
    }
}

/// Runs the save dialog and writes the program.
///
/// The one implementation both buttons share; they differ only in where the dialog opens,
/// which remembered directory they update, and whether an eject follows. Returns `None`
/// when the user cancelled — indistinguishable from a failed write at the call site by
/// design, because a failure has already been reported by then and neither should record
/// a directory.
fn run_save_dialog(
    state: Signal<AppCtx>,
    program: &str,
    start_directory: &Path,
    file_name: &str,
) -> Option<PathBuf> {
    let path = FileDialog::new()
        .set_title("Save G-code program")
        .set_directory(start_directory)
        .set_file_name(file_name)
        .add_filter("G-code", &[GCODE_FILE_EXTENSION, "ngc", "gcode", "tap"])
        .add_filter("All files", &["*"])
        .save_file()?;

    match fs::write(&path, program.as_bytes()) {
        Ok(()) => Some(path),
        Err(err) => {
            // Reported through the same path as a success, and taking `state` for exactly
            // that reason: a bare `with_ctx_mut` here would write the toast into the global
            // context without re-syncing the signal the notification stack renders from, so
            // the one message that must not be missed would sit unseen until something else
            // happened to re-render.
            log::error!("could not write {}: {err}", path.display());
            report(state, format!("Save failed: {err}"));
            None
        }
    }
}

/// Pushes a toast and re-syncs the UI signal.
///
/// `super::mutate_ctx` rather than a bare `with_ctx_mut`, because the toast has to reach
/// the signal the notification stack renders from — a global-context write alone would sit
/// there unseen until something else happened to re-sync.
fn report(state: Signal<AppCtx>, message: String) {
    log::info!("{message}");
    super::mutate_ctx(state, |ctx| ctx.log_event(message));
}

/// The file name to show in a toast, falling back to the whole path if it has none.
fn file_name_of(path: &Path) -> String {
    path.file_name().unwrap_or(path.as_os_str()).to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_toast_names_the_file_not_the_whole_path() {
        assert_eq!(file_name_of(Path::new("E:\\jobs\\panel.nc")), "panel.nc");
        assert_eq!(file_name_of(Path::new("panel.nc")), "panel.nc");
    }
}

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
    // Saveable means *both* that a program exists and that the job it came from is still
    // the job on screen. The orchestrator clears the programs when a regeneration it
    // cannot run falls due, so this is a second guard rather than the only one — but the
    // failure it prevents is writing the wrong G-code to a machine, so two is right. It
    // reads the same gate the status pill does, because a Save button live next to a
    // pill saying "Not ready" is the contradiction that started this.
    let is_ready = snapshot
        .status
        .get(crate::runtime::STATUS_KEY_GENERATION_READINESS)
        .map(|value| value == "true")
        .unwrap_or(false);
    let is_current = is_ready
        && !matches!(snapshot.generation_state, crate::ui::navigation::GenerationState::Running);
    let has_program = snapshot.has_any_program() && is_current;
    // A one-step job saves exactly as it always did: one dialog, `{board}.nc`, no modal
    // and no `_step1` suffix. The plan dialog exists only because N programs cannot each
    // be named by a save dialog when there is one folder prompt between them.
    let multi_step = snapshot.programs.len() > 1;
    let mut plan_rows = use_signal(Vec::<SaveRow>::new);
    let mut plan_to_medium = use_signal(|| false);
    let mut plan_start = use_signal(PathBuf::new);
    let mut plan_open = use_signal(|| false);

    // Read during render, which is sound only because the watcher bumps the shared UI
    // wake channel on every material change and `AppRoot` re-renders this subtree from
    // it — see `runtime::removable::removable_media`. There is no signal to subscribe to.
    let media = removable::removable_media();
    let target = removable::save_target(&media, snapshot.last_removable_media_path.as_deref());

    // Opening the plan is the same act either way; only where the folder prompt starts
    // and whether an eject follows differ.
    let mut open_plan = move |to_medium: bool, start: PathBuf| {
        let snapshot = state.read().clone();
        plan_rows.set(save_rows(&snapshot));
        plan_to_medium.set(to_medium);
        plan_start.set(start);
        plan_open.set(true);
    };

    rsx! {
        div { class: "topbar-save",
            button {
                class: "btn btn-primary",
                r#type: "button",
                disabled: !has_program,
                title: match (has_program, snapshot.has_any_program()) {
                    (true, _) => "Save the G-code program",
                    // Distinguish "nothing generated" from "what was generated no longer
                    // matches the job", which are different things to do something about.
                    (false, true) => "The job has changed — no current program to save",
                    (false, false) => "No program to save yet",
                },
                onclick: move |_| {
                    if multi_step {
                        let start = state.read().gcode_save_directory_or_default();
                        open_plan(false, start);
                    } else {
                        save_anywhere(state);
                    }
                },
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
                        move |_| {
                            if multi_step {
                                open_plan(true, target.directory.clone());
                            } else {
                                save_to_medium(state, target.clone());
                            }
                        }
                    },
                    UsbIcon {}
                }
            }

            if *plan_open.read() {
                SaveProgramDialog {
                    rows: plan_rows,
                    confirm_label: "Choose folder…".to_string(),
                    on_cancel: move |_| plan_open.set(false),
                    on_confirm: move |_| {
                        plan_open.set(false);
                        let rows = plan_rows.read().clone();
                        let start = plan_start.read().clone();
                        save_batch(state, rows, start, *plan_to_medium.read());
                    },
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

/// The pre-save plan: one row per step that produced a program, each named before the
/// folder is chosen.
///
/// Follows `ProfileNameDialog`'s shape (`wizard-overlay` / `wizard-dialog` /
/// `wizard-actions`, Escape to cancel) so it is the same dialog the rest of the app uses.
#[component]
fn SaveProgramDialog(
    rows: Signal<Vec<SaveRow>>,
    confirm_label: String,
    on_cancel: EventHandler<()>,
    on_confirm: EventHandler<()>,
) -> Element {
    let current = rows.read().clone();
    let problem = validate_rows(&current);

    rsx! {
        div { class: "wizard-overlay",
            div { class: "wizard-dialog save-plan-dialog",
                h2 { "Save programs" }
                p { class: "field-hint",
                    "Each machining step is its own program. Name them here, then choose one folder to write them to."
                }

                div { class: "save-plan-table",
                    for (position , row) in current.iter().enumerate() {
                        div { key: "{row.step_index}", class: "save-plan-row",
                            input {
                                r#type: "checkbox",
                                checked: row.include,
                                title: "Include this step",
                                onchange: move |evt| {
                                    let checked = evt.checked();
                                    rows.write()[position].include = checked;
                                },
                            }
                            div { class: "save-plan-step",
                                span { class: "save-plan-step-name", "{row.step_label}" }
                                span { class: "save-plan-step-meta",
                                    if row.cnc_name.is_empty() {
                                        "{row.line_count} lines"
                                    } else {
                                        "{row.cnc_name} · {row.line_count} lines"
                                    }
                                }
                            }
                            input {
                                class: "save-plan-name",
                                value: "{row.file_name}",
                                disabled: !row.include,
                                autofocus: position == 0,
                                oninput: move |evt| {
                                    let value = evt.value();
                                    rows.write()[position].file_name = value;
                                },
                                onkeydown: move |evt| {
                                    let key = evt.key().to_string().to_ascii_lowercase();
                                    if key == "escape" || key == "esc" {
                                        on_cancel.call(());
                                    }
                                },
                            }
                        }
                    }
                }

                if let Some(problem) = problem.clone() {
                    p { class: "diag-status diag-warning", "{problem}" }
                }

                div { class: "wizard-actions",
                    button {
                        class: "btn btn-secondary",
                        onclick: move |_| on_cancel.call(()),
                        "Cancel"
                    }
                    button {
                        class: "btn btn-primary",
                        disabled: problem.is_some(),
                        onclick: move |_| on_confirm.call(()),
                        "{confirm_label}"
                    }
                }
            }
        }
    }
}

/// Writes every planned program into one chosen folder.
///
/// `pick_folder` rather than a save dialog per file: with N programs the operator would
/// otherwise face N prompts, having already said what each is called.
fn save_batch(state: Signal<AppCtx>, rows: Vec<SaveRow>, start: PathBuf, to_medium: bool) {
    let Some(folder) = FileDialog::new()
        .set_title(if to_medium { "Choose a folder on the removable medium" } else { "Choose a folder for the programs" })
        .set_directory(&start)
        .pick_folder()
    else {
        return; // cancelled
    };
    if !confirm_overwrites(&rows, &folder) {
        return;
    }

    let snapshot = state.read().clone();
    let report = write_rows(&snapshot, &rows, &folder);

    with_ctx_mut(|ctx| {
        if to_medium {
            ctx.app.remember_removable_media_path(&folder);
        } else {
            ctx.app.remember_gcode_save_directory(&folder);
        }
    });

    if !report.failed.is_empty() {
        let (name, err) = &report.failed[0];
        report_message(
            state,
            format!(
                "Saved {} of {} — {name} failed: {err}",
                report.written.len(),
                report.written.len() + report.failed.len()
            ),
        );
    } else {
        report_message(
            state,
            format!("Saved {} programs to {}", report.written.len(), folder.display()),
        );
    }

    // Eject only when the whole batch landed: a partial write leaves the medium mounted
    // so the operator can retry the rest.
    if to_medium && report.all_written() {
        let media = removable::removable_media();
        if let Some(medium) = removable::medium_for_path(&media, &folder) {
            report_message(state, format!("Ejecting {}…", medium.display_name()));
            removable::request_eject(medium);
        }
    }
}

/// The ordinary Save: a dialog anywhere, no eject.
fn save_anywhere(state: Signal<AppCtx>) {
    let snapshot = state.read().clone();
    let Some(program) = snapshot.selected_program().and_then(|step| step.program()) else {
        return;
    };
    let Some(path) = run_save_dialog(
        state,
        &program.text,
        &snapshot.gcode_save_directory_or_default(),
        &snapshot.gcode_default_file_name(),
    ) else {
        return; // cancelled, or reported already
    };

    if let Some(directory) = path.parent() {
        with_ctx_mut(|ctx| ctx.app.remember_gcode_save_directory(directory));
    }
    report_message(state, format!("Saved {}", file_name_of(&path)));
}

/// Save to removable media, then eject it.
///
/// The dialog still opens — the user may want a sub-folder, or a different name — it just
/// starts on the medium. Which medium gets ejected is decided by where the file *actually*
/// landed rather than by `target`, because the dialog lets the user navigate anywhere and
/// ejecting a drive the program was not written to would be both useless and alarming.
fn save_to_medium(state: Signal<AppCtx>, target: SaveTarget) {
    let snapshot = state.read().clone();
    let Some(program) = snapshot.selected_program().and_then(|step| step.program()) else {
        return;
    };
    let Some(path) = run_save_dialog(
        state,
        &program.text,
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
            report_message(state, format!("Saved {} — ejecting {}…", file_name_of(&path), medium.display_name()));
            removable::request_eject(medium);
        }
        None => {
            // Saved somewhere that is not removable media (the user navigated away, or the
            // stick went). Nothing to eject, and nothing has gone wrong.
            report_message(state, format!("Saved {}", file_name_of(&path)));
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
            report_message(state, format!("Save failed: {err}"));
            None
        }
    }
}

/// One row of the pre-save plan: a step, the name its program will be written under, and
/// whether it is written at all.
///
/// The names are settled **before** the folder is chosen because `pick_folder` cannot ask
/// per file the way `save_file` does — with N programs there is one directory prompt and
/// no opportunity to name anything after it.
#[derive(Clone, PartialEq)]
pub(crate) struct SaveRow {
    pub step_index: usize,
    pub step_label: String,
    pub cnc_name: String,
    pub line_count: usize,
    pub include: bool,
    pub file_name: String,
}

/// The rows a multi-step save starts from: one per step that produced a program.
///
/// Named from the **step count**, not the number of programs: a three-step profile whose
/// middle step failed still writes `board_step3.nc`, because that ordinal is what tells
/// the operator which setup the file belongs to.
fn save_rows(snapshot: &AppCtx) -> Vec<SaveRow> {
    let step_count = snapshot.programs.len();
    snapshot
        .ready_programs()
        .into_iter()
        .map(|(step, program)| SaveRow {
            step_index: step.index,
            step_label: if step.name.trim().is_empty() {
                format!("Step {}", step.index + 1)
            } else {
                format!("Step {}: {}", step.index + 1, step.name)
            },
            cnc_name: step.cnc_name.clone(),
            line_count: program.text.lines().count(),
            include: true,
            file_name: snapshot.program_file_name(step.index, step_count, &program.extension),
        })
        .collect()
}

/// Why the plan cannot be written yet, or `None` when it can.
///
/// Checked before the folder prompt rather than after: discovering a clash once the
/// directory is chosen would mean either losing the choice or writing some of the batch.
pub(crate) fn validate_rows(rows: &[SaveRow]) -> Option<String> {
    let included: Vec<&SaveRow> = rows.iter().filter(|row| row.include).collect();
    if included.is_empty() {
        return Some("Select at least one program to save.".to_string());
    }
    for row in &included {
        let name = row.file_name.trim();
        if name.is_empty() {
            return Some(format!("{} needs a file name.", row.step_label));
        }
        // A separator would silently write outside the chosen folder, which is exactly
        // what choosing a folder once was meant to prevent.
        if name.contains('/') || name.contains('\\') {
            return Some(format!("'{name}' cannot contain a path separator."));
        }
    }
    let mut seen: Vec<String> = Vec::new();
    for row in &included {
        let name = row.file_name.trim().to_lowercase();
        if seen.contains(&name) {
            return Some(format!("'{}' is used twice — each step needs its own name.", row.file_name.trim()));
        }
        seen.push(name);
    }
    None
}

/// What a batch save actually did. Success is not all-or-nothing, and a half-written
/// batch has to say which half.
struct SaveReport {
    written: Vec<String>,
    failed: Vec<(String, String)>,
}

impl SaveReport {
    fn all_written(&self) -> bool {
        self.failed.is_empty() && !self.written.is_empty()
    }
}

/// Writes every included row into `folder`.
///
/// Successful writes are **not** rolled back when a later one fails: they are valid
/// programs, and deleting them would be the worse failure.
fn write_rows(snapshot: &AppCtx, rows: &[SaveRow], folder: &Path) -> SaveReport {
    let mut report = SaveReport { written: Vec::new(), failed: Vec::new() };
    for row in rows.iter().filter(|row| row.include) {
        let Some(program) = snapshot
            .programs
            .get(row.step_index)
            .and_then(|step| step.program())
        else {
            continue;
        };
        let name = row.file_name.trim().to_string();
        let path = folder.join(&name);
        match fs::write(&path, program.text.as_bytes()) {
            Ok(()) => report.written.push(name),
            Err(err) => {
                log::error!("could not write {}: {err}", path.display());
                report.failed.push((name, err.to_string()));
            }
        }
    }
    report
}

/// Asks once about the files already in `folder` that the batch would replace.
///
/// `pick_folder` has no overwrite confirmation, where `save_file` gets one from the OS —
/// so without this, saving a multi-step job would silently clobber, which is a regression
/// against the single-step behaviour. Returns whether to proceed.
fn confirm_overwrites(rows: &[SaveRow], folder: &Path) -> bool {
    let existing: Vec<String> = rows
        .iter()
        .filter(|row| row.include)
        .map(|row| row.file_name.trim().to_string())
        .filter(|name| folder.join(name).exists())
        .collect();
    if existing.is_empty() {
        return true;
    }
    rfd::MessageDialog::new()
        .set_title("Replace existing files?")
        .set_description(format!(
            "{} already exist{} in {}:\n\n{}\n\nReplace them?",
            existing.len(),
            if existing.len() == 1 { "s" } else { "" },
            folder.display(),
            existing.join("\n")
        ))
        .set_buttons(rfd::MessageButtons::YesNo)
        .show()
        == rfd::MessageDialogResult::Yes
}

/// Pushes a toast and re-syncs the UI signal.
///
/// `super::mutate_ctx` rather than a bare `with_ctx_mut`, because the toast has to reach
/// the signal the notification stack renders from — a global-context write alone would sit
/// there unseen until something else happened to re-sync.
fn report_message(state: Signal<AppCtx>, message: String) {
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

    fn row(step_index: usize, file_name: &str) -> SaveRow {
        SaveRow {
            step_index,
            step_label: format!("Step {}", step_index + 1),
            cnc_name: "CNC".to_string(),
            line_count: 10,
            include: true,
            file_name: file_name.to_string(),
        }
    }

    /// Checked before the folder prompt, because a clash found afterwards would mean
    /// either losing the operator's choice of folder or writing half the batch.
    #[test]
    fn a_save_plan_must_name_every_included_step_uniquely() {
        assert_eq!(validate_rows(&[row(0, "a.nc"), row(1, "b.nc")]), None, "distinct names are fine");

        assert!(
            validate_rows(&[row(0, "same.nc"), row(1, "SAME.nc")])
                .unwrap_or_default()
                .contains("twice"),
            "names that differ only in case would collide on Windows"
        );
        assert!(validate_rows(&[row(0, "   ")]).unwrap_or_default().contains("file name"));
        assert!(validate_rows(&[]).unwrap_or_default().contains("at least one"));

        // A separator would write outside the folder the operator chose — the one thing
        // choosing a folder once is meant to guarantee.
        assert!(validate_rows(&[row(0, "sub/dir.nc")]).unwrap_or_default().contains("separator"));
        assert!(validate_rows(&[row(0, r"sub\dir.nc")]).unwrap_or_default().contains("separator"));
    }

    /// An excluded row is not the operator's problem: it is not written, so its name need
    /// not be valid or unique.
    #[test]
    fn an_excluded_step_is_not_validated() {
        let mut excluded = row(1, "same.nc");
        excluded.include = false;
        assert_eq!(validate_rows(&[row(0, "same.nc"), excluded]), None);
    }

    #[test]
    fn a_toast_names_the_file_not_the_whole_path() {
        // Built from components so the separator is the host's. `Path` treats `\` as one
        // only on Windows, so a hard-coded `E:\jobs\panel.nc` reads as a single long
        // filename on the Linux CI and the assertion fails there for no real reason.
        let nested: PathBuf = ["jobs", "panel.nc"].iter().collect();
        assert_eq!(file_name_of(&nested), "panel.nc");
        assert_eq!(file_name_of(Path::new("panel.nc")), "panel.nc");
        // The shape the app actually produces: an absolute path with a drive letter.
        #[cfg(windows)]
        assert_eq!(file_name_of(Path::new("E:\\jobs\\panel.nc")), "panel.nc");
    }
}

//! The Export control in the top bar: write the generated programs out, or write them to
//! a removable medium and eject that in one action.
//!
//! **Export, not Save.** These are program files leaving the application; "Save" is the
//! word for keeping the project, which is a different act on different data.
//!
//! It lives in the top bar rather than on the Job → Code tab because the program is the
//! thing the whole app produces, and having to navigate to one tab to get it made the
//! last step the least reachable. Being global also means the outcome cannot be shown
//! inline any more — it goes out as a toast, which is visible from wherever the user
//! happened to be.
//!
//! ## One dialog, and a destination that is already right
//!
//! There is a single path for one program and for ten: the dialog names them, shows where
//! they are going, and offers **Browse…** to change it. The forced folder picker it
//! replaced cost a click on every export to answer a question the previous export had
//! already answered — the destination is remembered per volume (see
//! `runtime::removable::choose_export_target`), so the machine's disk and each stick keep
//! their own and the offered folder is almost always the wanted one.
//!
//! ## The split button
//!
//! The second, narrow button appears **only when a removable medium is plugged in**. That
//! is deliberate: its appearance is itself the signal that the stick is ready, and a
//! permanently greyed USB button on a machine that never has one is noise. The main Export
//! button, by contrast, is always visible and merely disabled when there is no program —
//! a control that vanishes is a control the user cannot find again.

use std::fs;
use std::path::{Path, PathBuf};

use dioxus::prelude::*;

use crate::runtime::removable::{self, ExportDestination, RemovableMedium};
use crate::runtime::{with_ctx_mut, AppCtx};

/// The Export control: an ordinary export, plus an export-to-removable-media button when
/// there is somewhere to export to.
#[component]
pub fn ExportProgramButton(state: Signal<AppCtx>) -> Element {
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

    let mut rows = use_signal(Vec::<ExportRow>::new);
    let mut destination = use_signal(PathBuf::new);
    // `Some` only when the dialog was opened from the USB button — which is what decides
    // whether an eject follows, so it is the intent that is remembered rather than where
    // the files happen to end up.
    let mut eject_medium = use_signal(|| Option::<RemovableMedium>::None);
    let mut open = use_signal(|| false);

    // Read during render, which is sound only because the watcher bumps the shared UI
    // wake channel on every material change and `AppRoot` re-renders this subtree from
    // it — see `runtime::removable::removable_media`. There is no signal to subscribe to.
    let media = removable::removable_media();
    let stick = removable::export_medium(&media);

    // Opening is the same act either way; only which volume's remembered folder is
    // resolved, and whether an eject follows, differ.
    let mut open_export = move |wants: ExportDestination| {
        let snapshot = state.read().clone();
        let target = snapshot.export_target(&wants);
        rows.set(export_rows(&snapshot));
        destination.set(target.directory);
        eject_medium.set(target.medium);
        open.set(true);
    };

    rsx! {
        div { class: "topbar-save",
            button {
                class: "btn btn-primary",
                r#type: "button",
                disabled: !has_program,
                title: match (has_program, snapshot.has_any_program()) {
                    (true, _) => "Export the G-code program",
                    // Distinguish "nothing generated" from "what was generated no longer
                    // matches the job", which are different things to do something about.
                    (false, true) => "The job has changed — no current program to export",
                    (false, false) => "No program to export yet",
                },
                onclick: move |_| open_export(ExportDestination::Host),
                "Export…"
            }

            if let Some(stick) = stick {
                button {
                    class: "btn btn-primary topbar-save-media",
                    r#type: "button",
                    disabled: !has_program,
                    title: "Export to {stick.display_name()} and eject",
                    "aria-label": "Export to {stick.display_name()} and eject",
                    onclick: {
                        let stick = stick.clone();
                        move |_| open_export(ExportDestination::Medium(stick.clone()))
                    },
                    UsbIcon {}
                }
            }

            if *open.read() {
                ExportProgramsDialog {
                    rows,
                    destination,
                    medium_name: eject_medium.read().as_ref().map(|m| m.display_name()),
                    on_cancel: move |_| open.set(false),
                    on_export: move |_| {
                        open.set(false);
                        let rows = rows.read().clone();
                        let folder = destination.read().clone();
                        let eject = eject_medium.read().clone();
                        // Spawned, never called inline. `open.set(false)` above has just
                        // queued this dialog's unmount, and a blocking dialog here would
                        // let its modal pump re-enter the event loop and render that
                        // pending unmount while the arena is still borrowed for this very
                        // event — see `profiles_common::dialog_parent`.
                        spawn(async move { export_batch(state, rows, folder, eject).await });
                    },
                }
            }
        }
    }
}

/// A USB stick lying on its side: a body with the metal connector off one end.
///
/// It names the *destination*, not the action — the button it is attached to already says
/// "Export", so the pair reads "Export to [this]". An eject symbol would suggest a button
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

/// The export: what is being written, what each file is called, and where it is going.
///
/// One dialog for one program and for ten. It used to appear only for a multi-step job,
/// with a single program going straight to a native save dialog — two journeys to keep
/// working, and the single one had no say in the folder beyond navigating there again
/// every time.
///
/// Follows `ProfileNameDialog`'s shape (`wizard-overlay` / `wizard-dialog` /
/// `wizard-actions`, Escape to cancel) so it is the same dialog the rest of the app uses.
#[component]
fn ExportProgramsDialog(
    rows: Signal<Vec<ExportRow>>,
    destination: Signal<PathBuf>,
    /// `Some` when the export was aimed at a stick — the note that it will be ejected.
    medium_name: Option<String>,
    on_cancel: EventHandler<()>,
    on_export: EventHandler<()>,
) -> Element {
    let current = rows.read().clone();
    let folder = destination.read().clone();
    let problem = validate_export(&current, &folder);
    // One program needs no checkbox: it cannot be unticked and still exported, and
    // `validate_export` refuses an empty selection anyway.
    let single = current.len() == 1;

    // Built before the closure takes it: the eject note below needs `medium_name` too.
    let browse_title = match &medium_name {
        Some(name) => format!("Choose a folder on {name}"),
        None => "Choose a folder for the programs".to_string(),
    };
    let browse = move |_| {
        let title = browse_title.clone();
        let start = destination.read().clone();
        spawn(async move {
            // Async, and never the blocking variant — see `profiles_common::dialog_parent`
            // and the rule `dialog_safety_tests` enforces over this very file. Nothing is
            // written here: this only moves where Export will write.
            if let Some(picked) = rfd::AsyncFileDialog::new()
                .set_parent(&*super::profiles_common::dialog_parent())
                .set_title(title)
                .set_directory(&start)
                .pick_folder()
                .await
            {
                destination.set(picked.path().to_path_buf());
            }
        });
    };

    rsx! {
        div { class: "wizard-overlay",
            div {
                class: "wizard-dialog export-dialog",
                // Escape used to be bound to the name input alone, so with any other
                // control focused the documented "Escape cancels" was not true. On the
                // dialog it is, and `tabindex` is what lets this element receive the key.
                tabindex: "-1",
                onkeydown: move |evt| {
                    let key = evt.key().to_string().to_ascii_lowercase();
                    if key == "escape" || key == "esc" {
                        on_cancel.call(());
                    }
                },

                h2 { if single { "Export program" } else { "Export programs" } }
                p { class: "field-hint",
                    if single {
                        "Check the name and where it is going, then export."
                    } else {
                        "Each machining step is its own program. Name them here, tick the ones to write, then export."
                    }
                }

                div { class: "export-table",
                    for (position , row) in current.iter().enumerate() {
                        div {
                            key: "{row.step_index}",
                            class: if single { "export-row export-row-single" } else { "export-row" },
                            if !single {
                                input {
                                    r#type: "checkbox",
                                    checked: row.include,
                                    title: "Include this step",
                                    onchange: move |evt| {
                                        let checked = evt.checked();
                                        rows.write()[position].include = checked;
                                    },
                                }
                            }
                            div { class: "export-step",
                                span { class: "export-step-name", "{row.step_label}" }
                                span { class: "export-step-meta",
                                    if row.cnc_name.is_empty() {
                                        "{row.line_count} lines"
                                    } else {
                                        "{row.cnc_name} · {row.line_count} lines"
                                    }
                                }
                            }
                            input {
                                class: "export-name",
                                value: "{row.file_name}",
                                disabled: !row.include,
                                autofocus: position == 0,
                                oninput: move |evt| {
                                    let value = evt.value();
                                    rows.write()[position].file_name = value;
                                },
                            }
                        }
                    }
                }

                div { class: "export-destination",
                    span { class: "export-destination-label", "Folder" }
                    // Read-only rather than free text: a typed path that does not exist is
                    // a validation branch with nothing to gain, and Browse is the
                    // affordance. It still selects, copies and scrolls a long path.
                    input {
                        class: "export-destination-path",
                        readonly: true,
                        value: "{folder.display()}",
                    }
                    button {
                        class: "btn btn-secondary",
                        r#type: "button",
                        onclick: browse,
                        "Browse…"
                    }
                }

                if let Some(name) = medium_name.as_ref() {
                    p { class: "export-eject-note",
                        "{name} will be ejected when the export finishes."
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
                        onclick: move |_| on_export.call(()),
                        "Export"
                    }
                }
            }
        }
    }
}

/// Writes every ticked program into the chosen folder, then ejects if that was the point.
///
/// The folder arrives already decided — the dialog resolved and showed it, and Browse is
/// what changed it. Nothing is picked here.
async fn export_batch(
    state: Signal<AppCtx>,
    rows: Vec<ExportRow>,
    folder: PathBuf,
    eject: Option<RemovableMedium>,
) {
    // The only overwrite guard there is. A native save dialog used to supply one for the
    // single-program case; with no picker anywhere in the flow, this covers every export.
    if !confirm_overwrites(&rows, &folder).await {
        return;
    }

    let snapshot = state.read().clone();
    let report = write_rows(&snapshot, &rows, &folder);

    // Filed under the volume the files actually landed on, which Browse may have changed —
    // and read against the media list as it is *now*, so a folder on a stick is filed by
    // that stick's serial rather than by the letter it currently holds.
    let media = removable::removable_media();
    let volume = removable::volume_key_for_path(&folder, &media);
    with_ctx_mut(|ctx| ctx.app.remember_export_directory(&volume, &folder));

    if !report.failed.is_empty() {
        let (name, err) = &report.failed[0];
        report_message(
            state,
            format!(
                "Exported {} of {} — {name} failed: {err}",
                report.written.len(),
                report.written.len() + report.failed.len()
            ),
        );
    } else if let [only] = report.written.as_slice() {
        report_message(state, format!("Exported {only} to {}", folder.display()));
    } else {
        report_message(
            state,
            format!("Exported {} programs to {}", report.written.len(), folder.display()),
        );
    }

    if let Some(medium) = eject_after_export(eject.as_ref(), &report, &media, &folder) {
        // Fire-and-forget: the eject waits on a volume lock for up to several seconds, and
        // this is the WebView thread. The outcome arrives as its own toast from the watcher.
        report_message(state, format!("Ejecting {}…", medium.display_name()));
        removable::request_eject(medium);
    }
}

/// Whether, and what, to eject once the files have landed. Three conditions, all needed:
///
///  1. **The export was aimed at a stick** — the USB button. Browsing onto a stick from an
///     ordinary export does not eject it: an eject is something the operator asked for by
///     pressing that button, not something inferred from where a file ended up.
///  2. **The whole batch was written.** A partial write leaves the medium mounted so the
///     rest can be retried.
///  3. **The folder really is on a mounted medium**, re-read after the write. Browse lets
///     the operator navigate off the stick, and the stick may have been pulled while the
///     dialog was open — ejecting a drive the programs did not go to would be both useless
///     and alarming.
fn eject_after_export(
    aimed_at: Option<&RemovableMedium>,
    report: &ExportReport,
    media: &[RemovableMedium],
    folder: &Path,
) -> Option<RemovableMedium> {
    aimed_at?;
    if !report.all_written() {
        return None;
    }
    removable::medium_for_path(media, folder)
}

/// Write one program to disk, recording the fact.
///
/// The single choke point both the one-file and the whole-job save go through, so
/// there is exactly one place that can put a G-code file on disk and exactly one that
/// has to remember to record it.
///
/// Worth recording because this is where k2g's output leaves the application and
/// becomes something that can drive a machine — "which program went where, and when"
/// is the question an operator most often needs answered after the fact. Only the
/// file name and byte count are kept; the directory is redacted, and the program text
/// itself never goes near the record.
fn write_program(path: &Path, text: &str) -> std::io::Result<()> {
    use crate::runtime::security_log::{self, Event, Outcome};

    let result = fs::write(path, text.as_bytes());
    security_log::record(
        Event::GcodeWritten,
        if result.is_ok() { Outcome::Ok } else { Outcome::Failed },
        serde_json::json!({
            "path": security_log::redact(path),
            "bytes": text.len(),
            "error": result.as_ref().err().map(|e| e.to_string()),
        }),
    );
    result
}

/// One row of the pre-save plan: a step, the name its program will be written under, and
/// whether it is written at all.
///
/// The names are settled **before** the folder is chosen because `pick_folder` cannot ask
/// per file the way `save_file` does — with N programs there is one directory prompt and
/// no opportunity to name anything after it.
#[derive(Clone, PartialEq)]
pub(crate) struct ExportRow {
    pub step_index: usize,
    pub step_label: String,
    pub cnc_name: String,
    pub line_count: usize,
    pub include: bool,
    pub file_name: String,
}

/// Which steps may take their file name from their own step name: `true` where that name
/// belongs to exactly one step.
///
/// A step name is the operator's label for a setup and is under no obligation to be
/// unique — two steps called "Drill" are a perfectly reasonable profile. Naming files
/// after them would produce the same name twice, which `validate_export_rows` then refuses: a
/// clash the application invented and left the operator to fix in the dialog before it
/// would let them save.
///
/// So the colliding steps fall back to their ordinals, which are unique by construction,
/// and only they pay for it — a job with one "Drill" and one "Route" keeps both names.
///
/// Compared trimmed and case-folded, matching `validate_export_rows`, which is the check this
/// exists to stay ahead of — and matching Windows, where `Drill.nc` and `drill.nc` are
/// one file and the second write would silently replace the first.
///
/// Blank names collide with each other on the empty string and so all take their ordinal,
/// which is the same answer `program_file_name` would reach on its own.
fn names_belong_to_one_step_each(names: &[String]) -> Vec<bool> {
    let key = |name: &String| name.trim().to_lowercase();
    let mut uses: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for name in names {
        *uses.entry(key(name)).or_insert(0) += 1;
    }
    names.iter().map(|name| uses.get(&key(name)).copied().unwrap_or(0) == 1).collect()
}

/// The rows a multi-step save starts from: one per step that produced a program.
///
/// Named from the **step count**, not the number of programs: a three-step profile whose
/// middle step failed still writes `board_step3.nc`, because that ordinal is what tells
/// the operator which setup the file belongs to.
fn export_rows(snapshot: &AppCtx) -> Vec<ExportRow> {
    let step_count = snapshot.programs.len();
    let ready = snapshot.ready_programs();

    let names: Vec<String> = ready.iter().map(|(step, _)| step.name.clone()).collect();
    let usable = names_belong_to_one_step_each(&names);

    ready
        .into_iter()
        .enumerate()
        .map(|(position, (step, program))| {
            let unique = usable[position];
            ExportRow {
                step_index: step.index,
                step_label: if step.name.trim().is_empty() {
                    format!("Step {}", step.index + 1)
                } else {
                    format!("Step {}: {}", step.index + 1, step.name)
                },
                cnc_name: step.cnc_name.clone(),
                line_count: program.text.lines().count(),
                include: true,
                file_name: snapshot.program_file_name(
                    step.index,
                    step_count,
                    if unique { step.name.as_str() } else { "" },
                    &program.extension,
                ),
            }
        })
        .collect()
}

/// Why the export cannot run yet, or `None` when it can.
///
/// The destination is checked here rather than only at the write, so a stick pulled while
/// the dialog is open disables Export and says so, instead of failing every row.
pub(crate) fn validate_export(rows: &[ExportRow], folder: &Path) -> Option<String> {
    if folder.as_os_str().is_empty() {
        return Some("Choose a folder to export to.".to_string());
    }
    if !folder.is_dir() {
        return Some(format!("{} is no longer there — choose another folder.", folder.display()));
    }
    validate_export_rows(rows)
}

/// Why the rows cannot be written yet, or `None` when they can.
///
/// Checked before the write rather than during: discovering a clash halfway through means
/// some of the batch is on disk and the rest is not.
pub(crate) fn validate_export_rows(rows: &[ExportRow]) -> Option<String> {
    let included: Vec<&ExportRow> = rows.iter().filter(|row| row.include).collect();
    if included.is_empty() {
        return Some("Select at least one program to export.".to_string());
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
struct ExportReport {
    written: Vec<String>,
    failed: Vec<(String, String)>,
}

impl ExportReport {
    fn all_written(&self) -> bool {
        self.failed.is_empty() && !self.written.is_empty()
    }
}

/// Writes every included row into `folder`.
///
/// Successful writes are **not** rolled back when a later one fails: they are valid
/// programs, and deleting them would be the worse failure.
fn write_rows(snapshot: &AppCtx, rows: &[ExportRow], folder: &Path) -> ExportReport {
    let mut report = ExportReport { written: Vec::new(), failed: Vec::new() };
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
        match write_program(&path, &program.text) {
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
async fn confirm_overwrites(rows: &[ExportRow], folder: &Path) -> bool {
    let existing: Vec<String> = rows
        .iter()
        .filter(|row| row.include)
        .map(|row| row.file_name.trim().to_string())
        .filter(|name| folder.join(name).exists())
        .collect();
    if existing.is_empty() {
        return true;
    }
    rfd::AsyncMessageDialog::new()
        .set_parent(&*super::profiles_common::dialog_parent())
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
        .await
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

#[cfg(test)]
mod tests {
    use super::*;

    fn row(step_index: usize, file_name: &str) -> ExportRow {
        ExportRow {
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
        assert_eq!(validate_export_rows(&[row(0, "a.nc"), row(1, "b.nc")]), None, "distinct names are fine");

        assert!(
            validate_export_rows(&[row(0, "same.nc"), row(1, "SAME.nc")])
                .unwrap_or_default()
                .contains("twice"),
            "names that differ only in case would collide on Windows"
        );
        assert!(validate_export_rows(&[row(0, "   ")]).unwrap_or_default().contains("file name"));
        assert!(validate_export_rows(&[]).unwrap_or_default().contains("at least one"));

        // A separator would write outside the folder the operator chose — the one thing
        // choosing a folder once is meant to guarantee.
        assert!(validate_export_rows(&[row(0, "sub/dir.nc")]).unwrap_or_default().contains("separator"));
        assert!(validate_export_rows(&[row(0, r"sub\dir.nc")]).unwrap_or_default().contains("separator"));
    }

    /// An excluded row is not the operator's problem: it is not written, so its name need
    /// not be valid or unique.
    #[test]
    fn an_excluded_step_is_not_validated() {
        let mut excluded = row(1, "same.nc");
        excluded.include = false;
        assert_eq!(validate_export_rows(&[row(0, "same.nc"), excluded]), None);
    }

    /// The three conditions on ejecting, each of which has a way of being wrong.
    ///
    /// The first is the one worth stating: an eject is something the operator asked for by
    /// pressing the USB button, not something inferred from a file having landed on a
    /// stick. Browsing onto one from an ordinary export must not unmount it underneath
    /// them.
    #[test]
    fn ejecting_needs_the_usb_button_a_whole_batch_and_a_folder_on_that_medium() {
        use crate::runtime::removable::RemovableMedium;

        let stick = RemovableMedium {
            root: PathBuf::from("E:\\"),
            label: "KINGSTON".to_string(),
            drive_letter: 'E',
            serial: Some(0x1A2B_3C4D),
            free_bytes: 1 << 30,
        };
        let media = [stick.clone()];
        let whole = ExportReport { written: vec!["panel.nc".into()], failed: Vec::new() };
        let partial = ExportReport {
            written: vec!["panel.nc".into()],
            failed: vec![("back.nc".into(), "denied".into())],
        };
        let on_stick = Path::new("E:\\jobs");

        assert!(
            eject_after_export(Some(&stick), &whole, &media, on_stick).is_some(),
            "asked for, wrote everything, and the files are on it"
        );
        assert!(
            eject_after_export(None, &whole, &media, on_stick).is_none(),
            "browsed onto a stick from an ordinary export — nobody asked to unmount it"
        );
        assert!(
            eject_after_export(Some(&stick), &partial, &media, on_stick).is_none(),
            "a partial write leaves it mounted so the rest can be retried"
        );
        assert!(
            eject_after_export(Some(&stick), &whole, &media, Path::new("C:\\out")).is_none(),
            "aimed at the stick but browsed off it — the files are not there"
        );
        assert!(
            eject_after_export(Some(&stick), &whole, &[], on_stick).is_none(),
            "pulled while the dialog was open"
        );
    }

    /// The rule that keeps the generated plan saveable.
    ///
    /// `validate_export_rows` refuses a duplicate file name, so a profile with two steps called
    /// the same thing would have produced a plan the operator could not save without
    /// editing it first — a clash the application created.
    #[test]
    fn a_step_name_is_used_only_when_it_belongs_to_one_step() {
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();

        assert_eq!(
            names_belong_to_one_step_each(&names(&["Drill", "Route"])),
            [true, true],
            "distinct names are each the step's own"
        );
        assert_eq!(
            names_belong_to_one_step_each(&names(&["Drill", "Drill"])),
            [false, false],
            "both fall back; neither may claim the name"
        );
        assert_eq!(
            names_belong_to_one_step_each(&names(&["Drill", "Route", "Drill"])),
            [false, true, false],
            "only the clashing pair pays for it"
        );
    }

    /// Case and surrounding space must not create a clash the check misses. On Windows
    /// `Drill.nc` and `drill.nc` are one file, so the second write would replace the
    /// first — silently, since neither the plan nor the overwrite prompt would see two
    /// names as the same.
    #[test]
    fn the_clash_check_folds_case_and_space() {
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();

        assert_eq!(names_belong_to_one_step_each(&names(&["Drill", "drill"])), [false, false]);
        assert_eq!(names_belong_to_one_step_each(&names(&["Drill", " Drill "])), [false, false]);
    }

    /// Unnamed steps all collide on the empty string, so every one of them takes its
    /// ordinal — which is what `program_file_name` would have done anyway.
    #[test]
    fn blank_names_all_fall_back() {
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();

        assert_eq!(names_belong_to_one_step_each(&names(&["", "  ", "Route"])), [false, false, true]);
        assert_eq!(names_belong_to_one_step_each(&names(&[""])), [true], "one blank has no clash");
    }
}

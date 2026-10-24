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
    let mut sheet = use_signal(SheetRow::default);
    // How far the zero has moved since the last export of this board, worked out as the
    // dialog opens. See `AppState::zero_shift_since_export`.
    let mut zero_shift = use_signal(|| Option::<(f64, f64)>::None);
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
        sheet.set(SheetRow { include: true, file_name: snapshot.setup_sheet_file_name() });
        zero_shift.set(snapshot.zero_shift_since_export(crate::runtime::machining_plan::job_zero_offset(&snapshot)));
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
                    sheet,
                    zero_shift: *zero_shift.read(),
                    unit: snapshot.unit_system,
                    destination,
                    medium_name: eject_medium.read().as_ref().map(|m| m.display_name()),
                    on_cancel: move |_| open.set(false),
                    on_preview: move |_| {
                        let rows = rows.read().clone();
                        let sheet = sheet.read().clone();
                        // Spawned for the same reason the export is: the PDF takes a moment
                        // to build, and nothing about it belongs inside this event.
                        spawn(async move { preview_sheet(state, &rows, &sheet) });
                    },
                    on_export: move |_| {
                        open.set(false);
                        let rows = rows.read().clone();
                        let sheet = sheet.read().clone();
                        let folder = destination.read().clone();
                        let eject = eject_medium.read().clone();
                        // Spawned, never called inline. `open.set(false)` above has just
                        // queued this dialog's unmount, and a blocking dialog here would
                        // let its modal pump re-enter the event loop and render that
                        // pending unmount while the arena is still borrowed for this very
                        // event — see `profiles_common::dialog_parent`.
                        spawn(async move { export_batch(state, rows, sheet, folder, eject).await });
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

/// An eye: look at it, without keeping it.
#[component]
fn PreviewIcon() -> Element {
    rsx! {
        svg {
            class: "export-preview-svg",
            view_box: "0 0 24 24",
            "aria-hidden": "true",
            path { d: "M2 12s3.6-6.5 10-6.5S22 12 22 12s-3.6 6.5-10 6.5S2 12 2 12z" }
            circle { cx: "12", cy: "12", r: "3" }
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
    sheet: Signal<SheetRow>,
    /// How far the zero has moved since this board was last exported, if it has.
    zero_shift: Option<(f64, f64)>,
    unit: units::UserUnitSystem,
    destination: Signal<PathBuf>,
    /// `Some` when the export was aimed at a stick — the note that it will be ejected.
    medium_name: Option<String>,
    on_cancel: EventHandler<()>,
    /// Open the setup sheet as it would be written now, without writing it anywhere.
    on_preview: EventHandler<()>,
    on_export: EventHandler<()>,
) -> Element {
    let current = rows.read().clone();
    let current_sheet = sheet.read().clone();
    let folder = destination.read().clone();
    let problem = validate_export(&current, &current_sheet, &folder);
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
                            class: "export-row",
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
                            } else {
                                // Holds the checkbox's column, so the name lines up with the
                                // setup sheet's row below, which always has one.
                                span { class: "export-check-gap" }
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
                    // The setup sheet: always a checkbox, even beside a single program,
                    // because leaving it out is a real choice where leaving out the only
                    // program is not.
                    div { class: "export-row export-row-sheet",
                        input {
                            r#type: "checkbox",
                            checked: current_sheet.include,
                            title: "Include the setup sheet",
                            onchange: move |evt| sheet.write().include = evt.checked(),
                        }
                        div { class: "export-step",
                            span { class: "export-step-name", "Setup sheet (PDF)" }
                            span { class: "export-step-meta", "The board, every step's setup, rack and tools" }
                        }
                        // The name and the preview share the name column, so this row's name
                        // box starts where every program's does.
                        div { class: "export-name-with-action",
                            input {
                                class: "export-name",
                                value: "{current_sheet.file_name}",
                                disabled: !current_sheet.include,
                                oninput: move |evt| sheet.write().file_name = evt.value(),
                            }
                            // Enabled whether or not the sheet is ticked: looking at it is
                            // how the operator decides whether to include it.
                            button {
                                r#type: "button",
                                class: "icon-btn export-preview-btn",
                                title: "Preview the setup sheet",
                                "aria-label": "Preview the setup sheet",
                                onclick: move |_| on_preview.call(()),
                                PreviewIcon {}
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

                // A warning, not a refusal: the new zero is right for the job as it now
                // stands, and wrong only for work already done with the old one — which
                // only the operator knows about.
                if let Some(text) = zero_shift.map(|shift| zero_shift_warning(shift, unit)) {
                    p { class: "diag-status diag-warning", "{text}" }
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
    sheet: SheetRow,
    folder: PathBuf,
    eject: Option<RemovableMedium>,
) {
    // The only overwrite guard there is. A native save dialog used to supply one for the
    // single-program case; with no picker anywhere in the flow, this covers every export.
    if !confirm_overwrites(&rows, &sheet, &folder).await {
        return;
    }

    let snapshot = state.read().clone();
    let mut report = write_rows(&snapshot, &rows, &folder);
    // After the programs, from the same snapshot, naming the files as they were written.
    if sheet.include {
        report.sheet = Some(write_sheet(&snapshot, &rows, &sheet, &folder));
    }

    // Filed under the volume the files actually landed on, which Browse may have changed —
    // and read against the media list as it is *now*, so a folder on a stick is filed by
    // that stick's serial rather than by the letter it currently holds.
    let media = removable::removable_media();
    let volume = removable::volume_key_for_path(&folder, &media);
    with_ctx_mut(|ctx| ctx.app.remember_export_directory(&volume, &folder));
    // What these programs were written against, so the next export can tell if it moves.
    if !report.written.is_empty() {
        let offset = crate::runtime::machining_plan::job_zero_offset(&snapshot);
        with_ctx_mut(|ctx| ctx.app.remember_export_zero(offset));
    }

    // The sheet is said separately: it is not a program, and a sheet that failed must not
    // read as a program that did.
    let with_sheet = match &report.sheet {
        Some(Ok(_)) => " and the setup sheet",
        _ => "",
    };
    if !report.failed.is_empty() {
        let (name, err) = &report.failed[0];
        report_message(
            state,
            format!(
                "Exported {} of {}{with_sheet} — {name} failed: {err}",
                report.written.len(),
                report.written.len() + report.failed.len()
            ),
        );
    } else if let [only] = report.written.as_slice() {
        report_message(state, format!("Exported {only}{with_sheet} to {}", folder.display()));
    } else {
        report_message(
            state,
            format!("Exported {} programs{with_sheet} to {}", report.written.len(), folder.display()),
        );
    }
    if let Some(Err(err)) = &report.sheet {
        report_message(state, format!("The setup sheet was not written: {err}"));
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

/// What the Export dialog says when the work zero has moved since the last export.
fn zero_shift_warning((dx, dy): (f64, f64), unit: units::UserUnitSystem) -> String {
    let len = |mm: f64| units::user_format::format_length_display(units::Length::from_mm(mm.abs()), unit);
    let mut moves = Vec::new();
    if dx.abs() > 1e-3 {
        moves.push(format!("{} in X", len(dx)));
    }
    if dy.abs() > 1e-3 {
        moves.push(format!("{} in Y", len(dy)));
    }
    format!(
        "The work zero has moved {} since this board was last exported, because the steps \
         changed. These programs are right for the job as it is now, but they will not line \
         up with a board already cut using the earlier ones.",
        moves.join(" and "),
    )
}

/// The setup sheet's row in the export: whether it is written, and as what.
#[derive(Clone, Default, PartialEq)]
pub(crate) struct SheetRow {
    pub include: bool,
    pub file_name: String,
}

/// The rows a multi-step save starts from: one per step that produced a program.
///
/// Named from the **step count**, not the number of programs: a three-step profile whose
/// middle step failed still writes `board_step3.nc`, because that ordinal is what tells
/// the operator which setup the file belongs to.
fn export_rows(snapshot: &AppCtx) -> Vec<ExportRow> {
    let step_count = snapshot.programs.len();
    let ready = snapshot.ready_programs();

    ready
        .into_iter()
        .map(|(step, program)| {
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
                    &step.name,
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
pub(crate) fn validate_export(rows: &[ExportRow], sheet: &SheetRow, folder: &Path) -> Option<String> {
    if folder.as_os_str().is_empty() {
        return Some("Choose a folder to export to.".to_string());
    }
    if !folder.is_dir() {
        return Some(format!("{} is no longer there — choose another folder.", folder.display()));
    }
    validate_export_rows(rows).or_else(|| validate_sheet(rows, sheet))
}

/// Why the setup sheet cannot be written as named, or `None` when it can.
///
/// The same rules as a program's name, plus one: it must not be any program's name too,
/// or one file would silently replace the other.
pub(crate) fn validate_sheet(rows: &[ExportRow], sheet: &SheetRow) -> Option<String> {
    if !sheet.include {
        return None;
    }
    let name = sheet.file_name.trim();
    if name.is_empty() {
        return Some("The setup sheet needs a file name.".to_string());
    }
    if name.contains('/') || name.contains('\\') {
        return Some(format!("'{name}' cannot contain a path separator."));
    }
    let clashes = rows
        .iter()
        .filter(|row| row.include)
        .any(|row| row.file_name.trim().eq_ignore_ascii_case(name));
    clashes.then(|| format!("'{name}' is a program's name too — give the setup sheet its own."))
}

/// The file each included step is being written as, by step index — what the setup sheet
/// lists, so it names the files that are actually on the stick.
fn program_files(rows: &[ExportRow]) -> std::collections::BTreeMap<usize, String> {
    rows.iter()
        .filter(|row| row.include)
        .map(|row| (row.step_index, row.file_name.trim().to_string()))
        .collect()
}

/// Builds and writes the setup sheet. `Ok` holds the file name written.
fn write_sheet(snapshot: &AppCtx, rows: &[ExportRow], sheet: &SheetRow, folder: &Path) -> Result<String, String> {
    let bytes = crate::report::setup_sheet_pdf(snapshot, &program_files(rows))?;
    let name = sheet.file_name.trim().to_string();
    let path = folder.join(&name);
    fs::write(&path, bytes).map_err(|err| {
        log::error!("could not write {}: {err}", path.display());
        err.to_string()
    })?;
    Ok(name)
}

/// Builds the setup sheet as the export would write it now and opens it in the system's
/// PDF viewer, writing nothing to the export folder.
///
/// Handed to the OS rather than shown in the app because the app is a WebView, and not
/// every WebView has a PDF viewer: WebView2 on Windows does, WebKitGTK on Linux does not.
/// The system viewer is the one place every platform can show it, and it can print it
/// too — which is often why the operator wanted to look.
///
/// It names the program files the dialog currently shows, so what is previewed is what
/// would be exported.
fn preview_sheet(state: Signal<AppCtx>, rows: &[ExportRow], sheet: &SheetRow) {
    let snapshot = state.read().clone();
    let opened = write_preview(&snapshot, rows, sheet)
        .and_then(|path| open::that_detached(&path).map_err(|err| err.to_string()));
    if let Err(err) = opened {
        report_message(state, format!("The setup sheet could not be previewed: {err}"));
    }
}

/// Writes the preview PDF into k2g's own folder under the system temp directory and returns
/// its path.
///
/// Named after the sheet's file name, so the viewer's title bar says which sheet it is. A
/// viewer may still have the last preview open — and on Windows an open file cannot be
/// overwritten — so a refused write moves on to a numbered name instead of failing the
/// preview. The temp directory is the OS's to clear; nothing here is meant to last.
fn write_preview(snapshot: &AppCtx, rows: &[ExportRow], sheet: &SheetRow) -> Result<PathBuf, String> {
    let bytes = crate::report::setup_sheet_pdf(snapshot, &program_files(rows))?;
    let dir = std::env::temp_dir().join("k2g-preview");
    fs::create_dir_all(&dir).map_err(|err| err.to_string())?;

    let stem = preview_stem(&sheet.file_name);
    let mut last_error = String::new();
    for n in 0..10 {
        let path = if n == 0 { dir.join(format!("{stem}.pdf")) } else { dir.join(format!("{stem}-{n}.pdf")) };
        match fs::write(&path, &bytes) {
            Ok(()) => return Ok(path),
            Err(err) => last_error = err.to_string(),
        }
    }
    Err(last_error)
}

/// The preview's file name without its extension: the sheet's own name, or a plain one when
/// that is blank or would reach outside the preview folder.
fn preview_stem(file_name: &str) -> String {
    let name = file_name.trim();
    let name = name.strip_suffix(".pdf").or_else(|| name.strip_suffix(".PDF")).unwrap_or(name);
    if name.is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." {
        "setup-sheet".to_string()
    } else {
        name.to_string()
    }
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
    /// The setup sheet's outcome, when it was asked for.
    sheet: Option<Result<String, String>>,
}

impl ExportReport {
    /// Everything asked for is on disk — the sheet included, so a stick is not ejected with
    /// the programs on it and the sheet that describes them missing.
    fn all_written(&self) -> bool {
        self.failed.is_empty() && !self.written.is_empty() && !matches!(self.sheet, Some(Err(_)))
    }
}

/// Writes every included row into `folder`.
///
/// Successful writes are **not** rolled back when a later one fails: they are valid
/// programs, and deleting them would be the worse failure.
fn write_rows(snapshot: &AppCtx, rows: &[ExportRow], folder: &Path) -> ExportReport {
    let mut report = ExportReport { written: Vec::new(), failed: Vec::new(), sheet: None };
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
async fn confirm_overwrites(rows: &[ExportRow], sheet: &SheetRow, folder: &Path) -> bool {
    let existing: Vec<String> = rows
        .iter()
        .filter(|row| row.include)
        .map(|row| row.file_name.trim().to_string())
        .chain(sheet.include.then(|| sheet.file_name.trim().to_string()))
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

    /// The preview is named after the sheet, so the viewer's title says which one it is —
    /// but never with a name that would leave the preview folder, or none at all.
    #[test]
    fn the_preview_is_named_after_the_sheet_and_stays_in_its_folder() {
        assert_eq!(preview_stem("panel-setup.pdf"), "panel-setup");
        assert_eq!(preview_stem("  panel-setup  "), "panel-setup");
        assert_eq!(preview_stem(""), "setup-sheet");
        assert_eq!(preview_stem("../evil.pdf"), "setup-sheet");
        assert_eq!(preview_stem("a\\b.pdf"), "setup-sheet");
        assert_eq!(preview_stem(".."), "setup-sheet");
    }

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
        let whole = ExportReport { written: vec!["panel.nc".into()], failed: Vec::new(), sheet: None };
        let partial = ExportReport {
            written: vec!["panel.nc".into()],
            failed: vec![("back.nc".into(), "denied".into())],
            sheet: None,
        };
        let sheet_failed = ExportReport {
            written: vec!["panel.nc".into()],
            failed: Vec::new(),
            sheet: Some(Err("denied".into())),
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
        assert!(
            eject_after_export(Some(&stick), &sheet_failed, &media, on_stick).is_none(),
            "the setup sheet did not land, so the stick stays mounted for a retry"
        );
    }

    /// The setup sheet is named like a program, and never the same as one: the second
    /// write would silently replace the first.
    #[test]
    fn the_setup_sheet_needs_a_name_of_its_own() {
        let rows = vec![row(0, "panel.nc")];
        let sheet = |name: &str| SheetRow { include: true, file_name: name.into() };

        assert_eq!(validate_sheet(&rows, &sheet("panel-setup.pdf")), None);
        assert!(validate_sheet(&rows, &sheet("  ")).is_some(), "a blank name");
        assert!(validate_sheet(&rows, &sheet("out/sheet.pdf")).is_some(), "a path separator");
        assert!(validate_sheet(&rows, &sheet("PANEL.NC")).is_some(), "a program's name, in any case");
        assert_eq!(
            validate_sheet(&rows, &SheetRow { include: false, file_name: String::new() }),
            None,
            "an unticked sheet is not checked"
        );
    }

    /// The sheet lists the programs under the names they are being written as, and only the
    /// ones being written.
    #[test]
    fn the_setup_sheet_names_the_files_being_written() {
        let mut excluded = row(1, "panel_back.nc");
        excluded.include = false;
        let files = program_files(&[row(0, " panel_front.nc "), excluded]);
        assert_eq!(files.get(&0).map(String::as_str), Some("panel_front.nc"), "trimmed, as written");
        assert!(!files.contains_key(&1), "a step left out of the export is not named");
    }

    /// The warning says how far, and on which axes — only the ones that moved.
    #[test]
    fn the_zero_shift_warning_names_the_axes_that_moved() {
        let text = zero_shift_warning((1.719, 0.0), units::UserUnitSystem::Metric);
        assert!(text.contains("in X"), "{text}");
        assert!(!text.contains("in Y"), "Y did not move: {text}");
        let both = zero_shift_warning((-0.5, 0.25), units::UserUnitSystem::Metric);
        assert!(both.contains("in X and") && both.contains("in Y"), "{both}");
    }
}

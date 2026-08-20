use dioxus::prelude::*;
use std::collections::BTreeSet;
use std::fs;
use uuid::Uuid;

use super::profiles_common::{
    slug_file_name, suggested_profile_name, ProfileLifecycleToolbar, ProfileNameDialog,
};
use crate::data::Profile;
use crate::ui::bindings::{
    add_step, clone_named, create_named, export_yaml, import_yaml, machining_operations, move_step,
    remove_profile_result, remove_step, use_conflicting_operations, use_field,
    use_job_machining_profile, use_operations, use_profiles, use_step_count, BindingPicker,
    OperationsEditor, SchemaField, SchemaForm,
};

/// Machining ("process") profile screen, fully backed by the `AppData` datastore.
///
/// A machining profile is mostly references (cnc/fixture/toolset bindings) plus an
/// operation set and per-operation configuration. The detail editor is generated
/// from `machining.yaml`: the deep per-operation config renders through
/// [`SchemaForm`]; only the reference bindings and the operation toggles use
/// dedicated pickers. AppData owns the `processing_profiles` files; the legacy generator
/// still reads the in-memory `process_profiles`, mirrored from AppData by the root's
/// single bridge ([`crate::ui::bindings::refresh_legacy_projections`]). Deletion is by
/// simple reference guard — a referenced cnc/fixture/toolset blocks its own deletion, so
/// profiles are removed leaf-first; nothing references a machining profile, so it deletes
/// freely.
#[component]
pub fn MachiningProfilesScreen(state: Signal<crate::runtime::AppCtx>) -> Element {
    let mut status_message = use_signal(String::new);
    let mut show_name_dialog = use_signal(|| false);
    let mut dialog_is_clone = use_signal(|| false);
    let mut dialog_name = use_signal(|| "My machining".to_string());
    let mut selected = use_signal(|| None::<Uuid>);

    let profiles = use_profiles(Profile::Machining);

    let current = profile_on_show(*selected.read(), use_job_machining_profile(), &profiles);
    let current_name = current
        .and_then(|id| profiles.iter().find(|(pid, _)| *pid == id).map(|(_, n)| n.clone()));
    let toolbar_profiles = profiles
        .iter()
        .map(|(id, name)| (id.to_string(), name.clone()))
        .collect::<Vec<_>>();
    let existing_names = profiles.iter().map(|(_, name)| name.clone()).collect::<Vec<_>>();

    rsx! {
        div { class: "screen single stock-shell",
            div { class: "stock-toolbar",
                div {
                    h3 { "Machining profile management" }
                    p {
                        "A machining profile defines a job context: which CNC, fixture and toolset to use, and which operations to run."
                    }
                }
                ProfileLifecycleToolbar {
                    profile_type_label: "Machining".to_string(),
                    profiles: toolbar_profiles,
                    selected_profile_id: current.map(|id| id.to_string()),
                    can_export: current.is_some(),
                    on_select: move |id: String| selected.set(Uuid::parse_str(&id).ok()),
                    on_add: {
                        let existing = existing_names.clone();
                        move |_| {
                            dialog_is_clone.set(false);
                            dialog_name.set(suggested_profile_name("Machining", &existing));
                            show_name_dialog.set(true);
                        }
                    },
                    on_clone: {
                        let current_name = current_name.clone();
                        move |_| {
                            if current.is_none() {
                                status_message.set("No profile selected".to_string());
                                return;
                            }
                            dialog_is_clone.set(true);
                            dialog_name.set(format!("Copy of {}", current_name.clone().unwrap_or_default()));
                            show_name_dialog.set(true);
                        }
                    },
                    on_delete: move |_| {
                        let Some(id) = current else {
                            status_message.set("No profile selected".to_string());
                            return;
                        };
                        spawn(async move {
                            if super::profiles_common::confirm("Delete machining profile", "Delete this machining profile?").await {
                                match remove_profile_result(id) {
                                    Ok(()) => {
                                        selected.set(None);
                                        status_message.set("Machining profile deleted".to_string());
                                    }
                                    Err(message) => status_message.set(message),
                                }
                            }
                        });
                    },
                    on_export: {
                        let current_name = current_name.clone();
                        move |_| {
                            let Some(id) = current else {
                                status_message.set("No profile selected".to_string());
                                return;
                            };
                            let name = current_name.clone().unwrap_or_else(|| "machining-profile".to_string());
                            let default_name = format!(
                                "{}.machining-profile.yaml",
                                slug_file_name(&name, "machining-profile"),
                            );
                            spawn(async move {
                                let Some(path) = super::profiles_common::pick_export_file(
                                    "Export machining profile",
                                    "Machining profile YAML",
                                    &default_name,
                                )
                                .await
                                else {
                                    return;
                                };
                                match export_yaml(id) {
                                    Some(yaml) => {
                                        if fs::write(&path, yaml).is_ok() {
                                            status_message.set("Machining profile exported".to_string());
                                        } else {
                                            status_message.set("Export failed: unable to write file".to_string());
                                        }
                                    }
                                    None => status_message.set("Export failed".to_string()),
                                }
                            });
                        }
                    },
                    on_import: move |_| {
                        spawn(async move {
                            let Some(path) =
                                super::profiles_common::pick_import_file("Import machining profile", "Machining profile YAML")
                                    .await
                            else {
                                return;
                            };
                            let text = match fs::read_to_string(&path) {
                                Ok(text) => text,
                                Err(_) => {
                                    status_message.set("Import failed: file not readable".to_string());
                                    return;
                                }
                            };
                            match import_yaml(Profile::Machining, &text) {
                                Some(id) => {
                                    selected.set(Some(id));
                                    status_message.set("Machining profile imported and selected".to_string());
                                }
                                None => status_message.set("Import failed: invalid profile".to_string()),
                            }
                            });
                    },
                }
            }

            if !status_message.read().is_empty() {
                p { class: "diag-status", "{status_message}" }
            }

            if let Some(id) = current {
                MachiningDetail { id }
            } else {
                div { class: "panel stock-detail-panel profile-editor-shell",
                    p { class: "diag-status", "Select or add a machining profile to edit details." }
                }
            }

            if *show_name_dialog.read() {
                ProfileNameDialog {
                    title: if *dialog_is_clone.read() { "Clone machining profile".to_string() } else { "Add machining profile".to_string() },
                    name_label: "Profile name".to_string(),
                    name_value: dialog_name.read().clone(),
                    template_options: Vec::<(String, String)>::new(),
                    selected_template: String::new(),
                    on_name_change: move |value| dialog_name.set(value),
                    on_template_change: |_| {},
                    on_cancel: move |_| show_name_dialog.set(false),
                    on_submit: move |_| {
                        let name = dialog_name.read().trim().to_string();
                        if name.is_empty() {
                            status_message.set("Profile name is required".to_string());
                            return;
                        }
                        let is_clone = *dialog_is_clone.read();
                        let result = if is_clone {
                            current.and_then(|id| clone_named(id, &name))
                        } else {
                            create_named(Profile::Machining, &name)
                        };
                        match result {
                            Some(id) => {
                                selected.set(Some(id));
                                show_name_dialog.set(false);
                                status_message.set(
                                    if is_clone { "Profile cloned".to_string() } else { "Profile created".to_string() },
                                );
                            }
                            None => status_message.set("Operation failed".to_string()),
                        }
                    },
                }
            }
        }
    }
}

/// Which profile the screen shows: what the operator picked here, else the one the live
/// job runs, else the first there is.
///
/// The job's profile comes before the head of the list because switching to this screen
/// is nearly always "let me look at what the job runs". Opening on an unrelated profile
/// merely because it sorts first is not only wrong but quietly dangerous — the toolbar's
/// Delete, Clone and Export all act on whatever happens to be showing.
///
/// `picked` is the local override and always wins, so choosing a profile here is not
/// undone on the next render. It resets on its own: the screen is unmounted whenever the
/// view changes, so a return to this screen asks the job again.
///
/// `job` is filtered against `profiles` rather than trusted — a job left pointing at a
/// since-deleted profile must fall through to a real one, not blank the editor.
fn profile_on_show(
    picked: Option<Uuid>,
    job: Option<Uuid>,
    profiles: &[(Uuid, String)],
) -> Option<Uuid> {
    let known = |id: Uuid| profiles.iter().any(|(pid, _)| *pid == id);
    picked
        .filter(|id| known(*id))
        .or_else(|| job.filter(|id| known(*id)))
        .or_else(|| profiles.first().map(|(id, _)| *id))
}

/// The machining detail editor: identity, then the ordered list of steps. Each
/// step is one machining setup (its own cnc/fixture/toolset + operations); the
/// list can grow, shrink and reorder.
#[component]
fn MachiningDetail(id: Uuid) -> Element {
    let step_count = use_step_count(id);
    let conflicts = use_conflicting_operations(id);

    // Which steps are folded shut, by index.
    //
    // Held here rather than per card because collapsing is not always the card's own
    // doing: adding a step folds the one before it, and removing or reordering has to
    // carry the state with the steps that moved. A step has no identity of its own in
    // the document — it is an array entry — so the index is the only handle there is,
    // and every structural edit remaps this set to match (see [`StepCard`]).
    //
    // Deliberately not persisted: it is a view of the moment, and a profile reopened
    // later should show its steps, not the shape of an editing session from last week.
    let mut collapsed = use_signal(BTreeSet::<usize>::new);

    rsx! {
        div { class: "panel stock-detail-panel cnc-profile-details-panel profile-editor-shell",
            div { class: "profile-editor-scroll",
                div { class: "edit-grid",
                    SchemaField { id, ptr: "/name".to_string() }

                    // The editor cannot produce these — the offending checkbox is
                    // disabled — so a conflict here came in from a hand-edited or
                    // imported file. Stated as a fault rather than asked about ("is that
                    // intended?"), because it is one: the job will refuse to generate
                    // until it is resolved.
                    for conflict in conflicts.iter() {
                        p { class: "diag-status diag-error", "{conflict.message()}" }
                    }

                    // A profile with one step is just a profile: nothing here says
                    // "step", and the single card renders as a plain section, so the
                    // screen reads like the CNC and Fixture editors. The word returns
                    // the moment a second step exists.
                    if step_count > 1 {
                        h4 { class: "section-title", "Machining steps" }
                        p { class: "field-hint",
                            "Each step is one physical setup with its own CNC, fixture, toolset and operations. Steps run in order."
                        }
                    }

                    for index in 0..step_count {
                        StepCard { key: "{index}", id, index, step_count, collapsed }
                    }

                    button {
                        r#type: "button",
                        class: "add-step-btn",
                        onclick: move |_| {
                            // Fold the step that was last, so the new card lands where the
                            // operator is looking. A profile is built by adding a step and
                            // filling it in, and by the fourth one the form to fill in was
                            // below a screenful of finished work.
                            //
                            // Only the immediately preceding step: the ones above it are
                            // left as the operator chose to leave them.
                            if step_count > 0 {
                                collapsed.write().insert(step_count - 1);
                            }
                            add_step(id);
                        },
                        "+ Add step"
                    }
                }
            }
        }
    }
}

/// Where each folded index lands when the step at `from` is spliced in at `to`.
///
/// Folded state is keyed by index because a step has no identity of its own — it is an
/// entry in an array. So every structural edit has to remap it, or the flags stay behind
/// on the *positions* and the wrong cards come back folded. `move_step` is a splice
/// (remove then insert), not a swap, so this mirrors that exactly rather than assuming
/// the adjacent case the buttons happen to use.
fn remap_indexes(folded: &BTreeSet<usize>, from: usize, to: usize) -> BTreeSet<usize> {
    if from == to {
        return folded.clone();
    }
    folded
        .iter()
        .map(|&at| {
            if at == from {
                to
            } else if from < at && at <= to {
                at - 1
            } else if to <= at && at < from {
                at + 1
            } else {
                at
            }
        })
        .collect()
}

/// The folded set with `removed` dropped and the gap behind it closed.
///
/// Without the shift, deleting step 2 would leave step 3's flag on what is now step 3's
/// successor — a card folding itself for no reason the operator can see.
fn drop_index(folded: &BTreeSet<usize>, removed: usize) -> BTreeSet<usize> {
    folded
        .iter()
        .filter(|&&at| at != removed)
        .map(|&at| if at > removed { at - 1 } else { at })
        .collect()
}

/// [`remap_indexes`], applied to the signal the cards share.
fn remap_folded(mut collapsed: Signal<BTreeSet<usize>>, from: usize, to: usize) {
    collapsed.with_mut(|folded| *folded = remap_indexes(folded, from, to));
}

/// [`drop_index`], applied to the signal the cards share.
fn drop_folded(mut collapsed: Signal<BTreeSet<usize>>, removed: usize) {
    collapsed.with_mut(|folded| *folded = drop_index(folded, removed));
}

/// One machining step card: identity + reference bindings + operation set, then
/// schema-generated configuration for routing and each enabled operation, plus
/// reorder/remove controls.
#[component]
fn StepCard(
    id: Uuid,
    index: usize,
    step_count: usize,
    collapsed: Signal<BTreeSet<usize>>,
) -> Element {
    let enabled_ops = use_operations(id, index);
    // One component with conditional chrome rather than two: duplicating the field list
    // for the single-step case is how the two would drift apart.
    let multi = step_count > 1;
    let drills_pins = enabled_ops.iter().any(|op| op == "drill_locating_pins");

    // Folding is a statement about a step among steps, like the heading and the reorder
    // controls. A lone step has no collapse control, so it must never render folded —
    // there would be nothing to click to get it back.
    let is_folded = multi && collapsed.read().contains(&index);

    // Which of this step's operation sections are folded, by operation key.
    //
    // Local to the card, and deliberately not persisted: it is a reading position, not a
    // preference — the operator folds what they are not editing right now, and next
    // session they are editing something else. The step's own fold state lives with the
    // parent because reordering has to move it; this does not, because the sections
    // reorder with the card that owns them.
    let mut folded_ops = use_signal(BTreeSet::<String>::new);

    // Read unconditionally, used only when folded. `use_field` allocates no hook slot
    // today — it subscribes by reading a global signal — so a conditional call happens to
    // be safe, but reading it as one invites the reader to conclude that hooks may be
    // called conditionally here, which is exactly the belief that breaks the next one.
    //
    // Shown beside the number only when folded: expanded, the name field is the first
    // thing under the header and repeating it is noise; folded, "Step 2" over a closed
    // card says nothing about which setup it is.
    //
    // The *displayed* name, so a step the operator has not named reads as what it does
    // ("PTH + NPTH") rather than as the placeholder every step starts with. The name
    // *field* below still shows the stored value — it has to, or it could not be typed in.
    let step_name = crate::data::model::step_display_name(
        &use_field(id, &format!("/steps/{index}/name"))
            .map(|field| field.display)
            .unwrap_or_default(),
        &enabled_ops,
    );
    let folded_name = is_folded.then_some(step_name);

    rsx! {
        div { class: if multi { "schema-section step-card" } else { "schema-section" },
            // The heading and the reorder/remove controls are all statements about a step
            // among steps. With one step every one of them is inert (the remove button is
            // already disabled), so they are absent rather than greyed.
            if multi {
                div { class: "step-card-header",
                    div { class: "step-card-title",
                        button {
                            r#type: "button",
                            class: "icon-btn",
                            title: if is_folded { "Expand step" } else { "Collapse step" },
                            "aria-expanded": if is_folded { "false" } else { "true" },
                            onclick: move |_| {
                                collapsed
                                    .with_mut(|folded| {
                                        if !folded.remove(&index) {
                                            folded.insert(index);
                                        }
                                    });
                            },
                            if is_folded { "▸" } else { "▾" }
                        }
                        h4 { class: "section-title", "Step {index + 1}" }
                        if let Some(name) = folded_name {
                            span { class: "step-card-folded-name", "{name}" }
                        }
                    }
                    div { class: "step-card-actions",
                        button {
                            r#type: "button", class: "icon-btn", disabled: index == 0,
                            title: "Move step up",
                            onclick: move |_| {
                                remap_folded(collapsed, index, index.saturating_sub(1));
                                move_step(id, index, index.saturating_sub(1));
                            },
                            "↑"
                        }
                        button {
                            r#type: "button", class: "icon-btn", disabled: index + 1 >= step_count,
                            title: "Move step down",
                            onclick: move |_| {
                                remap_folded(collapsed, index, index + 1);
                                move_step(id, index, index + 1);
                            },
                            "↓"
                        }
                        button {
                            r#type: "button", class: "icon-btn icon-btn-danger", disabled: step_count <= 1,
                            title: "Remove step",
                            onclick: move |_| {
                                drop_folded(collapsed, index);
                                remove_step(id, index);
                            },
                            "✕"
                        }
                    }
                }
            }

            if !is_folded {
                // A step's name distinguishes it from its siblings; with no siblings there
                // is nothing to distinguish. Still stored, so adding a second step later
                // starts from whatever this one was called.
                if multi {
                    SchemaField { id, ptr: format!("/steps/{index}/name") }
                }

                BindingPicker { id, step: index, field: "cnc".to_string(), kind: Profile::Cnc, label: "CNC profile".to_string() }
                BindingPicker { id, step: index, field: "fixture".to_string(), kind: Profile::Fixture, label: "Fixture profile".to_string() }
                BindingPicker { id, step: index, field: "toolset".to_string(), kind: Profile::Toolset, label: "Toolset profile".to_string() }

                OperationsEditor { id, step: index }

                // A locating-pins step has no face to choose. Pins are what *lets* the
                // board be turned over, so they are drilled before it ever is — on the
                // front, by definition. The control is absent rather than disabled: a
                // greyed-out dropdown invites the operator to look for the thing that
                // would ungrey it, and there is nothing.
                if drills_pins {
                    p { class: "field-hint",
                        "Machines the front face — locating pins are drilled before the board is turned over."
                    }
                } else {
                    SchemaField { id, ptr: format!("/steps/{index}/board_face") }
                }

                // Configuration sections for the currently enabled operations, each one
                // foldable. A step running three operations is three schema forms deep
                // enough to bury the one being edited, and they are edited one at a time.
                //
                // Folded by key rather than by position: the list is filtered by what the
                // step enables, so an operation's index changes when a *different* one is
                // ticked. Keyed by index, unticking PTH would silently fold whatever moved
                // up into its place.
                for op in machining_operations().iter() {
                    if enabled_ops.iter().any(|enabled| enabled == op.key) {
                        div { class: "schema-section op-section",
                            div { class: "op-section-header",
                                button {
                                    r#type: "button",
                                    class: "icon-btn",
                                    title: if folded_ops.read().contains(op.key) { "Expand section" } else { "Collapse section" },
                                    "aria-expanded": if folded_ops.read().contains(op.key) { "false" } else { "true" },
                                    onclick: move |_| {
                                        folded_ops
                                            .with_mut(|folded| {
                                                if !folded.remove(op.key) {
                                                    folded.insert(op.key.to_string());
                                                }
                                            });
                                    },
                                    if folded_ops.read().contains(op.key) { "▸" } else { "▾" }
                                }
                                h4 { class: "section-title", "{op.label}" }
                            }
                            if !folded_ops.read().contains(op.key) {
                                SchemaForm { id, ptr: format!("/steps/{index}/{}", op.key) }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod selection_tests {
    use super::*;

    /// Three profiles with stable, distinguishable ids.
    fn profiles() -> Vec<(Uuid, String)> {
        (1u8..=3)
            .map(|n| (Uuid::from_bytes([n; 16]), format!("Profile {n}")))
            .collect()
    }

    fn id(n: u8) -> Uuid {
        Uuid::from_bytes([n; 16])
    }

    /// The point of the rule: arriving on this screen shows what the job runs, not
    /// whichever profile happens to be first in the list.
    #[test]
    fn the_screen_opens_on_the_profile_the_job_runs() {
        assert_eq!(profile_on_show(None, Some(id(3)), &profiles()), Some(id(3)));
    }

    /// Picking here overrides the job, so the screen does not snap back to the job's
    /// profile on the next render.
    #[test]
    fn a_profile_picked_here_wins_over_the_jobs() {
        assert_eq!(profile_on_show(Some(id(2)), Some(id(3)), &profiles()), Some(id(2)));
    }

    /// A job with no profile yet still has to land the operator somewhere editable.
    #[test]
    fn with_no_job_profile_the_first_one_is_shown() {
        assert_eq!(profile_on_show(None, None, &profiles()), Some(id(1)));
    }

    /// A reference to something deleted must fall back, not blank the editor — the
    /// panel would otherwise read "select or add a profile" with profiles right there
    /// in the dropdown.
    #[test]
    fn a_stale_reference_falls_back_to_a_real_profile() {
        assert_eq!(profile_on_show(None, Some(id(9)), &profiles()), Some(id(1)), "job's is gone");
        assert_eq!(
            profile_on_show(Some(id(9)), Some(id(3)), &profiles()),
            Some(id(3)),
            "the pick is gone, so the job speaks again",
        );
    }

    /// Nothing to show when there is nothing at all — the empty-state panel.
    #[test]
    fn an_empty_library_selects_nothing() {
        assert_eq!(profile_on_show(Some(id(1)), Some(id(2)), &[]), None);
    }
}

#[cfg(test)]
mod fold_tests {
    use super::*;

    /// Drive the remaps through a plain set, the way the signal wrappers do.
    ///
    /// The signal versions are one-line adaptors over these rules; testing the rules is
    /// what matters, because getting them wrong is invisible — the wrong card folds, and
    /// the operator concludes the button is flaky rather than that the state is off by one.
    fn moved(folded: &[usize], from: usize, to: usize) -> Vec<usize> {
        let mut set: BTreeSet<usize> = folded.iter().copied().collect();
        set = remap_indexes(&set, from, to);
        set.into_iter().collect()
    }

    fn removed(folded: &[usize], at: usize) -> Vec<usize> {
        let set: BTreeSet<usize> = folded.iter().copied().collect();
        drop_index(&set, at).into_iter().collect()
    }

    /// A folded step keeps its fold when it is the one that moved.
    #[test]
    fn the_moved_step_carries_its_fold() {
        assert_eq!(moved(&[0], 0, 1), [1], "moved down");
        assert_eq!(moved(&[2], 2, 1), [1], "moved up");
    }

    /// The step displaced by the move shifts the other way, because `move_step` is a
    /// splice: removing from `from` and inserting at `to` slides everything between.
    #[test]
    fn the_displaced_steps_shift_the_other_way() {
        // 0 folded, step 0 moves down past it -> the old 1 becomes 0.
        assert_eq!(moved(&[1], 0, 1), [0]);
        // 0 folded, step 2 moves up to the top -> everything below slides down one.
        assert_eq!(moved(&[0, 1], 2, 0), [1, 2]);
    }

    /// Steps outside the moved span are untouched.
    #[test]
    fn steps_beyond_the_move_are_left_alone() {
        assert_eq!(moved(&[4], 0, 1), [4]);
        assert_eq!(moved(&[0], 2, 3), [0]);
    }

    /// A no-op move must not disturb anything.
    #[test]
    fn moving_a_step_onto_itself_changes_nothing() {
        assert_eq!(moved(&[0, 2], 1, 1), [0, 2]);
    }

    /// Removing a step drops its fold and closes the gap, so the cards below do not
    /// inherit a fold from the step that used to be above them.
    #[test]
    fn removing_a_step_drops_its_fold_and_closes_the_gap() {
        assert_eq!(removed(&[0, 2], 1), [0, 1], "2 slides down to 1");
        assert_eq!(removed(&[1], 1), [] as [usize; 0], "its own fold goes with it");
        assert_eq!(removed(&[0], 2), [0], "a removal below changes nothing above");
    }
}

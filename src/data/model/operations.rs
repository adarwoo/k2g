//! The machining operations a step can run, and how many steps may run each: any number,
//! one per board face, or one for the whole job ([`OperationScope`]).
//!
//! This mirrors the `operation_key` enum in `schemas/machining.yaml` and is the one
//! place that knows what each key *means* to the operator. It lives below the UI
//! because two very different consumers need the same answer: the machining editor,
//! which greys out an operation another step has claimed, and the readiness gate,
//! which refuses a hand-edited profile that claims one twice.

/// How many steps of one profile may run an operation.
///
/// Three answers, because there are three: some operations are a division of labour, some
/// describe a feature of one face, and one describes the job's own registration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationScope {
    /// Any number of steps. Passes at different depths, or over different regions, are
    /// all legitimately the same operation.
    Repeatable,
    /// At most one step per board face.
    ///
    /// Everything that removes the board's own defining material: those features exist
    /// once, so cutting them in two steps means cutting them twice — the second pass runs
    /// a tool through air it has already cleared, or worse, re-drills a hole that has
    /// moved with the fixture.
    ///
    /// *Per face*, not per profile, because a face is a separate setup with its own
    /// geometry: milling the front and then the back is two distinct jobs that happen to
    /// share a key.
    OncePerFace,
    /// At most one step in the whole profile, whichever face it is on.
    ///
    /// The locating pins and only the locating pins. They are not a feature of a face —
    /// they are the datum the *job* is registered against, and
    /// [`locating_pin_faults`](crate::runtime::tooling::locating_pin_faults) refuses any
    /// pins step that is not the first one, on either face.
    ///
    /// This entry used to be [`Self::Repeatable`], reasoning that a job which moves the
    /// board to a second fixture genuinely drills a second set. The readiness gate has
    /// never agreed: it refuses a pins step anywhere but the top. So the operator could
    /// tick the box in step 2 and only find out at the Job screen, with a no-go and no
    /// hint that the tick was what caused it. If re-fixturing mid-job is ever wanted, it
    /// is that rule that has to change first, and this follows it.
    OncePerJob,
}

/// One machining operation: its schema key, the operator-facing label, and how many steps
/// may run it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MachiningOperation {
    /// The `operation_key` value persisted in the profile.
    pub key: &'static str,
    /// How the operation is named to the operator, in the UI and in messages.
    pub label: &'static str,
    /// The same thing in as few characters as still identify it, for places that name
    /// several operations at once — a step chip, a folded card's heading.
    ///
    /// Not derived from [`Self::label`] by truncation: "Drill plated holes (PTH)" and
    /// "Drill non-plated holes (NPTH)" share their first nineteen characters, so any
    /// automatic shortening makes exactly the two operations an operator most needs to
    /// tell apart indistinguishable.
    pub short_label: &'static str,
    /// How many steps of one profile may run it. See [`OperationScope`].
    pub scope: OperationScope,
}

/// The operations, **in the order a board is made in**.
///
/// An operator reading this list is reading the sequence: it is the order the picker
/// shows, the order a step's blocks come out in, and the order the program runs. One
/// order to learn rather than three to reconcile.
///
/// The sequence is not arbitrary — each position is owed to a physical rule:
///
/// 1. **Engraving** while the board is whole, flat and undrilled. Z0 is verified against
///    an unmachined surface, and every hole made first is a place the surface can lift or
///    the bit can catch. It is also the one operation whose quality is a depth tolerance.
/// 2. **Locating pins** before the rest of the drilling: they are the datum the board is
///    registered against, so they want making before anything measured from them.
/// 3. **The holes** — PTH then NPTH — while the board is still fully attached. The
///    reading order is what this list gives; the blocks inside the drill phase stay
///    grouped by tool, which is what keeps a drill serving both kinds from being loaded
///    twice. See [`plan_drilling`](crate::gcode::planner::plan_drilling).
/// 4. **Interior cutouts** before the perimeter. Once the outline is breached — even
///    tabbed — the part shifts and interior cuts lose accuracy (op-planner §4).
/// 5. **The outline** last, tabbed, because it is what releases the part.
///
/// It used to be ordered by how often a step uses each one, which put engraving last and
/// the outline third. That order told the reader nothing, and the ordering rules above
/// were left to be discovered in the planner.
///
/// # Nothing may depend on a position here
///
/// `engrave_copper`'s old placement at the end was load-bearing: `AppData::add_step` took
/// the first operation that was repeatable *or* unclaimed, so a repeatable entry any
/// earlier became the default for every new step. That is fixed at the source — `add_step`
/// now asks for the first *unclaimed* operation and falls back to a repeatable one — so
/// this list is free to be ordered for the person reading it. Keep it that way: an entry
/// added here should be placed where the board is made, and anything that needs a
/// different order should say so itself.
pub const MACHINING_OPERATIONS: &[MachiningOperation] = &[
    // Repeatable: passes at different depths, or over different regions, are all
    // legitimately engraving.
    MachiningOperation {
        key: "engrave_copper",
        label: "Engrave copper isolation",
        short_label: "Engrave",
        scope: OperationScope::Repeatable,
    },
    // Once for the whole profile, on either face. Not a feature of a board face — the
    // datum the job is registered against. See `OperationScope::OncePerJob`.
    MachiningOperation {
        key: "drill_locating_pins",
        label: "Drill locating pins",
        short_label: "Pins",
        scope: OperationScope::OncePerJob,
    },
    MachiningOperation {
        key: "drill_pth",
        label: "Drill plated holes (PTH)",
        short_label: "PTH",
        scope: OperationScope::OncePerFace,
    },
    MachiningOperation {
        key: "drill_npth",
        label: "Drill non-plated holes (NPTH)",
        short_label: "NPTH",
        scope: OperationScope::OncePerFace,
    },
    // Once per face like the boundary: the openings exist once, so two steps both
    // claiming them on one face is a genuine conflict rather than a division of labour.
    MachiningOperation {
        key: "route_cutouts",
        label: "Route interior cutouts",
        short_label: "Cutouts",
        scope: OperationScope::OncePerFace,
    },
    MachiningOperation {
        key: "route_board",
        label: "Cut board outline",
        short_label: "Outline",
        scope: OperationScope::OncePerFace,
    },
];

/// The operation `key` describes, if it is one this build knows.
///
/// Unknown keys are possible — a profile written by a later version, or hand-edited —
/// and are treated as unconstrained rather than rejected, so an old build does not
/// refuse to open a newer file.
pub fn machining_operation(key: &str) -> Option<&'static MachiningOperation> {
    MACHINING_OPERATIONS.iter().find(|op| op.key == key)
}

/// How `key` is named to the operator, falling back to the raw key when unknown so a
/// message never comes out blank.
pub fn operation_label(key: &str) -> &str {
    machining_operation(key).map(|op| op.label).unwrap_or(key)
}

/// How many steps may run `key`.
///
/// An unknown key — a profile from a later build, or hand-edited — is
/// [`OperationScope::Repeatable`], i.e. unconstrained. An old k2g must still open a newer
/// file, and refusing an operation it cannot reason about would be inventing a rule.
pub fn operation_scope(key: &str) -> OperationScope {
    machining_operation(key).map_or(OperationScope::Repeatable, |op| op.scope)
}

/// The name a freshly added step carries until the operator gives it one of their own.
///
/// Written into the document by `AppData::add_step` and treated by
/// [`step_display_name`] as "not named yet".
pub const UNNAMED_STEP: &str = "Machining step";

/// What to call a step: the operator's own name, or — while they have not given it one —
/// a name built from what the step actually does.
///
/// "Machining step", "Machining step", "Machining step" is what a profile grown with
/// "+ Add step" looks like, and it tells the operator nothing about which is which at
/// exactly the moment they are trying to find one. The operations *are* the distinguishing
/// fact, so they are what the name says until something better is supplied.
///
/// Deliberately **not** persisted: writing the derived name into the document would make
/// the step look named, and it would then stop tracking the operations it describes — tick
/// "Cut board outline" on a step called "PTH" and the name is a lie the operator did not
/// tell. Derived on read, it always matches.
///
/// The test for "not named yet" is the literal [`UNNAMED_STEP`] default (or blank), not the
/// node's `default_applied` flag: `add_step` writes the name explicitly, so the flag is
/// false from the moment a step is created.
pub fn step_display_name(name: &str, operations: &[String]) -> String {
    let trimmed = name.trim();
    if !trimmed.is_empty() && trimmed != UNNAMED_STEP {
        return trimmed.to_string();
    }

    // Schema order, not the order they happen to be stored in, so two steps with the same
    // operations always read the same way round.
    let parts: Vec<&str> = MACHINING_OPERATIONS
        .iter()
        .filter(|op| operations.iter().any(|key| key == op.key))
        .map(|op| op.short_label)
        .collect();

    // An unknown key (a profile from a later build) still deserves to be named after
    // something. Fall back to the keys themselves rather than to a blank chip.
    if parts.is_empty() {
        let unknown: Vec<&str> = operations.iter().map(String::as_str).collect();
        return if unknown.is_empty() {
            UNNAMED_STEP.to_string()
        } else {
            unknown.join(" + ")
        };
    }
    parts.join(" + ")
}

/// How a step is referred to in a message: by the ordinal the editor shows as its
/// heading, plus the operator's own name for it when there is one.
///
/// The ordinal leads because names need not be unique — a profile grown with "+ Add
/// step" has every step called "Machining step", and a message naming two of those
/// tells the operator nothing about which two.
pub fn step_reference(index: usize, name: &str) -> String {
    match name.trim() {
        "" => format!("step {}", index + 1),
        name => format!("step {} '{name}'", index + 1),
    }
}

/// One operation claimed by more than one step on the same board face.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationConflict {
    /// The operation's schema key.
    pub key: String,
    /// Whether the clash is on the back face.
    pub back: bool,
    /// Every step claiming it as `(index, name)`, in step order.
    pub steps: Vec<(usize, String)>,
}

impl OperationConflict {
    /// The conflict as one operator-facing sentence.
    pub fn message(&self) -> String {
        format!(
            "{} is set in {} on the {} face; only one step may cut it.",
            operation_label(&self.key),
            self.steps
                .iter()
                .map(|(index, name)| step_reference(*index, name))
                .collect::<Vec<_>>()
                .join(" and "),
            if self.back { "back" } else { "front" },
        )
    }
}

/// The step whose claim on `key` stops step `step` claiming it too, as `(index, name)`.
///
/// Drives the editor's greyed-out checkbox. `None` when the box is free: a repeatable
/// operation, an unknown key, `step` itself, or — for a face-scoped operation — a claim
/// on the other face.
///
/// How far it looks is the [`OperationScope`]:
///
/// - [`Repeatable`](OperationScope::Repeatable) — never blocked.
/// - [`OncePerFace`](OperationScope::OncePerFace) — blocked by a step on the *same* face.
///   The front's outline says nothing about the back's; they are separate setups.
/// - [`OncePerJob`](OperationScope::OncePerJob) — blocked by any step at all. The
///   locating pins register the whole job, and the readiness gate refuses a second pins
///   step outright, so leaving the box tickable only moves the discovery to the Job
///   screen — where a no-go appears with nothing connecting it back to the tick.
///
/// Takes `(step name, machines the back, operations)` like [`conflicting_operations`], so
/// it stays a pure function over the three facts it needs and is testable without a store.
pub fn blocking_step<'a>(
    steps: impl IntoIterator<Item = (&'a str, bool, &'a [String])>,
    step: usize,
    key: &str,
) -> Option<(usize, String)> {
    let scope = operation_scope(key);
    if scope == OperationScope::Repeatable {
        return None;
    }
    let claims: Vec<(&str, bool, &[String])> = steps.into_iter().collect();
    let side = claims.get(step)?.1;
    claims
        .iter()
        .enumerate()
        .find(|(index, (_, back, operations))| {
            *index != step
                && (scope == OperationScope::OncePerJob || *back == side)
                && operations.iter().any(|op| op == key)
        })
        .map(|(index, (name, _, _))| (index, (*name).to_string()))
}

/// Every once-per-face operation claimed by two or more of `steps` on the same face.
///
/// Takes `(step name, machines the back, operations)` rather than any richer step type so
/// it stays a pure function over the only three facts it needs, testable without a
/// datastore. Conflicts come back in operation order, each listing its steps in step
/// order, so the message reads the way the editor is laid out.
pub fn conflicting_operations<'a>(
    steps: impl IntoIterator<Item = (&'a str, bool, &'a [String])>,
) -> Vec<OperationConflict> {
    // (key, face) -> claiming steps. Collected in one pass so the faces stay
    // independent: the same key on opposite faces is two separate tallies, never one.
    let mut claims: Vec<((&str, bool), Vec<(usize, String)>)> = Vec::new();

    // The iteration order is step order, so the position here *is* the step index the
    // editor shows — no index needs threading in from the caller.
    for (index, (name, back, operations)) in steps.into_iter().enumerate() {
        for key in operations {
            // Face-scoped operations only. A job-scoped one (the locating pins) is not a
            // face question at all — two pins steps on *opposite* faces is exactly as
            // wrong as two on the same one, and this tally, keyed by face, would miss it.
            // `locating_pin_faults` owns that rule and says the useful thing about it
            // ("move it to the top"), so adding a second message here would be noise.
            if operation_scope(key) != OperationScope::OncePerFace {
                continue;
            }
            match claims
                .iter_mut()
                .find(|(k, _)| *k == (key.as_str(), back))
            {
                Some((_, claimants)) => claimants.push((index, name.to_string())),
                None => claims.push(((key, back), vec![(index, name.to_string())])),
            }
        }
    }

    // Report in operation order rather than first-seen order, so two conflicts in one
    // profile are listed the way the editor lists their checkboxes.
    let mut conflicts: Vec<OperationConflict> = claims
        .into_iter()
        .filter(|(_, claimants)| claimants.len() > 1)
        .map(|((key, back), steps)| OperationConflict {
            key: key.to_string(),
            back,
            steps,
        })
        .collect();
    conflicts.sort_by_key(|conflict| {
        MACHINING_OPERATIONS
            .iter()
            .position(|op| op.key == conflict.key)
            .unwrap_or(usize::MAX)
    });
    conflicts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ops(keys: &[&str]) -> Vec<String> {
        keys.iter().map(|k| k.to_string()).collect()
    }

    /// The table must not drift from `machining.yaml`'s enum, which is what the UI
    /// renders and the documents persist.
    #[test]
    fn the_table_matches_the_schema_enum() {
        const SCHEMA: &str = include_str!("../../../schemas/machining.yaml");
        let schema: serde_yaml::Value =
            serde_yaml::from_str(SCHEMA).expect("machining.yaml parses");
        let keys: Vec<String> = schema["$defs"]["operation_key"]["enum"]
            .as_sequence()
            .expect("operation_key is an enum")
            .iter()
            .map(|v| v.as_str().expect("enum entries are strings").to_string())
            .collect();

        let table: Vec<String> = MACHINING_OPERATIONS
            .iter()
            .map(|op| op.key.to_string())
            .collect();
        assert_eq!(
            keys, table,
            "the operation table and the schema enum must agree, in order"
        );
    }

    /// The point of the rule: a feature the board has once is cut once.
    #[test]
    fn one_face_may_not_claim_the_same_operation_twice() {
        let conflicts = conflicting_operations([
            ("Drill", false, ops(&["drill_pth"]).as_slice()),
            (
                "Cut out",
                false,
                ops(&["drill_pth", "route_board"]).as_slice(),
            ),
        ]);

        assert_eq!(conflicts.len(), 1, "only drill_pth clashes");
        assert_eq!(conflicts[0].key, "drill_pth");
        assert_eq!(
            conflicts[0].steps,
            vec![(0, "Drill".to_string()), (1, "Cut out".to_string())]
        );
        assert!(!conflicts[0].back);
    }

    /// Steps need not have distinct names — a profile grown with "+ Add step" calls
    /// every one of them "Machining step" — so the message leads with the ordinal the
    /// editor shows. Naming two identically-named steps identifies neither.
    #[test]
    fn the_message_tells_identically_named_steps_apart() {
        let conflicts = conflicting_operations([
            ("Machining step", false, ops(&["drill_pth"]).as_slice()),
            ("Machining step", false, ops(&["drill_pth"]).as_slice()),
        ]);

        let message = conflicts[0].message();
        assert!(message.contains("step 1 'Machining step'"), "{message}");
        assert!(message.contains("step 2 'Machining step'"), "{message}");
    }

    /// A profile grown with "+ Add step" is three cards all called "Machining step", which
    /// says nothing about which is which at exactly the moment the operator is looking for
    /// one. Until they name it themselves, the step is called after what it does.
    #[test]
    fn an_unnamed_step_is_named_after_what_it_does() {
        assert_eq!(step_display_name(UNNAMED_STEP, &ops(&["drill_pth"])), "PTH");
        assert_eq!(step_display_name("", &ops(&["route_board"])), "Outline");
        assert_eq!(
            step_display_name("   ", &ops(&["drill_npth", "drill_pth", "drill_locating_pins"])),
            "Pins + PTH + NPTH",
            "the order the board is made in, not the order they were ticked"
        );
    }

    /// **The order a board is made in, pinned.**
    ///
    /// This list is the picker, the step name and — through the planner — the order the
    /// program runs. Asserted whole rather than as a set, so an operation added later has
    /// to be placed deliberately instead of landing wherever the diff was smallest.
    ///
    /// Each position is owed to a physical rule, and getting one wrong is not a cosmetic
    /// fault: engraving after drilling verifies Z0 against a surface that has already been
    /// machined, and the outline before the cutouts machines interior features on a part
    /// the perimeter cut has already released.
    #[test]
    fn the_operations_read_in_the_order_a_board_is_made_in() {
        let keys: Vec<&str> = MACHINING_OPERATIONS.iter().map(|op| op.key).collect();

        assert_eq!(
            keys,
            [
                "engrave_copper",       // whole, flat, undrilled — and Z0 is verified here
                "drill_locating_pins",  // the datum, before anything measured from it
                "drill_pth",            // holes while the board is still fully attached
                "drill_npth",
                "route_cutouts",        // interior before the perimeter (op-planner §4)
                "route_board",          // last: it is what releases the part
            ],
        );
    }

    /// The two operations no *face* claims lead the list, which is only safe because
    /// nothing derives a default from a position here — see `AppData::add_step`, which
    /// asks what a face still lacks rather than taking the first entry it is allowed to.
    ///
    /// Worth its own test because the coupling is invisible from either side: this list
    /// carries no marker saying a default is drawn from it, and `add_step` names no
    /// position. The previous arrangement worked only because both entries happened to
    /// sit at the end.
    #[test]
    fn the_operations_no_face_claims_may_lead_the_list() {
        let leading: Vec<&str> = MACHINING_OPERATIONS
            .iter()
            .take_while(|op| op.scope != OperationScope::OncePerFace)
            .map(|op| op.key)
            .collect();

        assert_eq!(
            leading,
            ["engrave_copper", "drill_locating_pins"],
            "if this changes, check `add_step` still claims work rather than a repeatable",
        );
        // And `add_step`'s fallback names `Repeatable`, so the pins sitting second here
        // cannot become the default for a new step however the list is reordered.
        assert_eq!(operation_scope("drill_locating_pins"), OperationScope::OncePerJob);
    }

    /// The operator's own name always wins, and is never overwritten by the derivation —
    /// otherwise naming a step would appear to work and then silently undo itself the next
    /// time an operation was ticked.
    #[test]
    fn a_name_the_operator_typed_is_left_alone() {
        assert_eq!(step_display_name("Flip and drill", &ops(&["drill_pth"])), "Flip and drill");
        assert_eq!(step_display_name("  Cut out  ", &ops(&[])), "Cut out", "trimmed, not replaced");
    }

    /// A step whose operations this build does not know still gets a name from them rather
    /// than a blank chip — an old k2g opening a newer profile must still be navigable.
    #[test]
    fn an_unknown_operation_still_names_its_step() {
        assert_eq!(step_display_name(UNNAMED_STEP, &ops(&["engrave"])), "engrave");
        assert_eq!(
            step_display_name(UNNAMED_STEP, &ops(&[])),
            UNNAMED_STEP,
            "and a step with no operations at all keeps the placeholder"
        );
    }

    /// An unnamed step still has to be referrable.
    #[test]
    fn a_step_with_no_name_is_referred_to_by_its_ordinal_alone() {
        assert_eq!(step_reference(0, ""), "step 1");
        assert_eq!(step_reference(2, "   "), "step 3");
        assert_eq!(step_reference(1, "Cut out"), "step 2 'Cut out'");
    }

    /// The reason the rule is per face rather than per profile: two faces are two
    /// setups, and cutting the outline of each is two different jobs.
    #[test]
    fn the_two_board_faces_are_counted_separately() {
        let conflicts = conflicting_operations([
            ("Cut the front", false, ops(&["route_board"]).as_slice()),
            ("Cut the back", true, ops(&["route_board"]).as_slice()),
        ]);
        assert!(
            conflicts.is_empty(),
            "one outline cut per face is the intended workflow"
        );

        let conflicts = conflicting_operations([
            ("Rough", true, ops(&["route_board"]).as_slice()),
            ("Finish", true, ops(&["route_board"]).as_slice()),
        ]);
        assert_eq!(conflicts.len(), 1, "but cutting the same face twice is not");
        assert!(
            conflicts[0].back,
            "and the message must name the face it happened on"
        );
    }

    /// **Pins ticked in step 1 grey the box in step 2 — on either face.**
    ///
    /// The readiness gate has always refused a second pins step ("move it to the top"),
    /// but the editor let it be ticked, so the operator met the rule as a no-go on the Job
    /// screen with nothing pointing back at the tick that caused it. The box says it now.
    ///
    /// The back-face case is the one a face-scoped rule would miss, and it is the case
    /// this got wrong: the pins register the *job*, so a second set on the other side is
    /// exactly as refused as a second set on the same one.
    #[test]
    fn pins_claimed_by_one_step_are_blocked_in_every_other() {
        let first = ops(&["drill_locating_pins", "drill_pth"]);
        let second = ops(&["drill_pth"]);
        let profile = |back_of_second: bool| {
            [
                ("Pins and front", false, first.as_slice()),
                ("Second", back_of_second, second.as_slice()),
            ]
        };

        for back in [false, true] {
            let blocked = blocking_step(profile(back), 1, "drill_locating_pins");
            assert_eq!(
                blocked,
                Some((0, "Pins and front".to_string())),
                "step 2 (back: {back}) must name step 1 as the owner"
            );
        }

        // Step 1 is not blocked by its own claim, or nothing could ever be unticked.
        assert_eq!(blocking_step(profile(false), 0, "drill_locating_pins"), None);
        // And a step that is the only one claiming them is free.
        assert_eq!(
            blocking_step([("Only", false, second.as_slice())], 0, "drill_locating_pins"),
            None
        );
    }

    /// A face-scoped operation stays a *face* question: the front's outline does not
    /// block the back's, because they are separate setups cutting separate geometry.
    #[test]
    fn a_face_scoped_operation_blocks_only_its_own_face() {
        let front = ops(&["route_board"]);
        let steps = [
            ("Front outline", false, front.as_slice()),
            ("Back outline", true, front.as_slice()),
            ("More front", false, front.as_slice()),
        ];
        assert_eq!(
            blocking_step(steps, 1, "route_board"),
            None,
            "the back's outline is its own"
        );
        assert_eq!(
            blocking_step(steps, 2, "route_board"),
            Some((0, "Front outline".to_string())),
            "but a second front outline is the first one cut twice"
        );
    }

    /// Repeatable operations are never blocked, and an unknown key — from a newer build —
    /// is left alone rather than constrained by a rule this build invented.
    #[test]
    fn repeatable_and_unknown_operations_are_never_blocked() {
        let both = ops(&["engrave_copper", "some_future_operation"]);
        let steps = [("A", false, both.as_slice()), ("B", false, both.as_slice())];
        assert_eq!(blocking_step(steps, 1, "engrave_copper"), None);
        assert_eq!(blocking_step(steps, 1, "some_future_operation"), None);
    }

    /// **A second locating-pins step is refused, but not by this function.**
    ///
    /// The pins are [`OperationScope::OncePerJob`], and this tally is keyed by face — so
    /// it would miss the case that matters most, two pins steps on *opposite* faces, and
    /// would phrase the one it caught as "on the front face", which is not the reason.
    /// `locating_pin_faults` owns the rule and gives the useful remedy ("move it to the
    /// top"); a second message here would only bury it.
    ///
    /// So the emptiness below is deliberate, and the scope assertion is what stops it
    /// reading as "pins may be drilled twice" — which is what this test used to claim.
    #[test]
    fn a_second_pins_step_is_not_this_functions_business() {
        assert_eq!(operation_scope("drill_locating_pins"), OperationScope::OncePerJob);

        let conflicts = conflicting_operations([
            (
                "First setup",
                false,
                ops(&["drill_locating_pins", "drill_pth"]).as_slice(),
            ),
            (
                "Second setup",
                false,
                ops(&["drill_locating_pins", "route_board"]).as_slice(),
            ),
        ]);
        assert!(
            conflicts.is_empty(),
            "a face tally has nothing to say about a job-scoped operation: {conflicts:?}"
        );
    }

    /// A key from a newer build is left alone rather than refused, so an older k2g can
    /// still open a profile it does not fully understand.
    #[test]
    fn an_unknown_operation_key_is_not_constrained() {
        let conflicts = conflicting_operations([
            ("A", false, ops(&["engrave"]).as_slice()),
            ("B", false, ops(&["engrave"]).as_slice()),
        ]);
        assert!(conflicts.is_empty());
        assert_eq!(
            operation_label("engrave"),
            "engrave",
            "and still names itself"
        );
    }

    #[test]
    fn the_message_names_the_operation_the_steps_and_the_face() {
        let conflict = OperationConflict {
            key: "route_board".to_string(),
            back: false,
            steps: vec![(0, "Drill".to_string()), (1, "Cut out".to_string())],
        };
        assert_eq!(
            conflict.message(),
            "Cut board outline is set in step 1 'Drill' and step 2 'Cut out' on the front \
             face; only one step may cut it."
        );
    }
}

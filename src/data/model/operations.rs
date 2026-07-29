//! The machining operations a step can run, and which of them a board side may
//! receive only once.
//!
//! This mirrors the `operation_key` enum in `schemas/machining.yaml` and is the one
//! place that knows what each key *means* to the operator. It lives below the UI
//! because two very different consumers need the same answer: the machining editor,
//! which greys out an operation another step has claimed, and the readiness gate,
//! which refuses a hand-edited profile that claims one twice.

/// One machining operation: its schema key, the operator-facing label, and whether a
/// board side may receive it more than once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MachiningOperation {
    /// The `operation_key` value persisted in the profile.
    pub key: &'static str,
    /// How the operation is named to the operator, in the UI and in messages.
    pub label: &'static str,
    /// Whether at most one step per board side may run it.
    ///
    /// True for everything that removes the board's own defining material: those
    /// features exist once, so cutting them in two steps means cutting them twice —
    /// the second pass runs a tool through air it has already cleared, or worse,
    /// re-drills a hole that has moved with the fixture.
    ///
    /// *Per side*, not per profile, because a side is a separate setup with its own
    /// geometry: milling the component side and then the solder side is two distinct
    /// jobs that happen to share a key.
    pub once_per_side: bool,
}

/// The operations, in schema order.
///
/// Ordered by how often a step uses them, not alphabetically or by phase: almost
/// every job drills PTH, most also drill NPTH, many route the edge; locating pins and
/// milling are the exceptions. The UI shows them in this order and persists the
/// enabled set in it.
///
/// `drill_locating_pins` is the one repeatable operation today. Pins register the
/// board against a *fixture*, so a job that moves the board to a second fixture
/// genuinely drills a second set — the key names the act, not a feature of the board.
/// Engraving will join it when it lands, for the same reason: several passes at
/// different depths, or on different regions, are all legitimately engraving.
pub const MACHINING_OPERATIONS: &[MachiningOperation] = &[
    MachiningOperation {
        key: "drill_pth",
        label: "Drill plated holes (PTH)",
        once_per_side: true,
    },
    MachiningOperation {
        key: "drill_npth",
        label: "Drill non-plated holes (NPTH)",
        once_per_side: true,
    },
    MachiningOperation {
        key: "route_board",
        label: "Route board edge",
        once_per_side: true,
    },
    MachiningOperation {
        key: "drill_locating_pins",
        label: "Drill locating pins",
        once_per_side: false,
    },
    MachiningOperation {
        key: "mill_board",
        label: "Mill board",
        once_per_side: true,
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

/// Whether `key` may appear in only one step per board side.
pub fn operation_once_per_side(key: &str) -> bool {
    machining_operation(key).is_some_and(|op| op.once_per_side)
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

/// One operation claimed by more than one step on the same board side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationConflict {
    /// The operation's schema key.
    pub key: String,
    /// Whether the clash is on the bottom side.
    pub bottom: bool,
    /// Every step claiming it as `(index, name)`, in step order.
    pub steps: Vec<(usize, String)>,
}

impl OperationConflict {
    /// The conflict as one operator-facing sentence.
    pub fn message(&self) -> String {
        format!(
            "{} is set in {} on the {} side; only one step may cut it.",
            operation_label(&self.key),
            self.steps
                .iter()
                .map(|(index, name)| step_reference(*index, name))
                .collect::<Vec<_>>()
                .join(" and "),
            if self.bottom { "bottom" } else { "top" },
        )
    }
}

/// Every once-per-side operation claimed by two or more of `steps` on the same side.
///
/// Takes `(step name, machines bottom, operations)` rather than any richer step type so
/// it stays a pure function over the only three facts it needs, testable without a
/// datastore. Conflicts come back in operation order, each listing its steps in step
/// order, so the message reads the way the editor is laid out.
pub fn conflicting_operations<'a>(
    steps: impl IntoIterator<Item = (&'a str, bool, &'a [String])>,
) -> Vec<OperationConflict> {
    // (key, side) -> claiming steps. Collected in one pass so the sides stay
    // independent: the same key on opposite sides is two separate tallies, never one.
    let mut claims: Vec<((&str, bool), Vec<(usize, String)>)> = Vec::new();

    // The iteration order is step order, so the position here *is* the step index the
    // editor shows — no index needs threading in from the caller.
    for (index, (name, bottom, operations)) in steps.into_iter().enumerate() {
        for key in operations {
            if !operation_once_per_side(key) {
                continue;
            }
            match claims
                .iter_mut()
                .find(|(k, _)| *k == (key.as_str(), bottom))
            {
                Some((_, claimants)) => claimants.push((index, name.to_string())),
                None => claims.push(((key, bottom), vec![(index, name.to_string())])),
            }
        }
    }

    // Report in operation order rather than first-seen order, so two conflicts in one
    // profile are listed the way the editor lists their checkboxes.
    let mut conflicts: Vec<OperationConflict> = claims
        .into_iter()
        .filter(|(_, claimants)| claimants.len() > 1)
        .map(|((key, bottom), steps)| OperationConflict {
            key: key.to_string(),
            bottom,
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
    fn one_side_may_not_claim_the_same_operation_twice() {
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
        assert!(!conflicts[0].bottom);
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

    /// An unnamed step still has to be referrable.
    #[test]
    fn a_step_with_no_name_is_referred_to_by_its_ordinal_alone() {
        assert_eq!(step_reference(0, ""), "step 1");
        assert_eq!(step_reference(2, "   "), "step 3");
        assert_eq!(step_reference(1, "Cut out"), "step 2 'Cut out'");
    }

    /// The reason the rule is per side rather than per profile: two sides are two
    /// setups, and milling each one is two different jobs.
    #[test]
    fn the_two_board_sides_are_counted_separately() {
        let conflicts = conflicting_operations([
            (
                "Mill component side",
                false,
                ops(&["mill_board"]).as_slice(),
            ),
            ("Mill solder side", true, ops(&["mill_board"]).as_slice()),
        ]);
        assert!(
            conflicts.is_empty(),
            "one mill per side is the intended workflow"
        );

        let conflicts = conflicting_operations([
            ("Rough", true, ops(&["mill_board"]).as_slice()),
            ("Finish", true, ops(&["mill_board"]).as_slice()),
        ]);
        assert_eq!(conflicts.len(), 1, "but milling the same side twice is not");
        assert!(
            conflicts[0].bottom,
            "and the message must name the side it happened on"
        );
    }

    /// Locating pins register the board against a fixture, so a job that re-fixtures
    /// legitimately drills them again.
    #[test]
    fn locating_pins_may_be_drilled_in_more_than_one_step() {
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
            "pins are the one repeatable operation today"
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
    fn the_message_names_the_operation_the_steps_and_the_side() {
        let conflict = OperationConflict {
            key: "route_board".to_string(),
            bottom: false,
            steps: vec![(0, "Drill".to_string()), (1, "Cut out".to_string())],
        };
        assert_eq!(
            conflict.message(),
            "Route board edge is set in step 1 'Drill' and step 2 'Cut out' on the top side; \
             only one step may cut it."
        );
    }
}

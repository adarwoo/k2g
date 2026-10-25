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
/// The default order, not the only one: a step runs its operations in the order it lists
/// them, and the operator may reorder them within [`order_fault`]'s rules. This is where
/// an added operation is placed ([`insert_position`]), the order the *Add operation*
/// overlay offers them in, and what every profile written before ordering existed holds.
///
/// The sequence is not arbitrary — each position is owed to a physical rule:
///
/// 1. **Isolation** while the board is whole, flat and undrilled. Z0 is verified against
///    an unmachined surface, and every hole made first is a place the surface can lift or
///    the bit can catch. It is also the one operation whose quality is a depth tolerance.
///    **Copper clearing** straight after it, with the bit it leaves in the spindle.
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
/// Engraving's old placement at the end was load-bearing: `AppData::add_step` took
/// the first operation that was repeatable *or* unclaimed, so a repeatable entry any
/// earlier became the default for every new step. That is fixed at the source — `add_step`
/// now asks for the first *unclaimed* operation and falls back to a repeatable one — so
/// this list is free to be ordered for the person reading it. Keep it that way: an entry
/// added here should be placed where the board is made, and anything that needs a
/// different order should say so itself.
pub const MACHINING_OPERATIONS: &[MachiningOperation] = &[
    // Repeatable: passes at different depths, or over different regions, are all
    // legitimately isolation. The two halves of what was once one `engrave_copper`
    // operation — split because cutting the channels and clearing the copper they leave
    // are different work, set up differently, with different tools.
    MachiningOperation {
        key: "isolate_traces",
        label: "Isolate traces",
        short_label: "Isolate",
        scope: OperationScope::Repeatable,
    },
    // Repeatable for the same reason, and only ever straight after `isolate_traces` in
    // its own step — see `order_fault`.
    MachiningOperation {
        key: "clear_copper",
        label: "Clear copper",
        short_label: "Clear",
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

    // In the order the step runs them. This was schema order once, so that two steps with
    // the same operations read the same way round — but the order is the operator's now,
    // and "PTH + Engrave" and "Engrave + PTH" are different programs.
    let parts: Vec<&str> = operations
        .iter()
        .filter_map(|key| machining_operation(key))
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

/// The operations that make holes. The planner drills them as **one phase**, grouped by
/// drill, so the order of the three among themselves is the drills' business, not the
/// list's — see [`order_fault`].
const HOLE_OPERATIONS: [&str; 3] = ["drill_locating_pins", "drill_pth", "drill_npth"];

/// Why a step's `operations` cannot run in the order given, or `None` when they can.
///
/// A step's operations run **in the order they are listed**, within these rules. Each is
/// physical, not a preference, which is why the editor refuses a move that breaks one
/// rather than warning about it:
///
/// 1. **The outline is last.** It is what releases the part; nothing can be machined
///    accurately on a board held only by its tabs.
/// 2. **Cutouts come after everything but the outline.** An opening weakens the board
///    around it, and the holes and copper near it are cut while it is still whole
///    (op-planner §4).
/// 3. **The holes are one block, in their own order.** Every hole the step makes — pins,
///    PTH, NPTH, and the drilled half of slots and mouse bites — is drilled in one phase
///    grouped by drill, so a drill both kinds need is loaded once, with the pins' drill
///    first because the pins are the datum. Nothing can run *between* them, and listing
///    NPTH before PTH would be a promise the program does not keep.
///
/// 4. **Copper clearing comes straight after the isolation it clears for.** It works from
///    the channels that pass cut, with the bit it leaves in the spindle, so it needs one in
///    the same step and nothing between them. The two move together.
///
/// What is left free is the real choice: **the copper work before the holes, or after
/// them**. Before is the long-standing default — Z0 is checked on an untouched surface, and the
/// copper is cut on a board with nowhere to lift. After has its own case: drilled after
/// isolation, a pad's annular ring is an island no longer tied to the copper round it, and
/// a drill breaking through can lift or tear it.
///
/// Unknown keys (a profile from a later build) are ignored rather than refused, as
/// everywhere else in this module.
pub fn order_fault(operations: &[String]) -> Option<String> {
    let known: Vec<&str> = operations
        .iter()
        .map(String::as_str)
        .filter(|key| machining_operation(key).is_some())
        .collect();
    if let Some(fault) = routing_fault(&known) {
        return Some(fault);
    }
    if let Some(clear) = known.iter().position(|key| *key == "clear_copper") {
        if clear == 0 || known[clear - 1] != "isolate_traces" {
            return Some(format!(
                "{} has to come straight after {}: it clears the copper that pass leaves, with \
                 the bit it leaves in the spindle.",
                operation_label("clear_copper"),
                operation_label("isolate_traces"),
            ));
        }
    }

    let holes: Vec<usize> = (0..known.len()).filter(|&i| HOLE_OPERATIONS.contains(&known[i])).collect();
    if holes.windows(2).any(|pair| pair[1] != pair[0] + 1) {
        return Some(
            "The holes are drilled as one phase, so nothing can run between them: they are              grouped by drill, so that a drill both kinds need is loaded once."
                .to_string(),
        );
    }
    let rank = |key: &str| HOLE_OPERATIONS.iter().position(|hole| *hole == key);
    if holes.windows(2).any(|pair| rank(known[pair[0]]) > rank(known[pair[1]])) {
        return Some(
            "The holes are drilled in the order their drills are loaded — the locating pins'              first, then by size — not in the order listed, so they stay listed as pins, PTH,              NPTH."
                .to_string(),
        );
    }
    None
}

/// Rules 1 and 2 of [`order_fault`] — the outline last, the cutouts just before it — over
/// a sequence of known operation keys.
///
/// Split out because they hold **across steps** as well as within one: a board released in
/// step 2 is released for step 3 too, so nothing may follow the outline wherever it is
/// cut. The hole rules do not carry over — they are about one drill phase in one setup —
/// which is why [`step_insert_position`] uses these alone.
fn routing_fault(known: &[&str]) -> Option<String> {
    let at = |key: &str| known.iter().position(|k| *k == key);

    if at("route_board").is_some_and(|i| i + 1 != known.len()) {
        return Some(format!(
            "{} has to be last: it releases the board, and nothing can be machined              accurately on a board held only by its tabs.",
            operation_label("route_board"),
        ));
    }
    if let Some(i) = at("route_cutouts") {
        if known[i + 1..].iter().any(|key| *key != "route_board") {
            return Some(format!(
                "{} has to come after the holes and the engraving, and before the outline:                  an opening weakens the board around it.",
                operation_label("route_cutouts"),
            ));
        }
    }
    None
}

/// `operations` with the operation at `index` moved one place up (`up`) or down, or why it
/// cannot move.
///
/// "One place" is the next position the rules allow, not the next slot: moving the copper
/// work down past a step's PTH and NPTH has to jump both, since stopping between them is
/// not an order the step can run in. Without the jump the move would be refused at the
/// first slot and the order after the holes could never be reached with the arrows at all.
///
/// **An operation moves with what must follow it.** Copper clearing has to sit straight
/// after its isolation ([`order_fault`]), so moving the isolation moves the pair; moved
/// alone, the isolation would leave its clearing stranded and every slot would be refused.
/// The clearing itself cannot move on its own, and says why.
///
/// The refusal is the rule the *nearest* slot breaks, which is the one the operator was
/// trying to reach.
pub fn moved(operations: &[String], index: usize, up: bool) -> Result<Vec<String>, String> {
    if index >= operations.len() {
        return Err("No such operation.".to_string());
    }
    let span = unit_len(operations, index);
    let mut rest = operations.to_vec();
    let unit: Vec<String> = rest.drain(index..index + span).collect();
    let candidates: Vec<usize> = if up {
        (0..index).rev().collect()
    } else {
        (index + 1..=rest.len()).collect()
    };
    if candidates.is_empty() {
        return Err(if up { "Already first." } else { "Already last." }.to_string());
    }
    let mut first_fault = None;
    for to in candidates {
        let mut candidate = rest.clone();
        candidate.splice(to..to, unit.iter().cloned());
        match order_fault(&candidate) {
            None => return Ok(candidate),
            Some(fault) => {
                first_fault.get_or_insert(fault);
            }
        }
    }
    Err(first_fault.unwrap_or_default())
}

/// How many operations starting at `index` move as one: the operation itself, plus a
/// `clear_copper` straight after an `isolate_traces`.
fn unit_len(operations: &[String], index: usize) -> usize {
    let carries_clearing = operations[index] == "isolate_traces"
        && operations.get(index + 1).is_some_and(|next| next == "clear_copper");
    if carries_clearing { 2 } else { 1 }
}

/// Why the operation at `index` cannot be removed from `operations`, or `None` when it can.
///
/// A step keeps at least one operation (`minItems: 1`), and taking one out must not leave
/// the rest in an order the step cannot run — the one case being an isolation with its
/// copper clearing still after it.
pub fn removal_fault(operations: &[String], index: usize) -> Option<String> {
    if operations.len() <= 1 {
        return Some("A step needs at least one operation — remove the step instead.".to_string());
    }
    let mut rest = operations.to_vec();
    if index < rest.len() {
        rest.remove(index);
    }
    order_fault(&rest).map(|_| {
        format!(
            "Remove {} first — it clears the copper this pass leaves.",
            operation_label("clear_copper"),
        )
    })
}

/// Where adding `key` to `operations` should put it: where the board-making order of
/// [`MACHINING_OPERATIONS`] would, relative to what the step already lists, or — if the
/// operator's own order makes that slot invalid — the first slot that is valid. `None`
/// only if no slot is, which no list the editor can produce reaches.
///
/// The default order rather than the end, because "add" means "this step also does X", not
/// "and then X": a step that drills and routes and gains engraving wants it at the top,
/// where it has always run, not after the outline where it cannot.
pub fn insert_position(operations: &[String], key: &str) -> Option<usize> {
    let rank = |k: &str| MACHINING_OPERATIONS.iter().position(|op| op.key == k).unwrap_or(usize::MAX);
    let preferred = operations
        .iter()
        .position(|existing| rank(existing) > rank(key))
        .unwrap_or(operations.len());
    std::iter::once(preferred)
        .chain(0..=operations.len())
        .find(|&at| {
            let mut added = operations.to_vec();
            added.insert(at, key.to_string());
            order_fault(&added).is_none()
        })
}

/// Where a new step running `operations` goes among a profile's `steps` (each given as its
/// operations, in step order): **the latest place it can run**, which is the end unless
/// the end is too late. `None` when there is no such place.
///
/// The end is the natural place for "+ Add step", and right for most additions. It is
/// wrong once a step already cuts the outline: a new step after that one machines a
/// released board, and appending it there built a profile the operator then had to
/// reorder by hand — or, if they did not notice, ran. So the steps are read as one
/// sequence and the new one goes in the latest slot that keeps the routing rules
/// ([`routing_fault`]): nothing after the outline, only the outline after the cutouts.
///
/// The locating pins are held to their own rule as well: a pins step is the first step
/// (`locating_pin_faults`), so nothing goes in front of one, and a new pins step goes
/// nowhere else.
pub fn step_insert_position(steps: &[&[String]], operations: &[String]) -> Option<usize> {
    let has_pins = |ops: &[String]| ops.iter().any(|op| op == "drill_locating_pins");
    let positions: Vec<usize> = if has_pins(operations) {
        vec![0]
    } else {
        let floor = usize::from(steps.first().is_some_and(|first| has_pins(first)));
        (floor..=steps.len()).rev().collect()
    };
    positions.into_iter().find(|&at| {
        let sequence: Vec<&str> = steps[..at]
            .iter()
            .flat_map(|ops| ops.iter())
            .chain(operations)
            .chain(steps[at..].iter().flat_map(|ops| ops.iter()))
            .map(String::as_str)
            .filter(|key| machining_operation(key).is_some())
            .collect();
        routing_fault(&sequence).is_none()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default order is a valid one — it is where every existing profile starts.
    #[test]
    fn the_board_making_order_is_a_valid_order() {
        let all: Vec<String> = MACHINING_OPERATIONS.iter().map(|op| op.key.to_string()).collect();
        assert_eq!(order_fault(&all), None);
    }

    /// The one real choice: engraving before the holes, or after them.
    #[test]
    fn engraving_may_come_before_or_after_the_holes() {
        assert_eq!(order_fault(&ops(&["isolate_traces", "drill_pth", "drill_npth", "route_board"])), None);
        assert_eq!(order_fault(&ops(&["drill_pth", "drill_npth", "isolate_traces", "route_board"])), None);
        assert!(order_fault(&ops(&["drill_pth", "isolate_traces", "drill_npth"])).is_some(), "not between them");
    }

    /// The outline releases the part, and cutouts weaken it: both are refused anywhere
    /// but the end.
    #[test]
    fn the_routing_stays_at_the_end() {
        assert!(order_fault(&ops(&["route_board", "drill_pth"])).is_some());
        assert!(order_fault(&ops(&["route_board", "route_cutouts"])).is_some());
        assert!(order_fault(&ops(&["route_cutouts", "isolate_traces"])).is_some());
        assert_eq!(order_fault(&ops(&["drill_pth", "route_cutouts", "route_board"])), None);
    }

    /// The holes keep the drills' order: a listed NPTH-before-PTH would be a promise the
    /// drill phase does not keep.
    #[test]
    fn the_holes_keep_their_own_order() {
        assert!(order_fault(&ops(&["drill_npth", "drill_pth"])).is_some());
        assert!(order_fault(&ops(&["drill_pth", "drill_locating_pins"])).is_some());
    }

    /// Moving engraving down jumps the whole hole block — stopping inside it is not an
    /// order — and stops before the routing, which it may not pass.
    #[test]
    fn a_move_jumps_to_the_next_valid_place() {
        let list = ops(&["isolate_traces", "drill_pth", "drill_npth", "route_board"]);
        let after = ops(&["drill_pth", "drill_npth", "isolate_traces", "route_board"]);
        assert_eq!(moved(&list, 0, false), Ok(after.clone()));
        assert_eq!(moved(&after, 2, true), Ok(list.clone()));
        assert!(moved(&after, 2, false).is_err(), "the outline stays last");
        assert!(moved(&list, 1, false).is_err(), "PTH and NPTH keep the drills' order");
        assert_eq!(moved(&list, 0, true), Err("Already first.".to_string()));
    }

    /// Copper clearing sits straight after the isolation it clears for, in the same step.
    #[test]
    fn copper_clearing_follows_its_isolation() {
        assert_eq!(order_fault(&ops(&["isolate_traces", "clear_copper", "drill_pth"])), None);
        assert_eq!(order_fault(&ops(&["drill_pth", "isolate_traces", "clear_copper"])), None);
        assert!(order_fault(&ops(&["clear_copper"])).is_some(), "not without isolation");
        assert!(order_fault(&ops(&["clear_copper", "isolate_traces"])).is_some(), "not before it");
        assert!(
            order_fault(&ops(&["isolate_traces", "drill_pth", "clear_copper"])).is_some(),
            "not with anything between",
        );
    }

    /// The pair moves together, and the clearing cannot be moved away from its isolation.
    #[test]
    fn isolation_moves_with_its_clearing() {
        let before = ops(&["isolate_traces", "clear_copper", "drill_pth", "drill_npth"]);
        let after = ops(&["drill_pth", "drill_npth", "isolate_traces", "clear_copper"]);
        assert_eq!(moved(&before, 0, false), Ok(after.clone()), "the pair jumps the holes");
        assert_eq!(moved(&after, 2, true), Ok(before.clone()), "and back");
        assert!(moved(&before, 1, false).is_err(), "the clearing alone cannot leave its isolation");
        assert!(moved(&after, 3, true).is_err());
    }

    /// The isolation cannot be removed from under its clearing; the clearing can go.
    #[test]
    fn isolation_is_not_removed_from_under_its_clearing() {
        let list = ops(&["isolate_traces", "clear_copper", "drill_pth"]);
        assert!(removal_fault(&list, 0).is_some(), "the clearing would be stranded");
        assert_eq!(removal_fault(&list, 1), None);
        assert_eq!(removal_fault(&list, 2), None);
        assert!(removal_fault(&ops(&["drill_pth"]), 0).is_some(), "a step keeps one operation");
    }

    /// Clearing added to a step lands straight after its isolation, and has nowhere to go
    /// in a step without one.
    #[test]
    fn added_clearing_lands_after_isolation_or_nowhere() {
        let list = ops(&["drill_pth", "isolate_traces", "route_board"]);
        assert_eq!(insert_position(&list, "clear_copper"), Some(2));
        assert_eq!(insert_position(&ops(&["drill_pth", "route_board"]), "clear_copper"), None);
    }

    /// A new step goes at the end unless the end is after the outline; then it goes in the
    /// latest place that is not — and never in front of a pins step.
    #[test]
    fn a_new_step_goes_in_the_latest_place_it_can_run() {
        let drill = ops(&["drill_pth"]);
        let cut = ops(&["route_cutouts"]);
        let outline = ops(&["route_board"]);
        let pins = ops(&["drill_locating_pins"]);
        let engrave = ops(&["isolate_traces"]);

        assert_eq!(step_insert_position(&[&drill], &engrave), Some(1), "the end, when it can run there");
        assert_eq!(step_insert_position(&[&drill, &cut, &outline], &engrave), Some(1), "before the cutouts");
        assert_eq!(step_insert_position(&[&drill, &outline], &cut), Some(1), "before the outline");
        assert_eq!(step_insert_position(&[&pins, &outline], &engrave), Some(1), "after the pins");
        assert_eq!(step_insert_position(&[&drill], &pins), Some(0), "a pins step leads");
        assert_eq!(
            step_insert_position(&[&ops(&["drill_pth", "route_board"])], &cut),
            None,
            "no place: the step that drills also releases the board"
        );
    }

    /// An added operation goes where the board-making order puts it, even in a step the
    /// operator has reordered.
    #[test]
    fn an_added_operation_lands_where_the_board_is_made() {
        let reordered = ops(&["drill_pth", "isolate_traces", "route_board"]);
        assert_eq!(insert_position(&reordered, "drill_npth"), Some(1), "with the holes, not after the copper");
        assert_eq!(insert_position(&reordered, "route_cutouts"), Some(2), "before the outline");
        assert_eq!(insert_position(&ops(&["drill_pth", "route_board"]), "isolate_traces"), Some(0));
        assert_eq!(insert_position(&[], "drill_pth"), Some(0));
    }

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
            step_display_name("   ", &ops(&["drill_pth", "drill_npth", "isolate_traces"])),
            "PTH + NPTH + Isolate",
            "the order the step runs them in, which is the operator's to choose"
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
                "isolate_traces",       // whole, flat, undrilled — and Z0 is verified here
                "clear_copper",         // with the bit isolation leaves in the spindle
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
            ["isolate_traces", "clear_copper", "drill_locating_pins"],
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
        let both = ops(&["isolate_traces", "some_future_operation"]);
        let steps = [("A", false, both.as_slice()), ("B", false, both.as_slice())];
        assert_eq!(blocking_step(steps, 1, "isolate_traces"), None);
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

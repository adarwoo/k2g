//! Isolation contours, computed off the UI thread.
//!
//! Reading every track, pad, via and zone fill off a board and working out where a mill
//! has to run to separate the nets takes a couple of seconds on a dense board. That is far
//! too long to spend inside a render, and it is not work a render should provoke at all:
//! the answer depends on the board and on the cut width, neither of which a repaint
//! changes.
//!
//! So it lives here, on its own worker, modelled on the generation service next door:
//! single-flight, newest request wins, older ones cancelled and their results discarded.
//! A **second** worker rather than a share of that one, because generation is gated on the
//! job being ready to machine while the views want contours to draw regardless.
//!
//! ## Asking for contours
//!
//! Nothing here decides *when* to compute; callers do, and the protocol is the usual one
//! for a derived value that cannot be produced synchronously:
//!
//! ```ignore
//! let spec = IsolationSpec { .. };
//! match ctx.isolation.matching(&spec) {
//!     Some(ready) => use_it(ready),
//!     None => request_isolation(spec),   // and say so until it lands
//! }
//! ```
//!
//! Callers need no guard around that request. Asking twice for the same thing is free —
//! [`request_isolation`] drops a repeat of what it is already working on — and that is
//! what stops the obvious runaway: the worker publishes through `with_ctx_mut`, which
//! re-runs the post-mutation sync, which is where the ask comes from. Without the dedupe
//! each ask would cancel the run about to answer it and nothing would ever finish.
//!
//! The cycle then closes on `matching`, which compares the **whole** spec. A publish that
//! satisfies the ask ends it; one that does not would keep asking, which is correct — a
//! near-match is a wrong answer, not a cheap one.
//!
//! [`request_isolation`] is safe to call from inside `with_ctx_mut` — it takes no context
//! lock, exactly like `enqueue_generation`. Taking one there would deadlock silently,
//! since that guard is held across the entire sync.

// Nothing asks for contours yet — the engrave operation that will is the next piece of
// work, and it lands on top of this rather than beside it. Deliberately in this order:
// wiring the operation first would put a two-second board read on the render path, which
// is the one arrangement that cannot be fixed afterwards without being noticed.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use pcb::{CopperSnapshot, IsolationResult, KiCad};
#[cfg(test)]
use pcb::IsolationContour;

use crate::runtime::{wake_ui, with_ctx_mut};

/// Everything that decides what the contours are.
///
/// Equality here is what tells a stale result from a current one, so it carries the cut
/// width and the floor as well as the board: contours for a 0.4 mm cutter are wrong for a
/// 0.2 mm one, and drawing them would be worse than drawing nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IsolationSpec {
    /// The board as KiCad names it. Not a handle: the worker reconnects, because the
    /// connection is not held anywhere and a board can be closed between ask and answer.
    pub board_name: String,
    /// Which acquisition of that board — [`AppCtx::board_epoch`](crate::runtime::AppCtx).
    ///
    /// The name alone is not identity. Edit the board in KiCad, press Reload PCB, and the
    /// name is exactly what it was; without this the held contours would match and the
    /// operator would be shown, and would machine, the board they had just changed.
    pub board_epoch: u64,
    /// KiCad layer id — `pcb::FRONT_COPPER` or `pcb::BACK_COPPER`.
    pub layer_id: i32,
    /// The cut width the operator asked for, nm.
    pub width_nm: i64,
    /// The narrowest cut the tool can make, nm: a V-bit's tip.
    pub min_width_nm: i64,
}

/// Contours for one layer, and what was wrong with the copper they came from.
#[derive(Clone, Debug, PartialEq)]
pub struct Isolation {
    pub spec: IsolationSpec,
    pub result: IsolationResult,
    /// From reading the copper rather than from isolating it — an unfilled zone, a via
    /// with no ring. Kept apart from `result.warnings` because they are answerable by
    /// different people: these by whoever drew the board, those by whoever picked the bit.
    pub copper_warnings: Vec<String>,
    /// How many copper layers the board has.
    ///
    /// A mill reaches the two outer ones and no others, so a four-layer board engraved
    /// this way is two layers short of the design. That is a thing to say plainly rather
    /// than leave the operator to notice.
    pub copper_layer_count: u32,
}

/// What the context knows about isolation right now.
#[derive(Clone, Debug, Default)]
pub struct IsolationState {
    /// The finished contours, at most one per copper face.
    ///
    /// **Per face, not one slot.** A profile that engraves both sides asks two different
    /// questions, and one slot meant each answer evicted the other: neither step ever
    /// found its contours, every program was emitted with one face's engraving deferred,
    /// and the publish that answered one question triggered the regeneration that asked
    /// the other. It span forever — alternating between two *different*, each incomplete,
    /// programs, so what you would have saved depended on when you looked.
    ///
    /// Keyed by layer because the layer is what alternates. Everything else about the
    /// question is still checked by [`Self::matching`], so a stale entry — a different
    /// width, a reloaded board — misses and is recomputed *in place*. That is what bounds
    /// this to one entry per face rather than one per question ever asked.
    pub ready: BTreeMap<i32, Arc<Isolation>>,
    /// Why the last run produced nothing, if it failed.
    pub error: Option<String>,
}

impl IsolationState {
    /// The held contours, but only if they are for exactly this question.
    ///
    /// The whole spec is compared. A near-match is not a match: contours cut to a
    /// different width describe a different board than the one about to be machined.
    pub fn matching(&self, spec: &IsolationSpec) -> Option<&Arc<Isolation>> {
        self.ready.get(&spec.layer_id).filter(|held| held.spec == *spec)
    }
}

struct IsolationRequest {
    id: u64,
    cancel: Arc<AtomicBool>,
    spec: IsolationSpec,
}

static ISO_TX: OnceLock<std::sync::mpsc::Sender<IsolationRequest>> = OnceLock::new();
static ISO_NEXT_ID: AtomicU64 = AtomicU64::new(1);
static ISO_LATEST_ID: AtomicU64 = AtomicU64::new(0);
static ISO_CURRENT_CANCEL: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);

/// What the worker is computing, if anything.
///
/// Here rather than on the context because [`request_isolation`] cannot take the context
/// lock — it is called from inside `with_ctx_mut`, where that guard is already held. This
/// is what makes the request self-deduplicating, which is the only thing standing between
/// the ask-on-every-sync pattern and a queue that cancels its own answer forever.
static ISO_IN_FLIGHT: Mutex<Option<IsolationSpec>> = Mutex::new(None);

/// The question the worker is busy with, for a view that wants to say "working".
pub fn in_flight() -> Option<IsolationSpec> {
    ISO_IN_FLIGHT.lock().ok().and_then(|held| held.clone())
}

/// Start the worker. Called once from `initialize_ctx`, beside the generation service.
pub fn start_isolation_service() {
    let (tx, rx) = std::sync::mpsc::channel::<IsolationRequest>();
    if ISO_TX.set(tx).is_err() {
        return; // already started
    }
    std::thread::Builder::new()
        .name("k2g-isolation".to_string())
        .spawn(move || isolation_worker(rx))
        .expect("failed to spawn isolation worker thread");
}

fn isolation_worker(rx: std::sync::mpsc::Receiver<IsolationRequest>) {
    while let Ok(request) = rx.recv() {
        if request.id != ISO_LATEST_ID.load(Ordering::SeqCst) {
            continue; // superseded before it started
        }
        let outcome = run_isolation(&request.spec, &request.cancel);
        // Committing a superseded run would put contours for the wrong question on screen.
        // The in-flight marker is *not* cleared here: a newer request already overwrote it
        // with its own spec, and clearing would let the next ask enqueue a duplicate.
        if request.id != ISO_LATEST_ID.load(Ordering::SeqCst)
            || request.cancel.load(Ordering::SeqCst)
        {
            continue;
        }
        clear_in_flight(&request.spec);
        match outcome {
            Ok(isolation) => publish_isolation(isolation),
            Err(message) => publish_isolation_failure(&request.spec, message),
        }
        wake_ui();
    }
}

/// Ask for contours, cancelling whatever the worker was doing.
///
/// **Asking twice for the same thing is free.** Callers ask on every plan, and a plan runs
/// again every time the context moves — including when this very worker publishes. Were
/// the repeat ask to enqueue, it would cancel the run that was about to answer it and the
/// queue would never finish anything. So a request identical to the one in flight is
/// dropped, and callers need no guard of their own.
///
/// Takes no context lock, so it is safe from inside `with_ctx_mut` — see the module note.
/// Silent when the service was never started, which is how the headless tests run.
pub fn request_isolation(spec: IsolationSpec) {
    let Some(tx) = ISO_TX.get() else {
        return;
    };
    {
        let mut in_flight = ISO_IN_FLIGHT.lock().expect("isolation in-flight mutex poisoned");
        if in_flight.as_ref() == Some(&spec) {
            return;
        }
        *in_flight = Some(spec.clone());
    }

    let id = ISO_NEXT_ID.fetch_add(1, Ordering::SeqCst);
    ISO_LATEST_ID.store(id, Ordering::SeqCst);

    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut current = ISO_CURRENT_CANCEL.lock().expect("isolation cancel mutex poisoned");
        if let Some(previous) = current.as_ref() {
            previous.store(true, Ordering::SeqCst);
        }
        *current = Some(Arc::clone(&cancel));
    }
    let _ = tx.send(IsolationRequest { id, cancel, spec });
}

/// Forget the in-flight question, so the next ask for it goes through.
///
/// Called once a run has finished with it — answered or failed. A failure must clear it
/// too, or a board that was briefly unreachable would never be asked about again.
fn clear_in_flight(spec: &IsolationSpec) {
    if let Ok(mut in_flight) = ISO_IN_FLIGHT.lock() {
        if in_flight.as_ref() == Some(spec) {
            *in_flight = None;
        }
    }
}

/// Read the copper and isolate it. Runs on the worker; touches no context.
fn run_isolation(spec: &IsolationSpec, cancel: &Arc<AtomicBool>) -> Result<Isolation, String> {
    if cancel.load(Ordering::SeqCst) {
        return Err("cancelled".into());
    }
    let (copper, copper_layer_count) = collect_copper(spec)?;
    // Between the two halves is the only place a cancel can land: reading the board is one
    // IPC round trip and isolating is one call, neither of which reports progress.
    if cancel.load(Ordering::SeqCst) {
        return Err("cancelled".into());
    }

    let started = std::time::Instant::now();
    let result = pcb::isolate(&copper, spec.width_nm, spec.min_width_nm);
    log::info!(
        "Isolated layer {} of {} in {:?}: {} contour(s), {} narrowed pair(s)",
        spec.layer_id,
        spec.board_name,
        started.elapsed(),
        result.contours.len(),
        result.narrowed.len(),
    );

    Ok(Isolation {
        spec: spec.clone(),
        result,
        copper_warnings: copper.warnings,
        copper_layer_count,
    })
}

/// The copper on the spec's layer, and how many copper layers the board has.
///
/// A fresh connection per run, like [`acquire_board`](crate::runtime::acquire_board): no
/// client is held anywhere, and a board that has been closed since the ask must fail here
/// rather than return the copper it used to have.
///
/// **The zones are re-poured first, every time.** A fill that has gone stale is copper
/// that is not there, and engraving against it cuts a board nobody drew. It costs a pause
/// in KiCad, which answers `AS_BUSY` until the fill completes — affordable because the
/// spec is keyed on the board's epoch, so this runs when the board is re-acquired or the
/// width changes, not on the way to every repaint.
fn collect_copper(spec: &IsolationSpec) -> Result<(CopperSnapshot, u32), String> {
    let client = KiCad::connect().map_err(|err| format!("KiCad is not reachable: {err}"))?;
    let pcbs = client
        .enumerate_pcbs()
        .map_err(|err| format!("KiCad would not list its open boards: {err}"))?;
    let board = pcbs
        .into_iter()
        .find(|pcb| pcb.display_name() == spec.board_name)
        .ok_or_else(|| format!("{} is no longer open in KiCad.", spec.board_name))?;

    // Not fatal: a board whose layer count will not come back is still engravable, it just
    // cannot be warned about for having more copper than a mill can reach.
    let copper_layer_count = client.copper_layer_count(&board).unwrap_or(0);

    let copper = client
        .collect_copper(&board, spec.layer_id, true)
        .map_err(|err| format!("The copper on layer {} could not be read: {err}", spec.layer_id))?;
    Ok((copper, copper_layer_count))
}

/// How much of a board must take the requested channel for the width to be sensible.
///
/// A contour only stays a whole loop if nothing forced it to narrow, so this is really
/// "did the width fit". A third is a low bar deliberately — a board with genuinely tight
/// corners still clears it comfortably (0.25 mm on the board this was built against comes
/// out at two thirds) while a width the layout cannot take anywhere falls far below it
/// (0.8 mm on the same board: one fortieth).
const MIN_INTACT_FRACTION: f64 = 1.0 / 3.0;

/// What is wrong with a set of contours, if anything, as a standing diagnostic.
///
/// Two faults, and they are not degrees of one thing.
///
/// **Copper left joined** is the one that matters: the ladder ran out of widths before it
/// ran out of boundary, so there is copper on this board that no cut separates. That is a
/// board which will not work, and it is raised however small the stretch is.
///
/// **A width the layout cannot take** is about the width rather than the board: the pass
/// narrows nearly everything and returns thousands of short fragments instead of outlines.
/// That is not a wrong answer — it is the honest one — but it is not a board anybody meant
/// to cut, and nothing else about the result says so. The contour count goes *up*, which
/// reads like more work rather than less.
///
/// The first cannot be found by looking at the second. `intact_fraction` is a ratio over
/// the contours that exist, and a contour that was never cut is not among them — a board
/// can score a perfect fraction and still be joined. Reading only that ratio is exactly how
/// a silently uncut board once passed every check this application makes.
fn isolation_faults(isolation: &Isolation) -> Vec<(String, Option<String>)> {
    let result = &isolation.result;
    let mut faults = Vec::new();

    if !result.uncut.is_empty() {
        let total: f64 = result.uncut.iter().map(|u| u.length_nm).sum();
        let mut worst: Vec<&pcb::UncutStretch> = result.uncut.iter().collect();
        worst.sort_by(|a, b| b.length_nm.total_cmp(&a.length_nm));
        faults.push((
            format!("{:.2} mm of this board's copper cannot be separated.", total / 1e6),
            Some(format!(
                "No width down to the bit's own tip fits between them, so nothing was cut \
                 there and those nets stay joined — worst on {}. Reduce the isolation \
                 width, fit a bit with a finer tip, or re-lay the board.",
                worst
                    .iter()
                    .take(UNCUT_NETS_NAMED)
                    .map(|u| format!("{} ({:.2} mm)", u.net, u.length_nm / 1e6))
                    .collect::<Vec<_>>()
                    .join(", "),
            )),
        ));
    }

    let intact = result.intact_fraction();
    if !result.contours.is_empty() && intact < MIN_INTACT_FRACTION {
        let closed = result.contours.iter().filter(|c| c.closed).count();
        let asked = isolation.spec.width_nm as f64 / 1e6;
        let advice = match result.widest_workable_nm() {
            Some(fits) => format!(
                "The widest channel this board's clearances allow throughout is about \
                 {:.2} mm. Set the isolation width to that.",
                fits as f64 / 1e6,
            ),
            None => "Reduce the isolation width.".to_string(),
        };

        faults.push((
            format!("An isolation channel of {asked:.2} mm does not fit this board."),
            Some(format!(
                "Only {closed} of {} contours could take it; the rest were broken into \
                 narrowed fragments, which is thousands of short cuts rather than an outline \
                 round each net. {advice}",
                result.contours.len(),
            )),
        ));
    }

    faults
}

/// How many nets the uncut fault names before it gives up and states the total.
const UNCUT_NETS_NAMED: usize = 5;

fn publish_isolation(isolation: Isolation) {
    let faults = isolation_faults(&isolation);
    for (headline, _) in &faults {
        log::warn!("{headline}");
    }
    with_ctx_mut(|ctx| {
        ctx.isolation.error = None;
        ctx.app.set_isolation_errors(faults);
        ctx.isolation.ready.insert(isolation.spec.layer_id, Arc::new(isolation));
        // What tells the regeneration trigger that this happened. It diffs the app state
        // and the job's references, and this is on neither.
        ctx.isolation_epoch = ctx.isolation_epoch.wrapping_add(1);
    });
}

fn publish_isolation_failure(spec: &IsolationSpec, message: String) {
    log::warn!("Isolation failed for layer {} — {message}", spec.layer_id);
    with_ctx_mut(|ctx| {
        // The previous result is *not* cleared. It was correct for what it was computed
        // for, and `matching` will refuse it for anything else anyway — so keeping it
        // costs nothing and spares the operator a view that empties every time KiCad is
        // briefly busy.
        ctx.isolation.error = Some(message);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(width_nm: i64) -> IsolationSpec {
        IsolationSpec {
            board_name: "demo".into(),
            board_epoch: 1,
            layer_id: pcb::FRONT_COPPER,
            width_nm,
            min_width_nm: 100_000,
        }
    }

    fn ready(spec: IsolationSpec) -> IsolationState {
        let mut state = IsolationState::default();
        hold(&mut state, spec);
        state
    }

    /// Adds one finished result to `state`, as `publish_isolation` does.
    fn hold(state: &mut IsolationState, spec: IsolationSpec) {
        state.ready.insert(
            spec.layer_id,
            Arc::new(Isolation {
                spec,
                result: IsolationResult::default(),
                copper_warnings: Vec::new(),
                copper_layer_count: 2,
            }),
        );
    }

    /// A profile that engraves both faces asks two questions, and the answers must not
    /// evict each other.
    ///
    /// This is the loop that shipped: one slot held "the last result, whatever it was
    /// computed for", so the back face's contours displaced the front's and the front's
    /// displaced the back's. Neither engrave step ever found what it needed, so every
    /// generation deferred one face — and the publish that answered one question woke the
    /// regeneration that asked the other, round and round, emitting a different
    /// half-finished program each way round.
    #[test]
    fn both_faces_are_held_at_once() {
        let front = spec(400_000);
        let back = IsolationSpec { layer_id: pcb::BACK_COPPER, ..spec(400_000) };

        let mut state = IsolationState::default();
        hold(&mut state, front.clone());
        hold(&mut state, back.clone());

        assert!(state.matching(&front).is_some(), "the front face survived the back's arrival");
        assert!(state.matching(&back).is_some(), "and the back face is there too");
    }

    /// Bounded to one entry per face: re-asking the same face a different way replaces
    /// that face's entry rather than accumulating one per question ever asked.
    #[test]
    fn a_new_answer_for_a_face_replaces_the_old_one() {
        let mut state = IsolationState::default();
        hold(&mut state, spec(400_000));
        hold(&mut state, spec(200_000));

        assert_eq!(state.ready.len(), 1, "one face, one entry");
        assert!(state.matching(&spec(200_000)).is_some(), "the newest answer is the held one");
        assert!(
            state.matching(&spec(400_000)).is_none(),
            "and the superseded width is not offered for a job that no longer asks for it"
        );
    }

    fn contour(closed: bool) -> IsolationContour {
        IsolationContour {
            net: "GND".into(),
            path: vec![(0, 0), (1000, 0), (1000, 1000)],
            closed,
            width_nm: 200_000,
        }
    }

    fn outcome(closed: usize, open: usize, narrowed: &[i64]) -> Isolation {
        Isolation {
            spec: spec(800_000),
            result: IsolationResult {
                layer_id: pcb::FRONT_COPPER,
                contours: std::iter::repeat_with(|| contour(true))
                    .take(closed)
                    .chain(std::iter::repeat_with(|| contour(false)).take(open))
                    .collect(),
                narrowed: narrowed
                    .iter()
                    .enumerate()
                    .map(|(i, w)| pcb::NarrowedPair {
                        nets: (format!("A{i}"), format!("B{i}")),
                        width_nm: *w,
                    })
                    .collect(),
                uncut: Vec::new(),
                warnings: Vec::new(),
            },
            copper_warnings: Vec::new(),
            copper_layer_count: 2,
        }
    }

    /// [`outcome`], with copper the ladder could not separate at any width.
    fn with_uncut(closed: usize, uncut_mm: f64) -> Isolation {
        let mut isolation = outcome(closed, 0, &[]);
        isolation.result.uncut = vec![pcb::UncutStretch {
            net: "GND".into(),
            length_nm: uncut_mm * 1e6,
        }];
        isolation
    }

    /// The fault this exists to catch. Asked for a channel the layout cannot take, the
    /// pass narrows nearly everything and returns thousands of short fragments — which is
    /// the honest answer to the question asked, and not a board anyone meant to cut. The
    /// contour count goes *up*, so nothing else about the result reads as a problem.
    #[test]
    fn a_width_that_shatters_the_board_is_reported_as_a_fault() {
        let faults = isolation_faults(&outcome(89, 3327, &[200_000, 250_000]));

        assert_eq!(faults.len(), 1);
        let (headline, detail) = &faults[0];
        assert!(headline.contains("0.80 mm"), "the width asked for: {headline}");
        let detail = detail.clone().unwrap_or_default();
        assert!(detail.contains("89 of 3416"), "how little fit: {detail}");
        assert!(
            detail.contains("0.20 mm"),
            "and the width that would, which is the tightest fallback: {detail}"
        );
    }

    /// A board with genuinely tight corners is not the same thing as a width that does not
    /// fit, and must not be refused. Two thirds intact is what a sensible width looks like
    /// on the board this was built against.
    #[test]
    fn a_board_that_mostly_takes_the_width_is_not_a_fault() {
        assert!(isolation_faults(&outcome(234, 115, &[200_000])).is_empty());
    }

    /// Nothing narrowed means nothing to suggest, and no fault to raise either.
    #[test]
    fn a_clean_pass_raises_nothing() {
        assert!(isolation_faults(&outcome(349, 0, &[])).is_empty());
    }

    /// **Copper left joined is a fault whatever the contours look like.**
    ///
    /// The tell-tale here is that every contour is a tidy closed loop — a perfect intact
    /// fraction — and the board is still not isolated. A stretch that was never cut leaves
    /// no contour behind to be counted as broken, so the ratio cannot see it. Reading only
    /// that ratio is how a board came off the machine joined with nothing on screen.
    #[test]
    fn copper_that_could_not_be_separated_is_a_fault_on_an_otherwise_perfect_pass() {
        let isolation = with_uncut(349, 1.25);
        assert_eq!(isolation.result.intact_fraction(), 1.0, "every contour is a whole loop");

        let faults = isolation_faults(&isolation);

        assert_eq!(faults.len(), 1, "the joined copper, and nothing else");
        let (headline, detail) = &faults[0];
        assert!(headline.contains("1.25 mm"), "how much: {headline}");
        assert!(headline.contains("cannot be separated"), "{headline}");
        let detail = detail.clone().unwrap_or_default();
        assert!(detail.contains("GND"), "which net: {detail}");
        assert!(detail.contains("stay joined"), "what it means: {detail}");
    }

    /// The two faults are independent, so a board can have both and must be told both: one
    /// says the width shatters the outlines, the other that some copper is not cut at all.
    #[test]
    fn a_shattered_board_with_joined_copper_raises_both_faults() {
        let mut isolation = outcome(89, 3327, &[200_000]);
        isolation.result.uncut =
            vec![pcb::UncutStretch { net: "GND".into(), length_nm: 500_000.0 }];

        let faults = isolation_faults(&isolation);
        let headlines: Vec<&str> = faults.iter().map(|(h, _)| h.as_str()).collect();

        assert_eq!(headlines.len(), 2, "{headlines:?}");
        assert!(headlines[0].contains("cannot be separated"), "the joined copper leads");
        assert!(headlines[1].contains("does not fit this board"));
    }

    /// A pair that can take no width at all must not be offered as the answer: it would
    /// suggest setting the channel to nothing.
    #[test]
    fn a_pair_that_fits_nothing_is_not_suggested_as_the_width() {
        let faults = isolation_faults(&outcome(10, 990, &[0, 150_000]));
        let detail = faults[0].1.clone().unwrap_or_default();
        assert!(detail.contains("0.15 mm"), "{detail}");
        assert!(!detail.contains("0.00 mm"), "{detail}");
    }

    /// Contours cut to one width describe a different board than contours cut to another.
    /// Handing back a near-match would put a toolpath on screen that the machine will not
    /// follow.
    #[test]
    fn held_contours_are_refused_for_a_different_question() {
        let state = ready(spec(400_000));
        assert!(state.matching(&spec(400_000)).is_some());
        assert!(state.matching(&spec(200_000)).is_none(), "a different width is a different job");
    }

    /// The board's *name* does not change when the operator edits it and reloads, so the
    /// name alone would hand back contours for copper that has since moved. Only the epoch
    /// tells one acquisition from the next.
    #[test]
    fn held_contours_are_refused_after_the_board_is_re_read() {
        let state = ready(spec(400_000));
        let reloaded = IsolationSpec { board_epoch: 2, ..spec(400_000) };
        assert!(state.matching(&reloaded).is_none());
    }

    /// A failed run must not leave the question looking answered, or the operator would be
    /// stuck with no contours and no way to provoke another attempt.
    #[test]
    fn a_failure_leaves_the_question_unanswered() {
        let state = IsolationState {
            ready: BTreeMap::new(),
            error: Some("KiCad is not reachable".into()),
        };
        assert!(state.matching(&spec(400_000)).is_none());
    }
}

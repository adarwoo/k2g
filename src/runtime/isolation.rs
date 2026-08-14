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
//!     None => request_isolation(spec),   // and draw nothing until it lands
//! }
//! ```
//!
//! The `matching` check is what stops the obvious infinite loop: the worker publishes
//! through `with_ctx_mut`, which re-runs the post-mutation sync, which is where the ask
//! comes from. A publish that satisfies the spec ends the cycle; one that does not would
//! spin, which is why [`IsolationState::matching`] compares the *whole* spec and not just
//! the board.
//!
//! [`request_isolation`] is safe to call from inside `with_ctx_mut` — it takes no context
//! lock, exactly like `enqueue_generation`. Taking one there would deadlock silently,
//! since that guard is held across the entire sync.

// Nothing asks for contours yet — the engrave operation that will is the next piece of
// work, and it lands on top of this rather than beside it. Deliberately in this order:
// wiring the operation first would put a two-second board read on the render path, which
// is the one arrangement that cannot be fixed afterwards without being noticed.
#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use pcb::{CopperSnapshot, IsolationResult, KiCad};

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
    /// KiCad layer id — `pcb::FRONT_COPPER` or `pcb::BACK_COPPER`.
    pub layer_id: i32,
    /// The cut width the operator asked for, nm.
    pub width_nm: i64,
    /// The narrowest cut the tool can make, nm: a V-bit's tip.
    pub min_width_nm: i64,
    /// Whether to have KiCad re-pour its zones first.
    ///
    /// Worth doing before cutting metal and worth *not* doing on the way to a preview: it
    /// blocks KiCad, which answers `AS_BUSY` to everything until the fill completes.
    pub refill: bool,
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
}

/// What the context knows about isolation right now.
#[derive(Clone, Debug, Default)]
pub struct IsolationState {
    /// What the worker is computing, if anything. Present so a view can say "working"
    /// rather than "nothing here".
    pub pending: Option<IsolationSpec>,
    /// The last finished result, whatever it was computed for.
    pub ready: Option<Arc<Isolation>>,
    /// Why the last run produced nothing, if it failed.
    pub error: Option<String>,
}

impl IsolationState {
    /// The held contours, but only if they are for exactly this question.
    ///
    /// The whole spec is compared. A near-match is not a match: contours cut to a
    /// different width describe a different board than the one about to be machined.
    pub fn matching(&self, spec: &IsolationSpec) -> Option<&Arc<Isolation>> {
        self.ready.as_ref().filter(|held| held.spec == *spec)
    }

    /// Whether this spec is neither answered nor already being worked on.
    ///
    /// Callers gate [`request_isolation`] on this. Without the `pending` half, every sync
    /// while the worker ran would enqueue the same job again and cancel the run that was
    /// about to answer it — a queue that never finishes anything.
    pub fn wants(&self, spec: &IsolationSpec) -> bool {
        self.matching(spec).is_none() && self.pending.as_ref() != Some(spec)
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
        // Committing a superseded run would put the wrong contours on screen and, worse,
        // clear the `pending` marker for a request that is still coming.
        if request.id != ISO_LATEST_ID.load(Ordering::SeqCst)
            || request.cancel.load(Ordering::SeqCst)
        {
            continue;
        }
        match outcome {
            Ok(isolation) => publish_isolation(isolation),
            Err(message) => publish_isolation_failure(&request.spec, message),
        }
        wake_ui();
    }
}

/// Ask for contours, cancelling whatever the worker was doing.
///
/// Takes no context lock, so it is safe from inside `with_ctx_mut` — see the module note.
/// Silent when the service was never started, which is how the headless tests run.
pub fn request_isolation(spec: IsolationSpec) {
    let Some(tx) = ISO_TX.get() else {
        return;
    };
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

/// Read the copper and isolate it. Runs on the worker; touches no context.
fn run_isolation(spec: &IsolationSpec, cancel: &Arc<AtomicBool>) -> Result<Isolation, String> {
    if cancel.load(Ordering::SeqCst) {
        return Err("cancelled".into());
    }
    let copper = collect_copper(spec)?;
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

    Ok(Isolation { spec: spec.clone(), result, copper_warnings: copper.warnings })
}

/// The copper on the spec's layer, reconnecting to KiCad to get it.
///
/// A fresh connection per run, like [`acquire_board`](crate::runtime::acquire_board): no
/// client is held anywhere, and a board that has been closed since the ask must fail here
/// rather than return the copper it used to have.
fn collect_copper(spec: &IsolationSpec) -> Result<CopperSnapshot, String> {
    let client = KiCad::connect().map_err(|err| format!("KiCad is not reachable: {err}"))?;
    let pcbs = client
        .enumerate_pcbs()
        .map_err(|err| format!("KiCad would not list its open boards: {err}"))?;
    let board = pcbs
        .into_iter()
        .find(|pcb| pcb.display_name() == spec.board_name)
        .ok_or_else(|| format!("{} is no longer open in KiCad.", spec.board_name))?;

    client
        .collect_copper(&board, spec.layer_id, spec.refill)
        .map_err(|err| format!("The copper on layer {} could not be read: {err}", spec.layer_id))
}

fn publish_isolation(isolation: Isolation) {
    with_ctx_mut(|ctx| {
        ctx.isolation.pending = None;
        ctx.isolation.error = None;
        ctx.isolation.ready = Some(Arc::new(isolation));
    });
}

fn publish_isolation_failure(spec: &IsolationSpec, message: String) {
    log::warn!("Isolation failed for layer {} — {message}", spec.layer_id);
    with_ctx_mut(|ctx| {
        ctx.isolation.pending = None;
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
            layer_id: pcb::FRONT_COPPER,
            width_nm,
            min_width_nm: 100_000,
            refill: false,
        }
    }

    fn ready(spec: IsolationSpec) -> IsolationState {
        IsolationState {
            pending: None,
            ready: Some(Arc::new(Isolation {
                spec,
                result: IsolationResult::default(),
                copper_warnings: Vec::new(),
            })),
            error: None,
        }
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

    /// The guard against a queue that never finishes: every post-mutation sync asks again
    /// while the worker runs, and each ask cancels the run that was about to answer it.
    #[test]
    fn a_question_already_being_worked_on_is_not_asked_again() {
        let state = IsolationState { pending: Some(spec(400_000)), ..Default::default() };
        assert!(!state.wants(&spec(400_000)));
        assert!(state.wants(&spec(200_000)), "but a different question still needs asking");
    }

    /// The other half of the same loop: once the answer lands, the ask has to stop.
    #[test]
    fn an_answered_question_is_not_asked_again() {
        assert!(!ready(spec(400_000)).wants(&spec(400_000)));
    }

    /// A failed run must not leave the spec looking answered, or the operator would be
    /// stuck with no contours and no way to provoke another attempt.
    #[test]
    fn a_failure_leaves_the_question_open() {
        let state = IsolationState {
            pending: None,
            ready: None,
            error: Some("KiCad is not reachable".into()),
        };
        assert!(state.wants(&spec(400_000)));
    }
}

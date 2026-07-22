// GCode generation service — the off-UI-thread pipeline from
// `docs/gcode-generation.md` §6–8. A single-flight OS worker thread consumes a
// queue: a new request cancels the in-flight run (an `Arc<AtomicBool>` checked at
// checkpoints), results are id-tagged so stale runs are discarded, and the worker
// publishes into the global ctx and bumps a wake channel so the UI re-syncs.
//
// This file is `include!`d into `runtime/mod.rs`, so it shares that module's
// imports (`with_ctx_mut`, `GenerationState`, `BoardSnapshot`, `StitchResult`,
// `OnceLock`, …); new std/tokio types are fully qualified to avoid touching them.

/// Immutable snapshot of everything a generation run needs, captured at enqueue
/// time so edits made *during* a run cannot corrupt it. Lean for now — it grows as
/// the OperationPlanner + Coder land; today it feeds the placeholder run.
#[derive(Clone)]
pub struct GenerationInput {
    pub board: Option<BoardSnapshot>,
    pub stitched: Option<StitchResult>,
    pub process_profile_name: String,
    pub cnc_profile_name: String,
    pub operations: Vec<String>,
}

/// A successful run's output, published atomically into `AppState`.
struct GenerationOutput {
    gcode: String,
    summary: String,
}

/// Why a run produced no output.
enum GenerationAbort {
    /// Superseded by a newer request — discard silently.
    Cancelled,
    /// The run failed — clear outputs and surface the message.
    Failed(String),
}

/// One unit of work handed to the worker.
struct GenerationRequest {
    id: u64,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    input: GenerationInput,
}

static GEN_TX: OnceLock<std::sync::mpsc::Sender<GenerationRequest>> = OnceLock::new();
static GEN_NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
static GEN_LATEST_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static GEN_CURRENT_CANCEL: std::sync::Mutex<Option<std::sync::Arc<std::sync::atomic::AtomicBool>>> =
    std::sync::Mutex::new(None);
static UI_WAKE: OnceLock<tokio::sync::watch::Sender<u64>> = OnceLock::new();

/// Start the background worker and the UI-wake channel. Called once from
/// `initialize_ctx`, after the global ctx is installed.
pub fn start_generation_service() {
    let (tx, rx) = std::sync::mpsc::channel::<GenerationRequest>();
    if GEN_TX.set(tx).is_err() {
        return; // already started
    }
    let (wake_tx, _seed_rx) = tokio::sync::watch::channel(0u64);
    let _ = UI_WAKE.set(wake_tx);

    std::thread::Builder::new()
        .name("k2g-generation".to_string())
        .spawn(move || generation_worker(rx))
        .expect("failed to spawn generation worker thread");
}

/// The worker loop: process requests one at a time, newest wins.
fn generation_worker(rx: std::sync::mpsc::Receiver<GenerationRequest>) {
    use std::sync::atomic::Ordering;
    while let Ok(request) = rx.recv() {
        // Skip a request already superseded before it even started.
        if request.id != GEN_LATEST_ID.load(Ordering::SeqCst) {
            continue;
        }
        match run_generation(&request.input, &request.cancel) {
            Ok(output) => {
                // Commit only if this is still the latest, uncancelled run.
                if request.id == GEN_LATEST_ID.load(Ordering::SeqCst)
                    && !request.cancel.load(Ordering::SeqCst)
                {
                    publish_success(output);
                    wake_ui();
                }
            }
            Err(GenerationAbort::Cancelled) => { /* superseded — discard */ }
            Err(GenerationAbort::Failed(message)) => {
                if request.id == GEN_LATEST_ID.load(Ordering::SeqCst) {
                    publish_failure(&message);
                    wake_ui();
                }
            }
        }
    }
}

/// Enqueue a generation request, cancelling any in-flight run. Non-blocking and
/// lock-free w.r.t. the ctx — it is called from inside `with_ctx_mut`, so it must
/// never re-take that lock.
fn enqueue_generation(input: GenerationInput) {
    use std::sync::atomic::Ordering;
    let Some(tx) = GEN_TX.get() else {
        return; // service not started (e.g. headless tests)
    };
    let id = GEN_NEXT_ID.fetch_add(1, Ordering::SeqCst);
    GEN_LATEST_ID.store(id, Ordering::SeqCst);

    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let mut current = GEN_CURRENT_CANCEL.lock().expect("cancel mutex poisoned");
        if let Some(previous) = current.as_ref() {
            previous.store(true, Ordering::SeqCst); // cancel the in-flight run
        }
        *current = Some(cancel.clone());
    }
    let _ = tx.send(GenerationRequest { id, cancel, input });
}

/// Produce the program for one request. **Placeholder** until the OperationPlanner
/// and Coder land: it emits a deterministic header program and honours the cancel
/// flag at checkpoints, so the worker/cancellation contract is exercised now.
fn run_generation(
    input: &GenerationInput,
    cancel: &std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<GenerationOutput, GenerationAbort> {
    use std::sync::atomic::Ordering;

    if cancel.load(Ordering::SeqCst) {
        return Err(GenerationAbort::Cancelled);
    }

    // The generator validates its own preconditions rather than trusting the gate;
    // an empty operation set has nothing to machine, so the run fails and the
    // program is cleared (§7).
    if input.operations.is_empty() {
        return Err(GenerationAbort::Failed(
            "no machining operations selected".to_string(),
        ));
    }

    let hole_count = input.board.as_ref().map(|board| board.holes.len()).unwrap_or(0);
    let contour_count = input
        .stitched
        .as_ref()
        .map(|stitched| stitched.contours.len())
        .unwrap_or(0);
    let operations = input.operations.join(", ");

    let mut gcode = String::new();
    gcode.push_str("(k2g generated program — placeholder)\n");
    gcode.push_str(&format!("(machining profile: {})\n", input.process_profile_name));
    gcode.push_str(&format!("(cnc profile: {})\n", input.cnc_profile_name));
    gcode.push_str(&format!("(board: {hole_count} holes, {contour_count} contours)\n"));
    gcode.push_str(&format!("(operations: {operations})\n"));
    gcode.push_str("M2\n");

    if cancel.load(Ordering::SeqCst) {
        return Err(GenerationAbort::Cancelled);
    }

    let summary = format!(
        "Generated {} operation(s), {} line(s)",
        input.operations.len(),
        gcode.lines().count()
    );
    Ok(GenerationOutput { gcode, summary })
}

/// Commit a successful run into the ctx and settle to Idle.
fn publish_success(output: GenerationOutput) {
    with_ctx_mut(|ctx| {
        ctx.app.generation_state = GenerationState::Idle;
        ctx.app.gcode = output.gcode;
        ctx.app.gcode_modified = false;
        ctx.app.log_event(output.summary);
    });
}

/// Commit a failure: clear all derived outputs (a live tool must never show a
/// stale program) and surface the diagnostic.
fn publish_failure(message: &str) {
    let message = message.to_string();
    with_ctx_mut(|ctx| {
        ctx.app.generation_state = GenerationState::Failed;
        ctx.app.gcode.clear();
        ctx.app.gcode_modified = false;
        ctx.app.log_event(format!("Generation failed: {message}"));
    });
}

/// Bump the UI-wake channel so the front-end re-syncs its ctx snapshot. Called
/// after every publish (the worker mutates the ctx off the UI thread, which the
/// UI cannot observe on its own).
fn wake_ui() {
    if let Some(sender) = UI_WAKE.get() {
        sender.send_modify(|counter| *counter = counter.wrapping_add(1));
    }
}

/// A receiver the UI awaits to learn when the worker has published new state.
/// `None` until the service is started. `tokio::sync::watch` needs no tokio
/// runtime (it only drives standard wakers), so it works under Dioxus's executor.
pub fn ui_wake_receiver() -> Option<tokio::sync::watch::Receiver<u64>> {
    UI_WAKE.get().map(|sender| sender.subscribe())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    fn sample_input() -> GenerationInput {
        GenerationInput {
            board: None,
            stitched: None,
            process_profile_name: "Proto".to_string(),
            cnc_profile_name: "Genmitsu".to_string(),
            operations: vec!["Drill PTH".to_string(), "Route outline".to_string()],
        }
    }

    #[test]
    fn placeholder_run_is_deterministic_and_summarises() {
        let cancel = Arc::new(AtomicBool::new(false));
        let a = run_generation(&sample_input(), &cancel);
        let b = run_generation(&sample_input(), &cancel);
        let (a, b) = (a.ok().unwrap(), b.ok().unwrap());
        assert_eq!(a.gcode, b.gcode, "same input must yield identical program");
        assert!(a.gcode.contains("(machining profile: Proto)"));
        assert!(a.gcode.contains("(operations: Drill PTH, Route outline)"));
        assert!(a.summary.contains("2 operation(s)"));
    }

    #[test]
    fn a_cancelled_run_aborts_at_the_first_checkpoint() {
        let cancel = Arc::new(AtomicBool::new(true));
        match run_generation(&sample_input(), &cancel) {
            Err(GenerationAbort::Cancelled) => {}
            _ => panic!("a pre-cancelled run must abort"),
        }
    }

    #[test]
    fn an_empty_operation_set_fails_the_run() {
        let cancel = Arc::new(AtomicBool::new(false));
        let mut input = sample_input();
        input.operations.clear();
        match run_generation(&input, &cancel) {
            Err(GenerationAbort::Failed(message)) => assert!(message.contains("operations")),
            _ => panic!("a run with no operations must fail"),
        }
    }

    #[test]
    fn wake_and_enqueue_are_safe_before_the_service_starts() {
        // No service (GEN_TX/UI_WAKE unset in a plain unit test) → both are no-ops,
        // never a panic. `ui_wake_receiver` yields nothing.
        wake_ui();
        enqueue_generation(sample_input());
        assert!(ui_wake_receiver().is_none());
    }
}

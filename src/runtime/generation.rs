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
    pub process_profile_name: String,
    pub operations: Vec<String>,
    pub filename: String,
    pub timestamp: String,
    // The resolved drill plan (one entry per machining step), the per-step program
    // context, and the tool→feed/speed lookup, all built on the main thread (the worker
    // has no ctx access). `steps[i]` matches `plan.steps[i]`.
    //
    // Everything the program layer needs lives in `steps` rather than out here, because
    // a step owns its CNC and its fixture: the header's templates, its `z_safe` retract
    // and its work coordinate system all differ between steps of the same job.
    pub plan: crate::gcode::plan::MachiningPlan,
    pub steps: Vec<crate::gcode::program::ProgramRender>,
    /// The machining profile's own `steps` array, in order, as the program templates read
    /// it — every step's record, not just the one being rendered, so a header can say
    /// "step 2 of 3". Aligned with `plan.steps[i]`, which is what `step_index` indexes.
    ///
    /// Held apart from `steps` above because the two are different things wearing similar
    /// names: that one is the *machine's* render context (templates, retract height),
    /// this one is the *operator's* record of what the step is for.
    pub step_values: Vec<crate::gcode::step_data::StepValue>,
    pub tool_feeds: std::collections::BTreeMap<String, crate::gcode::program::ToolFeed>,
}

/// One machining step's program, or why it has none.
#[derive(Clone, PartialEq)]
pub struct StepProgram {
    pub index: usize,
    pub name: String,
    pub cnc_name: String,
    pub outcome: ProgramOutcome,
}

/// A step fails **alone**. Steps are independent programs run in separate setups, so
/// withholding two correct programs because a third step's router has no feed rate would
/// punish the operator for a fault in work they were not about to do.
#[derive(Clone, PartialEq)]
pub enum ProgramOutcome {
    Ready(Program),
    Failed(String),
}

/// A finished program, with what the UI needs to describe it without re-parsing.
#[derive(Clone, PartialEq)]
pub struct Program {
    pub text: String,
    /// From the step's CNC, so an Excellon step saves as `.drl` rather than `.nc`.
    pub extension: String,
    pub block_count: usize,
    pub op_count: usize,
}

impl StepProgram {
    /// The program, when this step produced one.
    pub fn program(&self) -> Option<&Program> {
        match &self.outcome {
            ProgramOutcome::Ready(program) => Some(program),
            ProgramOutcome::Failed(_) => None,
        }
    }

    /// Why this step produced nothing, when it failed.
    pub fn failure(&self) -> Option<&str> {
        match &self.outcome {
            ProgramOutcome::Failed(message) => Some(message),
            ProgramOutcome::Ready(_) => None,
        }
    }
}

/// A successful run's output, published atomically into `AppState`.
struct GenerationOutput {
    steps: Vec<StepProgram>,
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

/// Produce the program for one request. **Header phase:** the program is the
/// CNC's real `initialise` and `conclude` primitives rendered through the Coder,
/// with an (as-yet empty) machining body between them. The body sections — tool
/// changes, drilling, routing — are filled in by later phases. The cancel flag is
/// honoured at checkpoints, exercising the worker/cancellation contract.
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

    // One program per step (§9.2). Each renders through its own `Coder` from its own
    // CNC's templates, so a step cannot inherit the previous one's modal unit state or
    // its fixture's safe height — see `render_step_program`.
    // Built once: it is the same for every step, and it is `step_index` that moves.
    let program_ctx = crate::gcode::program::ProgramContext {
        filename: &input.filename,
        timestamp: &input.timestamp,
        steps: &input.step_values,
    };

    let mut steps: Vec<StepProgram> = Vec::with_capacity(input.plan.steps.len());
    for (index, step) in input.plan.steps.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            return Err(GenerationAbort::Cancelled);
        }
        let name = step.name.clone();
        let Some(render) = input.steps.get(index) else {
            // A planned step with no render context has no CNC to render through. Said
            // out loud rather than skipped: a program that silently fails to exist is the
            // worst outcome available.
            steps.push(StepProgram {
                index,
                name,
                cnc_name: String::new(),
                outcome: ProgramOutcome::Failed(
                    "the step has no CNC profile to render through".to_string(),
                ),
            });
            continue;
        };
        let outcome = match crate::gcode::program::render_step_program(
            step,
            render,
            &program_ctx,
            &input.tool_feeds,
        ) {
            Ok(text) => ProgramOutcome::Ready(Program {
                extension: render.file_extension.clone(),
                block_count: step.blocks.len(),
                op_count: step.op_count(),
                text,
            }),
            Err(err) => ProgramOutcome::Failed(err.message()),
        };
        steps.push(StepProgram { index, name, cnc_name: render.cnc_name.clone(), outcome });
    }

    let summary = summarise(input, &steps);
    for step in steps.iter().filter(|s| s.failure().is_some()) {
        // WARN so the Logs screen carries the detail the one-line summary cannot.
        log::warn!(
            "Step {} ('{}') produced no program: {}",
            step.index + 1,
            step.name,
            step.failure().unwrap_or_default()
        );
    }
    return Ok(GenerationOutput { steps, summary });
}

/// One line describing what the run produced, for the toast and the log.
fn summarise(input: &GenerationInput, steps: &[StepProgram]) -> String {
    let ready: Vec<&StepProgram> = steps.iter().filter(|s| s.program().is_some()).collect();
    let failed: Vec<&StepProgram> = steps.iter().filter(|s| s.failure().is_some()).collect();
    let drilled = input.plan.total_ops();
    let deferred: usize = input.plan.steps.iter().map(|s| s.notes.len()).sum();

    let deferred_note = if deferred > 0 {
        format!(" · {deferred} item(s) deferred — see the Machining view")
    } else {
        String::new()
    };
    let failed_note = if failed.is_empty() {
        String::new()
    } else {
        format!(
            " · {} failed ({})",
            failed.len(),
            failed
                .iter()
                .map(|s| format!("Step {}: {}", s.index + 1, s.failure().unwrap_or_default()))
                .collect::<Vec<_>>()
                .join("; ")
        )
    };
    let lines: usize = ready
        .iter()
        .filter_map(|s| s.program())
        .map(|p| p.text.lines().count())
        .sum();

    // A single step is the common case and reads as it always has — no step counting,
    // no plural — so a one-step job shows no trace of the multi-step machinery.
    if steps.len() == 1 {
        return match steps[0].program() {
            Some(program) => format!(
                "Program for '{}' ({}): {} lines · {drilled} hole(s) in {} tool block(s){deferred_note}",
                input.process_profile_name,
                steps[0].cnc_name,
                program.text.lines().count(),
                program.block_count,
            ),
            None => format!(
                "No program for '{}': {}",
                input.process_profile_name,
                steps[0].failure().unwrap_or_default()
            ),
        };
    }

    format!(
        "Programs for '{}': {} of {} step(s) ready · {lines} lines · {drilled} hole(s){deferred_note}{failed_note}",
        input.process_profile_name,
        ready.len(),
        steps.len(),
    )
}

/// Commit a successful run into the ctx and settle to Idle.
fn publish_success(output: GenerationOutput) {
    // Mirror the summary into the tracing log so it lands in the Logs screen, not
    // only the transient event toast.
    log::info!("{}", output.summary);
    with_ctx_mut(|ctx| {
        // `Idle` while at least one step produced a program: the run completed and there
        // is something to save. A run where *every* step failed is a failed run, even
        // though no whole-run abort occurred.
        ctx.app.generation_state = if output.steps.iter().any(|s| s.program().is_some()) {
            GenerationState::Idle
        } else {
            GenerationState::Failed
        };
        // Standing diagnostics, one per step that produced nothing — replacing the previous
        // run's, so a step that now succeeds clears its own entry. A partial run reports
        // them too: "3 of 4 steps ready" is still a job the operator cannot run.
        ctx.app.set_generation_errors(step_failures(&output.steps));
        ctx.app.programs = output.steps;
        ctx.app.log_event(output.summary);
    });
}

/// The standing diagnostic for each step that produced no program.
///
/// The headline names the step, because a multi-step job's operator needs to know *which*
/// setup is broken before the message means anything; the detail is the renderer's own
/// sentence, which for a rejected origin reference is the profile author's wording.
fn step_failures(steps: &[StepProgram]) -> Vec<(String, Option<String>)> {
    steps
        .iter()
        .filter_map(|step| {
            let failure = step.failure()?;
            let headline = if steps.len() == 1 {
                "No program was generated".to_string()
            } else {
                format!("No program for step {} ('{}')", step.index + 1, step.name)
            };
            Some((headline, Some(failure.to_string())))
        })
        .collect()
}

/// Commit a failure: clear all derived outputs (a live tool must never show a
/// stale program) and surface the diagnostic.
fn publish_failure(message: &str) {
    let message = message.to_string();
    // Log at WARN so the failure is captured by the Logs screen and stdout — not
    // only the transient toast/banner. This is the diagnostic a user needs when a
    // primitive template references an unknown variable (e.g. `z_safe`).
    log::warn!("Generation failed: {message}");
    with_ctx_mut(|ctx| {
        ctx.app.generation_state = GenerationState::Failed;
        ctx.app.programs.clear();
        // Standing, not just a toast: the run is over, there is no program, and nothing
        // else on screen would say why once the notification faded.
        ctx.app.set_generation_errors(vec![(
            "Generation failed".to_string(),
            Some(message.clone()),
        )]);
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

    /// An `AppState` with nothing loaded — enough to exercise the diagnostics list, which
    /// is the only part of it these tests touch.
    fn bare_app() -> AppState {
        AppState::new(&UiLaunchData { kicad_status: String::new(), board_snapshot: None })
    }

    /// One step's program context, from the shared template fixtures. Production sources
    /// these from the step's own CNC and fixture (`build_program_render_ctx`).
    fn sample_program_render() -> crate::gcode::program::ProgramRender {
        crate::gcode::program::ProgramRender {
            cnc_name: "Genmitsu".to_string(),
            program_begin_tpl: crate::gcode::program::sample_initialise_tpl(),
            program_end_tpl: crate::gcode::program::sample_conclude_tpl(),
            line_format_tpl: String::new(),
            set_unit_tpl: crate::gcode::program::sample_set_unit_tpl(),
            set_origin_tpl: crate::gcode::program::sample_set_origin_tpl(),
            // The operator callables. Only reachable if a template calls them, and the
            // sample header does not — so they change no existing expectation.
            comment_tpl: "`( {text} )".to_string(),
            message_tpl: "`MSG {text}".to_string(),
            pause_tpl: "`MSG {text}\n`M01".to_string(),
            z_safe: units::Length::from_mm(5.0),
            origin_reference: "G54".to_string(),
            file_extension: "nc".to_string(),
            body: crate::gcode::program::sample_step_render(true),
        }
    }

    /// A step with no blocks. It still renders a program — header and footer are the
    /// setup's own, and a step that machines nothing is a legitimate (if idle) program.
    fn empty_step(index: usize) -> crate::gcode::plan::StepPlan {
        crate::gcode::plan::StepPlan {
            index,
            name: format!("Step {}", index + 1),
            blocks: vec![],
            notes: vec![],
        }
    }

    /// A minimal step record — enough for a header to name itself.
    fn sample_step_value(name: &str) -> crate::gcode::step_data::StepValue {
        use crate::gcode::step_data::StepValue;
        StepValue::Map(vec![
            ("name".into(), StepValue::Text(name.to_string())),
            ("board_face".into(), StepValue::Text("front".into())),
            ("cnc_name".into(), StepValue::Text("Sample mill".into())),
        ])
    }

    fn sample_input() -> GenerationInput {
        GenerationInput {
            process_profile_name: "Proto".to_string(),
            operations: vec!["Drill PTH".to_string(), "Route outline".to_string()],
            filename: "demo.kicad_pcb".to_string(),
            timestamp: "2026-01-01 00:00:00".to_string(),
            plan: crate::gcode::plan::MachiningPlan {
                steps: vec![empty_step(0)],
                note: None,
            },
            steps: vec![sample_program_render()],
            step_values: vec![sample_step_value("Drill PTH")],
            tool_feeds: std::collections::BTreeMap::new(),
        }
    }

    /// The `index`-th step's program text, or a panic naming what went wrong instead.
    fn text(out: &GenerationOutput, index: usize) -> String {
        let step = out.steps.get(index).expect("step present in the output");
        match step.program() {
            Some(program) => program.text.clone(),
            None => panic!("step {index} produced no program: {:?}", step.failure()),
        }
    }

    #[test]
    fn header_run_is_deterministic_and_renders_the_preamble() {
        let cancel = Arc::new(AtomicBool::new(false));
        let a = run_generation(&sample_input(), &cancel).ok().unwrap();
        let b = run_generation(&sample_input(), &cancel).ok().unwrap();
        let (a, b) = (text(&a, 0), text(&b, 0));
        assert_eq!(a, b, "same input must yield identical program");
        assert!(a.contains("(k2g demo.kicad_pcb - 2026-01-01 00:00:00)"), "header comment");
        assert!(a.contains("G21"), "metric() emitted the modal word");
        assert!(a.contains("(end of file)"), "footer rendered");
        // The program is the real rendered preamble + postamble — no mockup filler.
        assert!(!a.contains("body pending"), "no placeholder in the program");
        let header_pos = a.find("G0 Z5").expect("initialise rendered");
        let footer_pos = a.find("(end of file)").expect("conclude rendered");
        assert!(header_pos < footer_pos, "initialise precedes conclude");
    }

    #[test]
    fn a_drill_plan_renders_a_full_program_body_between_header_and_footer() {
        use crate::gcode::plan::{
            AtomicOp, MachiningPlan, OpKind, Phase, Point, StepPlan, ToolBlock, ZProfile,
        };
        use crate::gcode::program::{sample_step_render, ToolFeed};
        use units::{FeedRate, RotationalSpeed};

        let op = AtomicOp {
            phase: Phase::Drill,
            kind: OpKind::Drill,
            tool_id: "t1".to_string(),
            entry: Point::new(units::Length::from_mm(3.0), units::Length::from_mm(4.0)),
            exit: Point::new(units::Length::from_mm(3.0), units::Length::from_mm(4.0)),
            z: ZProfile {
                z_bottom: units::Length::from_mm(-2.4),
                z_retract: units::Length::from_mm(5.0),
                z_feed: None,
            },
            primitive: "drill",
            source: "h1".to_string(),
        };
        let plan = MachiningPlan {
            steps: vec![StepPlan {
                index: 0,
                name: "Step 1".to_string(),
                blocks: vec![ToolBlock {
                    tool_id: "t1".to_string(),
                    slot: Some(1),
                    diameter: units::Length::from_mm(1.0),
                    ops: vec![op],
                    travel_mm: 0.0,
                }],
                notes: vec![],
            }],
            note: None,
        };
        // Templates come from the shared test fixture, not hand-written GCode here —
        // production sources them from the CNC profile (build_step_render_ctx).
        let render = crate::gcode::program::ProgramRender {
            body: sample_step_render(true),
            ..sample_program_render()
        };
        let mut tool_feeds = std::collections::BTreeMap::new();
        tool_feeds.insert(
            "t1".to_string(),
            ToolFeed {
                name: "1mm drill".to_string(),
                feed: Some(FeedRate::from_mm_per_min(600.0)),
                speed: Some(RotationalSpeed::from_rpm(12_000.0)),
            },
        );

        let input =
            GenerationInput { plan, steps: vec![render], tool_feeds, ..sample_input() };
        let cancel = Arc::new(AtomicBool::new(false));
        let out = run_generation(&input, &cancel).ok().unwrap();
        let g = text(&out, 0);

        // header (G21) → tool change → drill cycle → spindle stop → footer, in order.
        let header = g.find("G21").expect("header rendered");
        let change = g.find("T1 M06").expect("tool change emitted");
        let drill = g.find("G81 X3 Y4 Z-2.4 R5 F600").expect("drill cycle with negative depth + feed");
        let stop = g.find("M05").expect("spindle stopped");
        let footer = g.find("(end of file)").expect("footer rendered");
        assert!(
            header < change && change < drill && drill < stop && stop < footer,
            "sections out of order:\n{g}"
        );
        assert!(out.summary.contains("1 hole(s) in 1 tool block(s)"), "summary: {}", out.summary);
    }

    #[test]
    fn line_numbers_are_applied_to_the_assembled_program_by_the_cnc_template() {
        let render = crate::gcode::program::ProgramRender {
            line_format_tpl: "`N{(index + 1) * 10} {text}".to_string(),
            ..sample_program_render()
        };
        let input = GenerationInput { steps: vec![render], ..sample_input() };
        let cancel = Arc::new(AtomicBool::new(false));
        let out = run_generation(&input, &cancel).ok().unwrap();
        let program = text(&out, 0);
        let lines: Vec<&str> = program.lines().collect();
        assert!(lines[0].starts_with("N10 "), "first line numbered: {:?}", lines[0]);
        assert!(lines[1].starts_with("N20 "), "second steps by the increment: {:?}", lines[1]);
        assert!(!lines.iter().any(|l| l.trim().is_empty()), "no blank lines remain");
        // The header content is intact behind the number.
        assert!(program.contains("G21"), "modal word still present:\n{program}");
    }

    // --- one program per step -----------------------------------------------

    /// Each step is a whole program, so each restarts its own `N` sequence. Numbering the
    /// job as one stream would leave step 2 starting at whatever step 1 reached.
    #[test]
    fn line_numbering_restarts_for_every_step() {
        let numbered = || crate::gcode::program::ProgramRender {
            line_format_tpl: "`N{(index + 1) * 10} {text}".to_string(),
            ..sample_program_render()
        };
        let input = GenerationInput {
            plan: crate::gcode::plan::MachiningPlan {
                steps: vec![empty_step(0), empty_step(1)],
                note: None,
            },
            steps: vec![numbered(), numbered()],
            ..sample_input()
        };
        let out = run_generation(&input, &Arc::new(AtomicBool::new(false))).ok().unwrap();
        for index in 0..2 {
            let program = text(&out, index);
            assert!(
                program.lines().next().unwrap_or_default().starts_with("N10 "),
                "step {index} must open at N10:\n{program}"
            );
        }
    }

    /// The build's version reaches the program, and is the real one.
    ///
    /// The point of `k2g_version` is tracing a board back to the build that machined it,
    /// so a placeholder or a stale constant would defeat it entirely. Asserted against
    /// `CARGO_PKG_VERSION` itself rather than a literal, because a literal here would be
    /// one more thing to remember at release time — which is the failure this whole
    /// mechanism exists to end (`build.rs` warns when Cargo.toml falls behind the tags).
    #[test]
    fn the_header_can_print_the_version_that_generated_it() {
        let input = GenerationInput {
            steps: vec![crate::gcode::program::ProgramRender {
                program_begin_tpl: "`(k2g {k2g_version})".to_string(),
                ..sample_program_render()
            }],
            ..sample_input()
        };
        let out = run_generation(&input, &Arc::new(AtomicBool::new(false))).ok().unwrap();

        let expected = format!("(k2g {})", env!("CARGO_PKG_VERSION"));
        assert!(text(&out, 0).contains(&expected), "expected {expected}:
{}", text(&out, 0));
    }

    /// It is in the footer's scope too. The two share one closure, so this is really a
    /// guard on that staying true — a footer is a natural place to stamp the version.
    #[test]
    fn the_version_is_available_to_the_footer_as_well() {
        let input = GenerationInput {
            steps: vec![crate::gcode::program::ProgramRender {
                program_end_tpl: "`(end k2g {k2g_version})".to_string(),
                ..sample_program_render()
            }],
            ..sample_input()
        };
        let out = run_generation(&input, &Arc::new(AtomicBool::new(false))).ok().unwrap();
        assert!(text(&out, 0).contains(&format!("(end k2g {})", env!("CARGO_PKG_VERSION"))));
    }

    /// Each step's header reads **its own** record out of the shared `steps` array.
    ///
    /// The trap this closes is an off-by-one that would be invisible: every step gets the
    /// same array, and only `step_index` distinguishes them, so a header wired to
    /// `steps[0]` would render a plausible program naming the wrong setup. Two steps with
    /// different names is the smallest case that can tell the difference.
    #[test]
    fn each_steps_header_names_itself_out_of_the_shared_step_list() {
        let header = || crate::gcode::program::ProgramRender {
            program_begin_tpl:
                "`(step {step_index + 1} of {steps.len()}: {steps[step_index].name} on \
                 {steps[step_index].cnc_name})"
                    .to_string(),
            ..sample_program_render()
        };
        let input = GenerationInput {
            plan: crate::gcode::plan::MachiningPlan {
                steps: vec![empty_step(0), empty_step(1)],
                note: None,
            },
            steps: vec![header(), header()],
            step_values: vec![sample_step_value("Drill PTH"), sample_step_value("Route outline")],
            ..sample_input()
        };
        let out = run_generation(&input, &Arc::new(AtomicBool::new(false))).ok().unwrap();

        assert!(
            text(&out, 0).contains("(step 1 of 2: Drill PTH on Sample mill)"),
            "step 0:\n{}",
            text(&out, 0)
        );
        assert!(
            text(&out, 1).contains("(step 2 of 2: Route outline on Sample mill)"),
            "step 1:\n{}",
            text(&out, 1)
        );
    }

    /// A measurement inside a step is still a measurement.
    ///
    /// This is the assertion that fails if the step tree ever degrades to plain JSON on
    /// the way to the worker: `Node::to_value` renders a `0.1mm` as the *string*
    /// `"0.1mm"`, which would print identically under `metric()` and so pass every test
    /// but this one — and then emit millimetres into an inch program.
    #[test]
    fn a_measurement_inside_a_step_follows_the_programs_unit_mode() {
        use crate::gcode::step_data::StepValue;

        let step_values = vec![StepValue::Map(vec![(
            "route_board".into(),
            StepValue::Map(vec![("finishing".into(), StepValue::Length(Length::from_mm(25.4)))]),
        )])];
        let render = |unit: &str| crate::gcode::program::ProgramRender {
            program_begin_tpl: format!(
                "{unit}();\n`(finish {{steps[step_index].route_board.finishing}})"
            ),
            ..sample_program_render()
        };

        for (unit, expected) in [("metric", "(finish 25.4)"), ("imperial", "(finish 1)")] {
            let input = GenerationInput {
                steps: vec![render(unit)],
                step_values: step_values.clone(),
                ..sample_input()
            };
            let out = run_generation(&input, &Arc::new(AtomicBool::new(false))).ok().unwrap();
            assert!(
                text(&out, 0).contains(expected),
                "{unit}() should give {expected}:\n{}",
                text(&out, 0)
            );
        }
    }

    /// A list has no printable form, and reaching past the end is an error rather than an
    /// empty word in the middle of a G-code line. Both are the honest outcome — a header
    /// that quietly dropped a value would produce a program nobody could tell was wrong —
    /// and both are documented in the operator help, so they are pinned here.
    #[test]
    fn a_step_list_cannot_be_printed_whole_or_indexed_past_its_end() {
        for template in ["`(steps: {steps})", "`(next: {steps[step_index + 1].name})"] {
            let input = GenerationInput {
                steps: vec![crate::gcode::program::ProgramRender {
                    program_begin_tpl: template.to_string(),
                    ..sample_program_render()
                }],
                ..sample_input()
            };
            let out = run_generation(&input, &Arc::new(AtomicBool::new(false))).ok().unwrap();
            assert!(
                out.steps[0].failure().is_some(),
                "`{template}` must fail rather than emit something:\n{:?}",
                out.steps[0].program().map(|p| p.text.clone())
            );
        }
    }

    /// The Coder carries the modal unit state `initialise` sets. Sharing one across steps
    /// would let step 1's `metric()` silently govern step 2's lengths — the exact
    /// cross-contamination the per-step model exists to prevent, and invisible in the
    /// output because the numbers still *look* plausible.
    #[test]
    fn a_step_does_not_inherit_the_previous_steps_unit_mode() {
        // Step 1 works in inches; step 2 sets no mode at all and just emits a length.
        // The two disagree observably: 25.4 mm is "1" in inches and "25.4" in mm, so a
        // shared Coder shows up as step 2 silently emitting step 1's units.
        let inches = crate::gcode::program::ProgramRender {
            program_begin_tpl: "imperial();\n`G0 Z{z_safe}".to_string(),
            z_safe: units::Length::from_mm(25.4),
            ..sample_program_render()
        };
        let no_mode = crate::gcode::program::ProgramRender {
            program_begin_tpl: "`G0 Z{z_safe}".to_string(),
            z_safe: units::Length::from_mm(25.4),
            ..sample_program_render()
        };
        let input = GenerationInput {
            plan: crate::gcode::plan::MachiningPlan {
                steps: vec![empty_step(0), empty_step(1)],
                note: None,
            },
            steps: vec![inches, no_mode],
            ..sample_input()
        };
        let out = run_generation(&input, &Arc::new(AtomicBool::new(false))).ok().unwrap();
        assert!(text(&out, 0).contains("G0 Z1"), "step 1 formats in its own inches");
        assert!(
            text(&out, 1).contains("G0 Z25.4"),
            "step 2 must format in its own default, not step 1's inches:\n{}",
            text(&out, 1)
        );
    }

    /// A step's header retracts to *its own* fixture's safe height and zeroes into *its
    /// own* machine origin. Both used to come from the job-level projection, so this test
    /// fails against the pre-per-step code.
    ///
    /// The origin half is the sharper of the two now: `set_origin` is rendered **once, at
    /// Coder construction**, so a Coder shared between steps would bake step 1's origin
    /// into step 2's program. This asserts that it does not.
    #[test]
    fn each_step_uses_its_own_fixtures_height_and_work_offset() {
        let fixture = |z_safe: f64, origin: &str| crate::gcode::program::ProgramRender {
            program_begin_tpl: "`G0 Z{z_safe}\nset_origin();".to_string(),
            z_safe: units::Length::from_mm(z_safe),
            origin_reference: origin.to_string(),
            ..sample_program_render()
        };
        let input = GenerationInput {
            plan: crate::gcode::plan::MachiningPlan {
                steps: vec![empty_step(0), empty_step(1)],
                note: None,
            },
            steps: vec![fixture(5.0, "G54"), fixture(22.0, "G56")],
            ..sample_input()
        };
        let out = run_generation(&input, &Arc::new(AtomicBool::new(false))).ok().unwrap();
        assert!(text(&out, 0).contains("G0 Z5") && text(&out, 0).contains("G54"));
        assert!(
            text(&out, 1).contains("G0 Z22") && text(&out, 1).contains("G56"),
            "step 2 must use its own fixture, not step 1's:\n{}",
            text(&out, 1)
        );
        assert!(!text(&out, 1).contains("G54"), "step 1's origin must not leak into step 2");
    }

    /// A two-sided job: the back-face program asks the operator to confirm the board was
    /// turned over, and the front-face one does not.
    ///
    /// End to end through `render_step_program`, not just the body renderer, because the
    /// prompt reaches the program through the machine's own `pause` primitive — a path that
    /// runs from `StepRender` through the Coder's callable registration and out into the
    /// numbered text. And it is load-bearing: two symmetric pins of one diameter accept the
    /// board unflipped, and turned 180°, exactly as readily as the right way up, so no
    /// geometry k2g has can reject a wrong remount. The question is the whole guard.
    #[test]
    fn only_the_back_face_program_asks_the_operator_to_confirm_the_flip() {
        let face = |prompt: Option<&str>| crate::gcode::program::ProgramRender {
            body: crate::gcode::program::StepRender {
                opening_prompt: prompt.map(str::to_string),
                ..crate::gcode::program::sample_step_render(true)
            },
            ..sample_program_render()
        };
        let input = GenerationInput {
            plan: crate::gcode::plan::MachiningPlan {
                steps: vec![empty_step(0), empty_step(1)],
                note: None,
            },
            steps: vec![face(None), face(Some("Board back face up?"))],
            ..sample_input()
        };

        let out = run_generation(&input, &Arc::new(AtomicBool::new(false))).ok().unwrap();
        assert!(
            !text(&out, 0).contains("Board back face up?"),
            "the front face has nothing to confirm — a prompt on every program is one every \
             operator learns to click through:\n{}",
            text(&out, 0)
        );
        let back = text(&out, 1);
        assert!(
            back.contains("MSG Board back face up?") && back.contains("M01"),
            "the machine's own pause primitive carries it:\n{back}"
        );
    }

    /// One bad step must not cost the operator the programs for the others — they are
    /// separate setups, and the good ones are still worth running.
    #[test]
    fn a_failing_step_does_not_take_the_others_down_with_it() {
        use crate::gcode::plan::{AtomicOp, OpKind, Phase, Point, ToolBlock, ZProfile};
        // A block whose tool has no entry in `tool_feeds` cannot resolve a feed/speed.
        let block = ToolBlock {
            tool_id: "unknown".to_string(),
            slot: Some(1),
            diameter: units::Length::from_mm(1.0),
            ops: vec![AtomicOp {
                phase: Phase::Drill,
                kind: OpKind::Drill,
                tool_id: "unknown".to_string(),
                entry: Point::new(units::Length::from_mm(1.0), units::Length::from_mm(1.0)),
                exit: Point::new(units::Length::from_mm(1.0), units::Length::from_mm(1.0)),
                z: ZProfile {
                    z_bottom: units::Length::from_mm(-1.0),
                    z_retract: units::Length::from_mm(2.0),
                    z_feed: None,
                },
                primitive: "drill",
                source: "h".to_string(),
            }],
            travel_mm: 0.0,
        };
        let mut broken = empty_step(0);
        broken.blocks = vec![block];

        let input = GenerationInput {
            plan: crate::gcode::plan::MachiningPlan {
                steps: vec![broken, empty_step(1)],
                note: None,
            },
            steps: vec![sample_program_render(), sample_program_render()],
            ..sample_input()
        };
        let out = run_generation(&input, &Arc::new(AtomicBool::new(false)))
            .ok()
            .expect("the run itself must not abort");
        assert!(out.steps[0].failure().is_some(), "the broken step failed");
        assert!(out.steps[1].program().is_some(), "the sound step still generated");
        assert!(out.summary.contains("1 of 2"), "summary counts them: {}", out.summary);

        // A partly-failed run is still a run the operator cannot execute, so the failure
        // must stand in the banner and name which step it was — not just pass by as a toast.
        let failures = step_failures(&out.steps);
        assert_eq!(failures.len(), 1, "one standing diagnostic, for the one failed step");
        assert!(
            failures[0].0.contains("step 1"),
            "the headline must name the step: {}",
            failures[0].0
        );
        assert!(failures[0].1.is_some(), "and carry the renderer's own reason as the detail");
    }

    /// A failed generation is a **standing** condition, not a passing notification: with
    /// `programs` cleared, the banner is the only thing left on screen that can say why
    /// there is no G-code. This is the regression guard for that — the failure used to be
    /// reported by `log_event` alone, so it faded after a few seconds and left an empty Code
    /// view with no explanation.
    #[test]
    fn a_failed_step_becomes_a_standing_diagnostic_that_clears_on_success() {
        let failed = |reason: &str| StepProgram {
            index: 0,
            name: "Drill top".to_string(),
            cnc_name: "Masso".to_string(),
            outcome: ProgramOutcome::Failed(reason.to_string()),
        };

        // The case reported from the field: an origin reference the machine does not have.
        let reason = "primitive 'set_origin': 'XX' is not a valid origin reference for this machine.";
        let failures = step_failures(&[failed(reason)]);
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].1.as_deref(), Some(reason), "the detail is the author's sentence");

        // A single-step job does not count steps at it — there is only one.
        assert_eq!(failures[0].0, "No program was generated");

        // Applied to the ctx it lands in `errors`, which is what the banner renders from;
        // a later good run replaces the domain and the banner goes away by itself.
        let mut state = bare_app();
        state.set_generation_errors(failures);
        assert_eq!(state.errors.len(), 1, "the failure stands in the diagnostics");
        assert!(state.errors[0].is_error);
        assert_eq!(state.errors[0].domain, GENERATION_ERROR_DOMAIN);

        state.set_generation_errors(Vec::new());
        assert!(state.errors.is_empty(), "a run with no failures clears the banner");
    }

    /// A standing generation error must not shut the gate that would clear it.
    ///
    /// The entry is `is_error`, and the readiness gate refuses to run while any such error
    /// is present — so counting this one would make a single failure permanent: the error
    /// stops the next run, and only a run can replace the error. The operator would correct
    /// the fixture and watch nothing happen, forever. A config error still blocks.
    #[test]
    fn a_standing_generation_error_does_not_block_the_run_that_would_clear_it() {
        let mut state = bare_app();
        state.set_generation_errors(vec![("No program was generated".to_string(), None)]);
        assert!(state.errors[0].is_error, "precondition: the entry is a blocking-shaped error");
        assert!(
            !has_blocking_config_error(&state),
            "the previous run's own failure must not be a reason to refuse the next run"
        );

        // An error from anywhere else still does block.
        state.push_runtime_error_quiet("current-job-ref", None, "missing fixture".into(), None);
        assert!(has_blocking_config_error(&state), "a configuration error is still blocking");

        // And the generation entry clearing does not clear the configuration one.
        state.set_generation_errors(Vec::new());
        assert!(has_blocking_config_error(&state), "domains are independent");
    }

    /// Entries pushed within the same millisecond must not collide on `id` — a multi-step
    /// failure pushes several at once, and duplicate keys make the UI list misbehave.
    #[test]
    fn several_failures_from_one_run_get_distinct_ids() {
        let mut state = bare_app();
        state.set_generation_errors(vec![
            ("a".to_string(), None),
            ("b".to_string(), None),
            ("c".to_string(), None),
        ]);
        let ids: std::collections::BTreeSet<&str> =
            state.errors.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids.len(), 3, "three entries, three ids: {ids:?}");
    }

    /// A planned step with no CNC to render through says so. Skipping it would leave a
    /// program silently absent, which is the worst way for this to go wrong.
    #[test]
    fn a_step_with_no_render_context_reports_rather_than_vanishing() {
        let input = GenerationInput {
            plan: crate::gcode::plan::MachiningPlan {
                steps: vec![empty_step(0), empty_step(1)],
                note: None,
            },
            steps: vec![sample_program_render()], // only one context for two steps
            ..sample_input()
        };
        let out = run_generation(&input, &Arc::new(AtomicBool::new(false))).ok().unwrap();
        assert_eq!(out.steps.len(), 2, "both steps are accounted for");
        assert!(out.steps[1].failure().unwrap_or_default().contains("CNC profile"));
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

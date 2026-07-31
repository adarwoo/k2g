//! The 3D machining view — a WebGL toolpath render inside the WebView.
//!
//! Draws what [`scene::trace_step`](crate::gcode::scene::trace_step) extracts from the
//! machining plan: one coloured polyline set per tool block, rapids muted and thin
//! against solid cutting moves, orbitable so the Z motion is actually legible.
//!
//! ## Why the split looks like this
//!
//! Everything that can be decided in Rust is decided in Rust, and the JavaScript stays
//! deliberately stupid — it receives geometry and owns the camera, nothing else. That
//! matters for two reasons. Testing: this is the only part of k2g with no compiler and
//! no unit tests, so the less logic it holds the less is untested. And latency: the
//! camera living in JS means the Rust↔JS boundary is crossed **once per plan change**,
//! never per mouse move.
//!
//! ## Errors
//!
//! A script that throws in a WebView leaves no trace — no panic, no stderr, just a blank
//! canvas — and WebView2's devtools have not been dependable on this project. So the
//! page traps its own errors (`ERROR_TRAP` in `crate::ui`) and this module drains them
//! into the Logs screen after every draw.

use std::collections::BTreeSet;

use dioxus::prelude::*;

use crate::gcode::scene;
use crate::runtime::machining_plan::{self, plan_machining};
use crate::runtime::AppCtx;

/// The DOM id the canvas is mounted under. The script finds it rather than being handed
/// a node, because `eval` runs in page scope with no reference to Dioxus's tree.
const CANVAS_ID: &str = "k2g-machining-3d";

/// The scene script. Four placeholders are substituted before it is evaluated:
/// `CANVAS_ID_PLACEHOLDER`, `TRACES_PLACEHOLDER` (the serialised
/// [`ToolTrace`](crate::gcode::scene::ToolTrace) list), `BOARD_PLACEHOLDER` (the
/// serialised [`BoardSolid`](crate::gcode::scene::BoardSolid)) and `FIXTURE_PLACEHOLDER`
/// (the [`FixtureMark`](crate::gcode::scene::FixtureMark): work origin, stop and pins).
///
/// The palette used to be substituted too, and the script picked a colour by the trace's
/// position in the list. It no longer is: each trace carries its own `colour`, assigned
/// in [`trace_step`](crate::gcode::scene::trace_step), so hiding a tool cannot recolour
/// the ones after it.
///
/// Substitution rather than `dioxus.send` from the Rust side because the payload is
/// needed *before* the first frame, and a script that has its data inlined cannot race
/// with the channel.
///
/// The script is evaluated once per plan change, and splits in two on arrival: the first
/// evaluation builds the renderer, the camera and the controls and leaves a `__k2g_draw`
/// on the canvas; every later one calls that with the new geometry and returns. The split
/// exists because the two halves have opposite lifetimes — a second `WebGLRenderer` on
/// the same canvas would leak a WebGL context (browsers cap those at ~16), and a rebuilt
/// camera would throw away wherever the user had orbited to, while the geometry has to be
/// replaced wholesale or the view silently keeps showing the previous plan.
///
/// `scratchpad/harness/render.sh` extracts this literal verbatim and renders it under
/// headless Edge, so it can be checked without launching the app.
const BOOTSTRAP_SCRIPT: &str = r#"
(function () {
  const T = window.K2G_THREE;
  if (!T) { dioxus.send("three.js global K2G_THREE is missing — the head script did not run"); return; }
  const canvas = document.getElementById("CANVAS_ID_PLACEHOLDER");
  if (!canvas) { dioxus.send("canvas #CANVAS_ID_PLACEHOLDER not found"); return; }

  const board = BOARD_PLACEHOLDER;
  const traces = TRACES_PLACEHOLDER;
  const fixture = FIXTURE_PLACEHOLDER;

  // Already running on this canvas: hand the new plan to the renderer that is up and
  // return. Everything below this line is one-per-canvas and must not run twice.
  if (canvas.__k2g_draw) {
    try { canvas.__k2g_draw(board, traces, fixture); dioxus.send(""); }
    catch (err) { dioxus.send(String((err && err.stack) || err)); }
    return;
  }

  try {
    const renderer = new T.WebGLRenderer({ canvas: canvas, antialias: true, alpha: true });
    renderer.setPixelRatio(window.devicePixelRatio || 1);

    const scene = new T.Scene();
    const camera = new T.PerspectiveCamera(45, 1, 0.1, 5000);
    camera.up.set(0, 0, 1);            // Z is up: machine convention, not screen convention.

    const controls = new T.OrbitControls(camera, canvas);
    controls.enableDamping = true;

    scene.add(new T.HemisphereLight(0xffffff, 0x334455, 2.0));
    const key = new T.DirectionalLight(0xffffff, 1.2);
    key.position.set(80, -120, 200);
    scene.add(key);
    scene.add(new T.AxesHelper(20));

    // Everything that comes from the plan hangs off one group, so a redraw is "empty the
    // group and refill it" rather than working out which of the scene's children belonged
    // to the previous plan.
    const content = new T.Group();
    scene.add(content);

    // Fat lines are sized in *screen* space, so every material's `resolution` has to
    // follow the canvas; the live set is collected here for `resize` and replaced
    // wholesale on each redraw.
    let materials = [];

    // The bounds the camera was last framed on — see `frameScene`.
    let framed = null;

    // How far the work must move or resize, as a fraction of its own span, before a
    // redraw is allowed to take the camera back.
    const REFRAME_RATIO = 0.25;

    // Drops the previous plan's geometry, GPU buffers included. Three.js does not free
    // those on removal from the scene graph, so a redraw without this would grow the
    // renderer's memory by a whole plan every time.
    function clearContent() {
      content.traverse(function (object) {
        if (object.geometry) object.geometry.dispose();
        if (object.material) object.material.dispose();
      });
      content.clear();
      materials = [];
    }

    // The workpiece: the outline extruded downward from Z0, with every cutout and
    // drilled hole as a hole in the shape. Real holes rather than cylinders sitting in
    // them, so you can see through the board and see the drill go somewhere.
    function addBoard(board) {
      if (!board || !board.outline || board.outline.length <= 2) return;
      const shape = new T.Shape(board.outline.map(function (p) { return new T.Vector2(p[0], p[1]); }));
      (board.openings || []).forEach(function (loop) {
        if (loop.length > 2) {
          shape.holes.push(new T.Path(loop.map(function (p) { return new T.Vector2(p[0], p[1]); })));
        }
      });
      const slab = new T.Mesh(
        new T.ExtrudeGeometry(shape, { depth: board.thickness_mm, bevelEnabled: false }),
        // Lambert, not Standard: no metalness or roughness to tune, and a matt surface
        // is easier to read orientation from than a shiny one.
        //
        // Slightly transparent on purpose. Cutting happens *below* Z0 — the whole
        // breakthrough is inside the material — so an opaque slab hides the part of
        // the job the view exists to show. `depthWrite: false` stops the board from
        // occluding the paths behind it while still shading as a solid.
        new T.MeshLambertMaterial({
          color: 0x1f6f43,
          side: T.DoubleSide,
          transparent: true,
          opacity: 0.72,
          depthWrite: false,
        })
      );
      // Z0 is the board's top surface (op-planner §6), so the slab hangs below it —
      // which is what puts the toolpaths' plunges *into* the material.
      slab.position.z = -board.thickness_mm;
      content.add(slab);

      // The two faces, in the board's own colours: the **back** is always red and the
      // **front** always green, whichever way up the board happens to be lying.
      //
      // Which one the spindle sees is what `back_face_up` says, so a front-face step shows
      // green with red underneath, and a back-face step shows red with green underneath.
      // That is not decoration: the artwork is mirrored for a back-face step (correctly,
      // since the board is physically turned over), and a mirrored board is
      // indistinguishable from a right-way-round one unless you already know which face
      // you are looking at. Colouring both means the answer is on screen from any angle.
      const face = function (colour, z) {
        const mesh = new T.Mesh(
          new T.ShapeGeometry(shape),
          new T.MeshLambertMaterial({
            color: colour,
            side: T.DoubleSide,
            transparent: true,
            opacity: 0.85,
            // Off, so the toolpaths that plunge through this face are not occluded by it —
            // the same reason the slab itself does not write depth.
            depthWrite: false,
          })
        );
        // Held a hair clear of the slab. Coplanar faces z-fight, and the flicker reads as
        // a rendering fault rather than as the deliberate marking it is.
        mesh.position.z = z;
        content.add(mesh);
      };
      const BACK = 0xc0392b, FRONT = 0x1f6f43;
      const up = 0.02, down = -board.thickness_mm - 0.02;
      face(board.back_face_up ? BACK : FRONT, up);
      face(board.back_face_up ? FRONT : BACK, down);
    }

    // The setup around the work: the zero, the stop the board registers against, and the
    // locating pins. None of it is cut — it is the *frame* the program is written in, and
    // the gap it shows between the bracket and the board is the room the origin made for
    // the pins. That gap is the one thing about a pinned job's coordinates that is
    // otherwise invisible: the numbers in the program look entirely ordinary either way.
    function addFixture(fixture) {
      if (!fixture) return;

      const line = function (points, colour, width) {
        const flat = [];
        points.forEach(function (p) { flat.push(p[0], p[1], p[2]); });
        const geometry = new T.LineGeometry();
        geometry.setPositions(flat);
        const material = new T.LineMaterial({ color: colour, linewidth: width, worldUnits: false });
        materials.push(material);
        content.add(new T.Line2(geometry, material));
      };

      // The L-bracket, drawn at Z0 (the board's top) so it sits in the plane the
      // coordinates are quoted in.
      const arm = fixture.arm_mm || 10;
      line([[arm * fixture.dir_x, 0, 0], [0, 0, 0], [0, arm * fixture.dir_y, 0]], 0x8fa3bf, 3.0);

      // The work zero itself: a short cross, so it is findable when the bracket runs off
      // the edge of a close-in view.
      const tick = arm / 4;
      line([[-tick, 0, 0], [tick, 0, 0]], 0xff5c5c, 2.5);
      line([[0, -tick, 0], [0, tick, 0]], 0xff5c5c, 2.5);

      // The pin holes, as rings at Z0. Rings and not board openings: they are holes in the
      // blank and the backboard, not in the board, and adding them to the workpiece would
      // put two holes through a PCB that does not have them.
      (fixture.pins || []).forEach(function (pin) {
        const radius = pin[2] / 2;
        const points = [];
        for (let n = 0; n <= 24; n++) {
          const angle = (Math.PI * 2 * n) / 24;
          points.push([pin[0] + radius * Math.cos(angle), pin[1] + radius * Math.sin(angle), 0]);
        }
        line(points, 0x8fa3bf, 2.5);
      });
    }

    // The toolpaths, from the payload Rust built. One Line2 per (tool, run) — the
    // extraction merges consecutive moves of the same kind, so this is a handful of
    // objects per tool rather than one per move.
    function addTraces(traces) {
      (traces || []).forEach(function (trace) {
        trace.moves.forEach(function (run) {
          const flat = [];
          run.points.forEach(function (p) { flat.push(p.x, p.y, p.z); });
          if (flat.length < 6) return;

          const geometry = new T.LineGeometry();
          geometry.setPositions(flat);
          // Both kinds carry the tool's colour, so the legend identifies every line on
          // screen. What separates them is the dash: a rapid is the tool travelling, a
          // solid run is the tool cutting. Colour alone used to do this job — rapids were
          // grey — but on a drilling step the cuts are only the short vertical plunges and
          // every transit between holes is a rapid, so the picture was almost entirely
          // grey and told you nothing about which tool was where.
          const rapid = run.kind === "rapid";
          const material = new T.LineMaterial({
            color: trace.colour,
            linewidth: rapid ? 1.5 : 3.5,
            dashed: rapid,
            dashSize: 1.2,
            gapSize: 1.2,
            transparent: rapid,
            opacity: rapid ? 0.6 : 1.0,
            worldUnits: false,
          });
          materials.push(material);
          const line = new T.Line2(geometry, material);
          // Dashes come out of the line's own distance attribute, and without this the
          // material's `dashed` flag renders a solid line and reports nothing.
          if (rapid) line.computeLineDistances();
          content.add(line);
        });
      });
    }

    // Frame whatever is in the scene. The work sits in the positive quadrant with the
    // board's min corner on the machine origin (see gcode::placement), so the centre is
    // never (0,0) and a fixed camera position would always look past the board. Fitting
    // the bounding box instead means the view is correct for any board size.
    //
    // A redraw only re-frames when the work has materially moved or resized. Switching
    // an operation off barely shifts the bounds, and snapping the camera back to the
    // default three-quarter view every time a checkbox is ticked would undo the orbit the
    // user just made to look at the thing they are changing — whereas a different board
    // would otherwise be left framed for the previous one.
    function frameScene() {
      const bounds = new T.Box3().setFromObject(scene);
      if (bounds.isEmpty()) return;
      const centre = bounds.getCenter(new T.Vector3());
      const size = bounds.getSize(new T.Vector3());
      const span = Math.max(size.x, size.y, size.z) || 1;
      if (framed &&
          centre.distanceTo(framed.centre) < framed.span * REFRAME_RATIO &&
          Math.abs(span - framed.span) < framed.span * REFRAME_RATIO) {
        return;
      }
      // Back off far enough that the largest dimension fits the vertical field of view,
      // with a little air around it.
      const distance = (span / 2) / Math.tan((camera.fov * Math.PI) / 360) * 1.6;
      controls.target.copy(centre);
      // Looking from the front-right and above: the orientation an operator stands in.
      camera.position.set(
        centre.x + distance * 0.55,
        centre.y - distance * 0.75,
        centre.z + distance * 0.55
      );
      camera.near = Math.max(distance / 1000, 0.01);
      camera.far = distance * 10;
      camera.updateProjectionMatrix();
      controls.update();
      framed = { centre: centre.clone(), span: span };
    }

    function resize() {
      const w = canvas.clientWidth || 1, h = canvas.clientHeight || 1;
      renderer.setSize(w, h, false);
      camera.aspect = w / h;
      camera.updateProjectionMatrix();
      materials.forEach(function (m) { m.resolution.set(w, h); });
    }

    // The whole of what a plan change changes. Kept on the canvas because that is the
    // only handle a later `eval` — which runs in page scope, with no reference to this
    // closure — can find it by.
    canvas.__k2g_draw = function (board, traces, fixture) {
      clearContent();
      addBoard(board);
      addFixture(fixture);
      addTraces(traces);
      resize();          // the new fat-line materials have no resolution until this runs
      frameScene();
    };

    new ResizeObserver(resize).observe(canvas);
    canvas.__k2g_draw(board, traces, fixture);

    // Frames the canvas has been out of the document for. The canvas is destroyed when
    // the Job view switches tab or the dock closes, and this loop would otherwise keep a
    // dead renderer — and its WebGL context, of which browsers allow ~16 — alive for the
    // life of the page, so a dozen tab switches would exhaust them. Given a grace period
    // rather than acted on at once because the teardown is irreversible: a canvas the
    // renderer has released cannot get its context back, so a detach that turns out to be
    // momentary must not be fatal.
    let orphaned = 0;
    const ORPHAN_FRAMES = 60;

    (function frame() {
      if (!canvas.isConnected) {
        if (++orphaned > ORPHAN_FRAMES) {
          canvas.__k2g_draw = null;
          renderer.forceContextLoss();
          renderer.dispose();
          return;
        }
        requestAnimationFrame(frame);
        return;                  // nothing to draw into, but keep counting
      }
      orphaned = 0;
      requestAnimationFrame(frame);
      controls.update();
      renderer.render(scene, camera);
    })();

    // The renderer clears to transparent (alpha: true) so the panel's own themed
    // background shows through, which means the placeholder behind it would too.
    // Retire it now that there is something to look at.
    const placeholder = canvas.parentElement &&
      canvas.parentElement.querySelector(".machining-3d-placeholder");
    if (placeholder) placeholder.remove();

    dioxus.send("");    // empty == the bootstrap got all the way through
  } catch (err) {
    canvas.__k2g_draw = null;
    dioxus.send(String((err && err.stack) || err));
  }
})();
"#;

/// Everything the scene script draws, already serialised.
///
/// One `PartialEq` value so a [`use_memo`] can tell whether there is anything new to
/// draw. The plan is re-derived on every state change, but most changes leave the
/// geometry byte-identical — a theme toggle, a rename, a log tick — and re-evaluating the
/// script for those would rebuild the scene for nothing.
#[derive(Clone, PartialEq)]
struct ScenePayload {
    /// The serialised [`ToolTrace`](crate::gcode::scene::ToolTrace) list.
    traces: String,
    /// The serialised [`BoardSolid`](crate::gcode::scene::BoardSolid), or `null`.
    board: String,
    /// The serialised [`FixtureMark`](crate::gcode::scene::FixtureMark), or `null`.
    fixture: String,
    tool_count: usize,
    point_count: usize,
    /// Every tool of the step, hidden ones included — the legend is what switches them
    /// back on. Part of the payload so the memo that suppresses no-op redraws suppresses
    /// no-op legend rebuilds with it.
    legend: Vec<LegendRow>,
}

/// One tool in the legend beside the canvas.
///
/// Built from the same [`ToolTrace`](crate::gcode::scene::ToolTrace) list the renderer is
/// handed, so the swatch cannot show a colour the lines do not use.
#[derive(Clone, PartialEq)]
struct LegendRow {
    /// The tool's stock id — what the hidden set is keyed by, and what survives a step's
    /// blocks being reordered when a position would not.
    tool_id: String,
    /// `#rrggbb`, for the swatch.
    swatch: String,
    /// `T1 · 0.8mm drill ⌀0.80`, or the raw id when the tool is no longer in stock.
    label: String,
    hidden: bool,
}

/// Names a trace's tool the way the Machining summary does, so the two read as one job.
///
/// Falls back to the raw id rather than to "unknown": a tool removed from stock after a
/// plan was made still has lines on screen, and a legend that cannot name them must still
/// let them be switched off.
fn legend_label(tools: &[crate::data::model::Tool], trace: &scene::ToolTrace) -> String {
    let slot = trace.slot.map(|n| format!("T{n}")).unwrap_or_else(|| "—".to_string());
    let name = tools
        .iter()
        .find(|tool| tool.id == trace.tool_id)
        .map(|tool| tool.display_name())
        .unwrap_or_else(|| trace.tool_id.clone());
    format!("{slot} · {name} ⌀{:.2}", trace.diameter_mm)
}

impl ScenePayload {
    /// Plans the machining and flattens it into what the renderer is handed.
    ///
    /// All of it decided here, in Rust: the renderer never sees a `MachiningPlan`, it
    /// gets points and colours, which is the entire reason the untested half of this
    /// feature stays small.
    /// One step, not the whole plan: a step is a physical setup, and compositing two
    /// setups' toolpaths into one scene would draw motions that never coexist.
    ///
    /// `hidden` names tools to leave out. Filtering **here** rather than in the script is
    /// what keeps the JavaScript free of a visibility API: the redraw path already
    /// rebuilds the scene wholesale, and `frameScene`'s hysteresis already exists so that
    /// dropping geometry does not snap the camera back. The colour survives the filter
    /// because it was fixed to the trace when it was built, not by counting position in
    /// this list.
    fn build(ctx: &AppCtx, step: usize, hidden: &BTreeSet<String>) -> Self {
        let plan = plan_machining(ctx);
        let all = plan.steps.get(step).map(scene::trace_step).unwrap_or_default();

        // The legend lists every tool of the step, hidden ones included — it is the only
        // way back to one that has been switched off.
        let legend: Vec<LegendRow> = all
            .iter()
            .map(|trace| LegendRow {
                swatch: format!("#{:06x}", trace.colour),
                label: legend_label(&ctx.tools, trace),
                hidden: hidden.contains(&trace.tool_id),
                tool_id: trace.tool_id.clone(),
            })
            .collect();

        let shown: Vec<&scene::ToolTrace> =
            all.iter().filter(|trace| !hidden.contains(&trace.tool_id)).collect();
        let board = machining_plan::board_solid(ctx, step);
        // Not filtered by the legend: the origin, the stop and the pins are the frame the
        // program is written in, not one tool's work, so switching a tool off must not take
        // the coordinate system with it.
        let fixture = machining_plan::fixture_scene(ctx, step);
        Self {
            point_count: shown.iter().map(|t| t.point_count()).sum(),
            tool_count: shown.len(),
            traces: serde_json::to_string(&shown).unwrap_or_else(|_| "[]".to_string()),
            board: serde_json::to_string(&board).unwrap_or_else(|_| "null".to_string()),
            fixture: serde_json::to_string(&fixture).unwrap_or_else(|_| "null".to_string()),
            legend,
        }
    }
}

/// The 3D toolpath canvas.
///
/// Sized entirely by CSS: the canvas has no width/height attributes, and the script's
/// `ResizeObserver` follows the element. Setting them in the markup would fight the
/// layout and give a blurry, stretched drawing on a HiDPI display.
#[component]
pub fn Machining3dView(state: Signal<AppCtx>) -> Element {
    // Two sources have to be watched, because a plan has two ways of changing. The
    // fixture, stock and board come off `state`; the machining profile — the operation
    // toggles, the per-operation settings — is a schema-driven AppData edit that only
    // announces itself through the store revision.
    // Tools switched off in the legend, by stock id.
    //
    // Keyed by id and not by position: a step's blocks are ordered by the planner, and a
    // profile edit can reorder them. An index would then carry the hidden flag onto
    // whichever tool inherited the position.
    let mut hidden = use_signal(BTreeSet::<String>::new);

    let payload = use_memo(move || {
        let _ = crate::ui::bindings::data_revision();
        let snapshot = state.read().clone();
        let step = snapshot.selected_step;
        // Read inside the memo so ticking a box rebuilds the payload; read outside, the
        // toggle would restyle the legend and never reach the canvas.
        ScenePayload::build(&snapshot, step, &hidden.read())
    });

    // `use_effect` runs after the node exists, which is the whole point — the script
    // looks the canvas up by id and would find nothing if this ran during render.
    //
    // The payload is read *inside* the closure, and that read is what subscribes the
    // effect: `use_effect` registers its closure once, so deriving the payload in the
    // component body and capturing it here — which is what this did — pinned the view to
    // the plan as it stood on first render, and no later edit ever reached the canvas.
    use_effect(move || {
        let payload = payload();
        spawn(async move {
            let ScenePayload { traces, board, fixture, tool_count, point_count, .. } = payload;
            // Logged at info, not debug: the default filter is `info`, and a 3D view
            // that quietly does nothing is the failure mode this whole module exists to
            // prevent. Both ends are logged so a hang in between is distinguishable
            // from a script that never ran.
            log::info!(
                "3D machining view: drawing {tool_count} tool(s), {point_count} point(s)"
            );
            let script = BOOTSTRAP_SCRIPT
                .replace("CANVAS_ID_PLACEHOLDER", CANVAS_ID)
                .replace("TRACES_PLACEHOLDER", &traces)
                .replace("BOARD_PLACEHOLDER", &board)
                .replace("FIXTURE_PLACEHOLDER", &fixture);
            match document::eval(&script).recv::<String>().await {
                Ok(message) if message.is_empty() => {
                    log::info!("3D machining view: drawn");
                }
                Ok(message) => log::error!("3D machining view failed to draw: {message}"),
                Err(err) => log::error!("3D machining view could not be evaluated: {err}"),
            }
            report_page_errors().await;
        });
    });

    let legend = payload().legend;

    rsx! {
        div { class: "machining-3d-layout",
            // The canvas keeps `.machining-3d` as its **direct parent**: the script finds
            // the "starting renderer…" placeholder through `canvas.parentElement`, so the
            // legend goes beside this div rather than inside it.
            div { class: "machining-3d",
                // Sits behind the canvas. If the canvas never paints, this is what shows —
                // so an empty box always says which half broke instead of looking like a
                // styling mistake.
                div { class: "machining-3d-placeholder", "3D toolpath — starting renderer…" }
                canvas { id: CANVAS_ID, class: "machining-3d-canvas" }
            }

            if !legend.is_empty() {
                aside { class: "machining-3d-legend",
                    div { class: "machining-3d-legend-title", "Tools" }
                    for row in legend.iter() {
                        label {
                            key: "{row.tool_id}",
                            class: if row.hidden { "machining-3d-legend-row is-hidden" } else { "machining-3d-legend-row" },
                            input {
                                r#type: "checkbox",
                                checked: !row.hidden,
                                oninput: {
                                    let tool_id = row.tool_id.clone();
                                    move |evt: FormEvent| {
                                        let shown = evt.checked();
                                        hidden
                                            .with_mut(|off| {
                                                if shown {
                                                    off.remove(&tool_id);
                                                } else {
                                                    off.insert(tool_id.clone());
                                                }
                                            });
                                    }
                                },
                            }
                            span {
                                class: "machining-3d-legend-swatch",
                                style: "background: {row.swatch}",
                            }
                            span { class: "machining-3d-legend-label", "{row.label}" }
                        }
                    }
                }
            }
        }
    }
}

/// Drains anything the page's error trap caught and logs it.
///
/// Separate from the bootstrap's own result because these are the errors *nobody
/// caught* — a three.js parse failure in the head, or a throw inside the animation
/// frame, neither of which the `try` above can see.
async fn report_page_errors() {
    match document::eval(crate::ui::DRAIN_ERRORS).recv::<Vec<String>>().await {
        Ok(errors) => {
            for error in errors {
                log::error!("WebView error: {error}");
            }
        }
        Err(err) => log::debug!("could not drain WebView errors: {err}"),
    }
}

#[cfg(test)]
mod legend_tests {
    use super::*;
    use crate::data::model::Tool;

    fn trace(tool_id: &str, slot: Option<u8>) -> scene::ToolTrace {
        scene::ToolTrace {
            tool_id: tool_id.to_string(),
            colour: 0x4ea3ff,
            slot,
            diameter_mm: 0.8,
            change_at: None,
            moves: vec![],
        }
    }

    /// A stock tool. Only `id` and `name` matter here — the rest is filler, because
    /// `legend_label` reads nothing else.
    fn stock(id: &str, name: &str) -> Tool {
        Tool {
            id: id.to_string(),
            composite_name: name.to_string(),
            name: name.to_string(),
            kind: "Drill bit".to_string(),
            diameter: units::Length::from_mm(0.8),
            catalog_diameter: None,
            point_angle: units::Angle::from_degrees(118.0),
            catalog_point_angle: None,
            flute_length: None,
            table_feed: None,
            catalog_table_feed: None,
            z_feed: None,
            catalog_z_feed: None,
            spindle_speed: None,
            catalog_spindle_speed: None,
            status: crate::data::model::ToolStatus::InStock,
            preference: crate::data::model::ToolPreference::Neutral,
            source_catalog: String::new(),
            manufacturer: None,
            sku: None,
        }
    }

    /// The legend names a tool the way the Machining summary does, so the two read as
    /// one job rather than as two lists that happen to be about the same step.
    #[test]
    fn a_tool_in_stock_is_named_with_its_slot_and_diameter() {
        let tools = vec![stock("t1", "0.8mm carbide drill")];
        assert_eq!(
            legend_label(&tools, &trace("t1", Some(3))),
            "T3 · 0.8mm carbide drill ⌀0.80"
        );
    }

    /// A tool that has left stock since the plan was made still has lines on screen. The
    /// row must survive so they can be switched off — a legend that silently omitted it
    /// would leave paths nobody could account for or hide.
    #[test]
    fn a_tool_no_longer_in_stock_falls_back_to_its_id() {
        let label = legend_label(&[], &trace("019f-deleted", Some(1)));
        assert_eq!(label, "T1 · 019f-deleted ⌀0.80");
    }

    /// A manual-change machine assigns no rack slot; the row still needs a left-hand
    /// column so the names line up down the panel.
    #[test]
    fn a_tool_with_no_rack_slot_still_lines_up() {
        let tools = vec![stock("t1", "1mm router")];
        assert_eq!(legend_label(&tools, &trace("t1", None)), "— · 1mm router ⌀0.80");
    }
}

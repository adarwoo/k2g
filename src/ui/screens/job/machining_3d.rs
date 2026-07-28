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
/// serialised [`BoardSolid`](crate::gcode::scene::BoardSolid)) and `PALETTE_PLACEHOLDER`.
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
  const PALETTE = PALETTE_PLACEHOLDER;
  const T = window.K2G_THREE;
  if (!T) { dioxus.send("three.js global K2G_THREE is missing — the head script did not run"); return; }
  const canvas = document.getElementById("CANVAS_ID_PLACEHOLDER");
  if (!canvas) { dioxus.send("canvas #CANVAS_ID_PLACEHOLDER not found"); return; }

  const board = BOARD_PLACEHOLDER;
  const traces = TRACES_PLACEHOLDER;

  // Already running on this canvas: hand the new plan to the renderer that is up and
  // return. Everything below this line is one-per-canvas and must not run twice.
  if (canvas.__k2g_draw) {
    try { canvas.__k2g_draw(board, traces); dioxus.send(""); }
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
    }

    // The toolpaths, from the payload Rust built. One Line2 per (tool, run) — the
    // extraction merges consecutive moves of the same kind, so this is a handful of
    // objects per tool rather than one per move.
    function addTraces(traces) {
      (traces || []).forEach(function (trace, index) {
        const colour = PALETTE[index % PALETTE.length];
        trace.moves.forEach(function (run) {
          const flat = [];
          run.points.forEach(function (p) { flat.push(p.x, p.y, p.z); });
          if (flat.length < 6) return;

          const geometry = new T.LineGeometry();
          geometry.setPositions(flat);
          // Rapids read as scaffolding, cuts as the work — thin and dim against thick
          // and saturated. The convention every backplot worth using shares.
          const rapid = run.kind === "rapid";
          const material = new T.LineMaterial({
            color: rapid ? 0x55606f : colour,
            linewidth: rapid ? 1 : 2.5,
            transparent: rapid,
            opacity: rapid ? 0.4 : 1.0,
            worldUnits: false,
          });
          materials.push(material);
          content.add(new T.Line2(geometry, material));
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
    canvas.__k2g_draw = function (board, traces) {
      clearContent();
      addBoard(board);
      addTraces(traces);
      resize();          // the new fat-line materials have no resolution until this runs
      frameScene();
    };

    new ResizeObserver(resize).observe(canvas);
    canvas.__k2g_draw(board, traces);

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
    tool_count: usize,
    point_count: usize,
}

impl ScenePayload {
    /// Plans the machining and flattens it into what the renderer is handed.
    ///
    /// All of it decided here, in Rust: the renderer never sees a `MachiningPlan`, it
    /// gets points and colours, which is the entire reason the untested half of this
    /// feature stays small.
    /// One step, not the whole plan: a step is a physical setup, and compositing two
    /// setups' toolpaths into one scene would draw motions that never coexist.
    fn build(ctx: &AppCtx, step: usize) -> Self {
        let plan = plan_machining(ctx);
        let traces = plan.steps.get(step).map(scene::trace_step).unwrap_or_default();
        let board = machining_plan::board_solid(ctx, step);
        Self {
            point_count: traces.iter().map(|t| t.point_count()).sum(),
            tool_count: traces.len(),
            traces: serde_json::to_string(&traces).unwrap_or_else(|_| "[]".to_string()),
            board: serde_json::to_string(&board).unwrap_or_else(|_| "null".to_string()),
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
    let payload = use_memo(move || {
        let _ = crate::ui::bindings::data_revision();
        let snapshot = state.read().clone();
        let step = snapshot.selected_step;
        ScenePayload::build(&snapshot, step)
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
            let ScenePayload { traces, board, tool_count, point_count } = payload;
            // Logged at info, not debug: the default filter is `info`, and a 3D view
            // that quietly does nothing is the failure mode this whole module exists to
            // prevent. Both ends are logged so a hang in between is distinguishable
            // from a script that never ran.
            log::info!(
                "3D machining view: drawing {tool_count} tool(s), {point_count} point(s)"
            );
            let palette =
                serde_json::to_string(&scene::TOOL_PALETTE).unwrap_or_else(|_| "[]".to_string());
            let script = BOOTSTRAP_SCRIPT
                .replace("CANVAS_ID_PLACEHOLDER", CANVAS_ID)
                .replace("TRACES_PLACEHOLDER", &traces)
                .replace("PALETTE_PLACEHOLDER", &palette)
                .replace("BOARD_PLACEHOLDER", &board);
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

    rsx! {
        div { class: "machining-3d",
            // Sits behind the canvas. If the canvas never paints, this is what shows —
            // so an empty box always says which half broke instead of looking like a
            // styling mistake.
            div { class: "machining-3d-placeholder", "3D toolpath — starting renderer…" }
            canvas { id: CANVAS_ID, class: "machining-3d-canvas" }
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

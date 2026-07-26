//! The 3D machining view — a WebGL toolpath render inside the WebView.
//!
//! Draws what [`scene::trace_plan`](crate::gcode::scene::trace_plan) extracts from the
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
//! into the Logs screen after every mount.

use dioxus::prelude::*;

use crate::gcode::scene;
use crate::runtime::machining_plan::plan_machining;
use crate::runtime::AppCtx;

/// The DOM id the canvas is mounted under. The script finds it rather than being handed
/// a node, because `eval` runs in page scope with no reference to Dioxus's tree.
const CANVAS_ID: &str = "k2g-machining-3d";

/// The scene script. Three placeholders are substituted before it is evaluated:
/// `CANVAS_ID_PLACEHOLDER`, `TRACES_PLACEHOLDER` (the serialised
/// [`ToolTrace`](crate::gcode::scene::ToolTrace) list) and `PALETTE_PLACEHOLDER`.
///
/// Substitution rather than `dioxus.send` from the Rust side because the payload is
/// needed *before* the first frame, and a script that has its data inlined cannot race
/// with the channel.
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

  // Idempotent: the view re-renders on every plan change, and a second renderer on the
  // same canvas would leak a WebGL context (browsers cap those at ~16).
  if (canvas.__k2g_started) { dioxus.send(""); return; }
  canvas.__k2g_started = true;

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

    // The toolpaths, from the payload Rust built. One Line2 per (tool, run) — the
    // extraction merges consecutive moves of the same kind, so this is a handful of
    // objects per tool rather than one per move.
    //
    // Fat lines are sized in *screen* space, so every material's `resolution` has to
    // follow the canvas; they are collected here for `resize` to update.
    const materials = [];
    const traces = TRACES_PLACEHOLDER;

    traces.forEach(function (trace, index) {
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
        scene.add(new T.Line2(geometry, material));
      });
    });

    // Frame whatever is in the scene. The work sits in the positive quadrant with the
    // board's min corner on the machine origin (see gcode::placement), so the centre is
    // never (0,0) and a fixed camera position would always look past the board. Fitting
    // the bounding box instead means the view is correct for any board size.
    function frameScene() {
      const bounds = new T.Box3().setFromObject(scene);
      if (bounds.isEmpty()) return;
      const centre = bounds.getCenter(new T.Vector3());
      const size = bounds.getSize(new T.Vector3());
      const span = Math.max(size.x, size.y, size.z) || 1;
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
    }
    frameScene();

    function resize() {
      const w = canvas.clientWidth || 1, h = canvas.clientHeight || 1;
      renderer.setSize(w, h, false);
      camera.aspect = w / h;
      camera.updateProjectionMatrix();
      materials.forEach(function (m) { m.resolution.set(w, h); });
    }
    new ResizeObserver(resize).observe(canvas);
    resize();

    (function frame() {
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
    canvas.__k2g_started = false;
    dioxus.send(String((err && err.stack) || err));
  }
})();
"#;

/// The 3D toolpath canvas.
///
/// Sized entirely by CSS: the canvas has no width/height attributes, and the script's
/// `ResizeObserver` follows the element. Setting them in the markup would fight the
/// layout and give a blurry, stretched drawing on a HiDPI display.
#[component]
pub fn Machining3dView(state: Signal<AppCtx>) -> Element {
    // Built here, in Rust, and handed over whole. The renderer never sees a
    // `MachiningPlan` — it gets points and colours, which is the entire reason the
    // untested half of this feature stays small.
    let traces = {
        let snapshot = state.read().clone();
        scene::trace_plan(&plan_machining(&snapshot))
    };
    let point_count: usize = traces.iter().map(|t| t.point_count()).sum();
    let tool_count = traces.len();
    let payload = serde_json::to_string(&traces).unwrap_or_else(|_| "[]".to_string());
    let palette = serde_json::to_string(&scene::TOOL_PALETTE).unwrap_or_else(|_| "[]".to_string());
    // `use_effect` runs after the node exists, which is the whole point — the script
    // looks the canvas up by id and would find nothing if this ran during render.
    use_effect(move || {
        let payload = payload.clone();
        let palette = palette.clone();
        spawn(async move {
            // Logged at info, not debug: the default filter is `info`, and a 3D view
            // that quietly does nothing is the failure mode this whole module exists to
            // prevent. Both ends are logged so a hang in between is distinguishable
            // from a script that never ran.
            log::info!(
                "3D machining view: starting WebGL bootstrap ({tool_count} tool(s), \
                 {point_count} point(s))"
            );
            let script = BOOTSTRAP_SCRIPT
                .replace("CANVAS_ID_PLACEHOLDER", CANVAS_ID)
                .replace("TRACES_PLACEHOLDER", &payload)
                .replace("PALETTE_PLACEHOLDER", &palette);
            match document::eval(&script).recv::<String>().await {
                Ok(message) if message.is_empty() => {
                    log::info!("3D machining view: WebGL bootstrap ok");
                }
                Ok(message) => log::error!("3D machining view failed to start: {message}"),
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

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
use std::sync::{Mutex, OnceLock};

use dioxus::prelude::*;

use crate::gcode::scene;
use crate::runtime::machining_plan::{self, cached_plan};
use crate::runtime::AppCtx;

/// Tools switched off in the legend, for as long as the application is running.
///
/// A `static` rather than component state because this view is *remounted* constantly:
/// switching the Job tab, pinning the dock beside a profile screen, or navigating away
/// and back all destroy the component and build another. Component state resets on each
/// of those, so a tool switched off to see underneath it came back the moment the screen
/// was split — which is what this fixes.
///
/// Session-scoped and deliberately not persisted: hiding a tool is a way of looking at
/// one job, not a preference to carry into the next one. It lives beside the view rather
/// than in `AppCtx` for the reason `runtime::removable` gives for its own static — this
/// is not job state, and putting it in the context would mean a full clone and a
/// post-mutation reconciliation every time a checkbox is ticked.
fn hidden_tools() -> &'static Mutex<BTreeSet<String>> {
    static HIDDEN: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();
    HIDDEN.get_or_init(|| Mutex::new(BTreeSet::new()))
}

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
  const highlight = HIGHLIGHT_PLACEHOLDER;

  // Already running on this canvas: hand the new plan to the renderer that is up and
  // return. Everything below this line is one-per-canvas and must not run twice.
  if (canvas.__k2g_draw) {
    try { canvas.__k2g_draw(board, traces, fixture, highlight); dioxus.send(""); }
    catch (err) { dioxus.send(String((err && err.stack) || err)); }
    return;
  }

  try {
    const renderer = new T.WebGLRenderer({ canvas: canvas, antialias: true, alpha: true });
    renderer.setPixelRatio(window.devicePixelRatio || 1);

    const scene = new T.Scene();
    // Two cameras, one active. The flat view is a real orthographic projection rather
    // than a very long lens: judging whether two paths line up is the thing this view is
    // for, and perspective moves them apart by however far they are from the middle.
    const persp = new T.PerspectiveCamera(45, 1, 0.1, 5000);
    const ortho = new T.OrthographicCamera(-1, 1, 1, -1, 0.1, 5000);
    persp.up.set(0, 0, 1);             // Z is up: machine convention, not screen convention.
    ortho.up.set(0, 0, 1);
    let camera = persp;

    const controls = new T.OrbitControls(camera, canvas);
    // Damping on, but barely. It is kept because it smooths the step between one mouse
    // sample and the next — dragging with it off is visibly notchy — and turned well up
    // from three.js's own 0.05 because that default is *inertia*, not smoothing: the view
    // carries on drifting for the best part of a second after the hand comes off, which on
    // a board being inspected feels like lag rather than like polish.
    //
    // The factor is the fraction of the remaining movement applied per frame, so larger is
    // less damping. At 0.3 the camera has caught up with the pointer within a few frames
    // and the drift is gone, while a single jumpy sample still does not snap the view.
    //
    // It has to stay non-zero for a second reason: `saveView` runs on the controls' `end`
    // event, which fires when the button is released — so anything still coasting after
    // that point is motion the remembered view never sees. Less damping is also less of
    // that discrepancy.
    controls.enableDamping = true;
    controls.dampingFactor = 0.3;

    // Which way up the board is lying, so the face views know which side of it to stand
    // on. Kept from the last `addBoard`; false until one arrives, which is the same
    // default the payload carries.
    let backFaceUp = false;

    // The board's two faces. Deep enough that every colour in `scene::TOOL_PALETTE`
    // reads as a line *on* the board rather than as a shade *of* it — which is what
    // they did not, before: engraving covers the whole face in dense paths, and a mint
    // toolpath on a bright green board was two greens rather than a path and a board.
    //
    // Measured rather than eyeballed, by rendering this exact material stack (Lambert,
    // these lights, 0.85/0.72 opacity) over both themes' backgrounds and sampling the
    // pixels. Taking the worse theme of the two: contrast against the palette over the
    // *green* face went 3.0:1 → 5.5:1 at its weakest, and the mint the engraving usually
    // draws in 4.6:1 → 8.3:1. The *red* face was worse than either at 2.8:1 — a rose
    // path on a red board — and is now 5.1:1.
    //
    // Named by colour rather than by face because the two faces have since swapped roles
    // (below) and the figures did not move with them: both were measured as a face over
    // the green slab, and the slab is still green, so the two stacks are the same two.
    //
    // Keep both above ~5:1 if these are ever retuned, and keep
    // them within about a stop of each other — they are the front/back cue, so one
    // reading deeper than the other would say something the board is not.
    //
    // Still green and still red, because that is the cue — but **red is the front**, to
    // agree with the 2D board view, which paints the front copper in KiCad's F.Cu red.
    // A soldermask-green front is the prettier metaphor and it lost: two views of one
    // board that disagree about which face is which is a way to machine the wrong side,
    // and no amount of realism is worth that.
    const BOARD_GREEN = 0x103a24;
    const BOARD_RED = 0x6b1f17;

    // Draw order: the board, then the toolpaths over it.
    //
    // The board is translucent and writes no depth precisely so it cannot hide the work
    // (see `addBoard`) — but that only covers *occlusion*. A face at 0.85 still blends
    // over any path beneath it, and geometry order alone cannot prevent it: an opaque
    // line renders in the earlier pass, so the board is composited on top afterwards
    // whatever the scene graph says. Looking straight down at the front face, that turned
    // the engraving into a faint tint of the board — which is exactly what these colours
    // were retuned to stop.
    //
    // Explicit order in the transparent pass is what fixes it, so every trace is drawn at
    // its own colour from any angle. The paths therefore also show through the board from
    // behind, which is what a toolpath view is for.
    const BOARD_ORDER = 0;
    const TRACE_ORDER = 1;
    // Above the traces, because the highlight redraws a line that is already there: at the
    // same order the two z-fight and the selected op flickers between its tool's colour and
    // the accent as the camera moves.
    const HIGHLIGHT_ORDER = 2;

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
      backFaceUp = !!board.back_face_up;
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
          color: BOARD_GREEN,
          side: T.DoubleSide,
          transparent: true,
          opacity: 0.72,
          depthWrite: false,
        })
      );
      // Z0 is the board's top surface (op-planner §6), so the slab hangs below it —
      // which is what puts the toolpaths' plunges *into* the material.
      slab.position.z = -board.thickness_mm;
      // Before the toolpaths, whatever the geometry says — see `addTraces`.
      slab.renderOrder = BOARD_ORDER;
      content.add(slab);

      // The two faces, in the board's own colours: the **front** is always red — as it is
      // in the 2D view — and the **back** always green, whichever way up the board happens
      // to be lying.
      //
      // Which one the spindle sees is what `back_face_up` says, so a front-face step shows
      // red with green underneath, and a back-face step shows green with red underneath.
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
        mesh.renderOrder = BOARD_ORDER;
        content.add(mesh);
      };
      const FRONT = BOARD_RED, BACK = BOARD_GREEN;
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

    // The toolpaths, from the payload Rust built. **Two objects per tool** — one for
    // everything it cuts and one for everything it travels — rather than one per run.
    //
    // A run used to be its own Line2, which was fine while a step was a few hundred
    // plunges. An isolation pass is 900 runs on one layer, and 900 draw calls a frame is
    // felt immediately on a machine falling back to software rendering, which is where
    // this runs when the GPU driver is not trusted (see WEBKIT_DISABLE_DMABUF_RENDERER).
    //
    // The geometry is the same either way: LineGeometry expands a polyline into segment
    // pairs internally, so pre-expanding them here and handing the lot to one
    // LineSegments2 costs no extra memory — it only stops the scene graph carrying a
    // thousand objects that all draw with the same two materials.
    // A run list flattened into the segment pairs LineSegmentsGeometry wants, split by
    // move kind. Shared with `addHighlight`, which needs exactly the same expansion and
    // differs only in the material it hangs on the result.
    function segmentsByKind(moves) {
      const bucket = { feed: [], rapid: [] };
      (moves || []).forEach(function (run) {
        const into = run.kind === "rapid" ? bucket.rapid : bucket.feed;
        const pts = run.points;
        for (let i = 1; i < pts.length; i++) {
          const a = pts[i - 1], b = pts[i];
          into.push(a.x, a.y, a.z, b.x, b.y, b.z);
        }
      });
      return bucket;
    }

    // One op, drawn over the top of the line it duplicates — how a row in the table beside
    // this is found among hundreds of lines on the canvas.
    //
    // Heavier than the trace beneath it and undashed whichever kind it is: this is not
    // saying "cut" or "travel", it is saying "this one", and a dashed highlight over a
    // dashed rapid reads as neither.
    function addHighlight(runs) {
      if (!runs || !runs.length) return;
      const bucket = segmentsByKind(runs);
      const flat = bucket.feed.concat(bucket.rapid);
      if (flat.length < 6) return;

      const geometry = new T.LineSegmentsGeometry();
      geometry.setPositions(flat);
      const material = new T.LineMaterial({
        // Magenta, and fixed rather than taken from the theme's accent. Two constraints
        // leave very little room: it has to be absent from TOOL_PALETTE, which already
        // spends blue, amber, green, rose, violet, yellow, cyan and orange — and it has to
        // read against *both* themed panel backgrounds, because the renderer clears to
        // transparent and the panel shows through. White fails the second on a light theme
        // and the accent fails the first, being the same blue as the palette's first tool.
        color: 0xff2ec4,
        linewidth: 6.0,
        transparent: true,
        opacity: 0.95,
        // Depth test off, so the selected op shows through the board when it is on the far
        // side. A highlight that can be hidden by the workpiece fails at the one job it
        // has, which is to say where something is.
        depthTest: false,
        worldUnits: false,
      });
      materials.push(material);
      const line = new T.LineSegments2(geometry, material);
      line.renderOrder = HIGHLIGHT_ORDER;
      content.add(line);
    }

    function addTraces(traces) {
      (traces || []).forEach(function (trace) {
        const bucket = segmentsByKind(trace.moves);

        ["feed", "rapid"].forEach(function (kind) {
          const flat = bucket[kind];
          if (flat.length < 6) return;

          const geometry = new T.LineSegmentsGeometry();
          geometry.setPositions(flat);
          // Both kinds carry the tool's colour, so the legend identifies every line on
          // screen. What separates them is the dash: a rapid is the tool travelling, a
          // solid run is the tool cutting. Colour alone used to do this job — rapids were
          // grey — but on a drilling step the cuts are only the short vertical plunges and
          // every transit between holes is a rapid, so the picture was almost entirely
          // grey and told you nothing about which tool was where.
          const rapid = kind === "rapid";
          const material = new T.LineMaterial({
            color: trace.colour,
            linewidth: rapid ? 1.5 : 3.5,
            dashed: rapid,
            dashSize: 1.2,
            gapSize: 1.2,
            // Transparent even at full opacity, and that is not a contradiction: only the
            // transparent pass honours `renderOrder`, and being drawn *after* the board is
            // the whole point (see `TRACE_ORDER`). An opaque line renders in the earlier
            // pass, where the board's 0.85 face then blends over it.
            transparent: true,
            opacity: rapid ? 0.6 : 1.0,
            worldUnits: false,
          });
          materials.push(material);
          const line = new T.LineSegments2(geometry, material);
          line.renderOrder = TRACE_ORDER;
          // Dashes come out of the line's own distance attribute, and without this the
          // material's `dashed` flag renders a solid line and reports nothing. Across a
          // batch the phase carries on from one segment to the next, including over the
          // jump between two separate transits — which shifts where a dash falls and
          // nothing else.
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
      // with a little air around it. Always the *perspective* camera's field of view,
      // even when the flat one is active: an orthographic camera has none, and this is
      // the distance both are framed from — see `syncOrtho`.
      const distance = (span / 2) / Math.tan((persp.fov * Math.PI) / 360) * 1.6;
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
      syncOrtho();
      controls.update();
      framed = { centre: centre.clone(), span: span };
      saveView();
    }

    // Gives the flat camera the frustum that frames what the 3D one would frame from
    // where it is standing. An orthographic projection has no field of view, so "how
    // much fits on screen" has to be derived from the orbit distance and the lens the
    // other camera would have used — otherwise switching projection also changes the
    // zoom, and the two views cannot be compared.
    function syncOrtho() {
      const distance = camera.position.distanceTo(controls.target) || 1;
      const height = 2 * distance * Math.tan((persp.fov * Math.PI) / 360);
      const aspect = (canvas.clientWidth || 1) / (canvas.clientHeight || 1);
      ortho.left = (-height * aspect) / 2;
      ortho.right = (height * aspect) / 2;
      ortho.top = height / 2;
      ortho.bottom = -height / 2;
      ortho.near = camera.near;
      ortho.far = camera.far;
      ortho.updateProjectionMatrix();
    }

    function resize() {
      const w = canvas.clientWidth || 1, h = canvas.clientHeight || 1;
      renderer.setSize(w, h, false);
      persp.aspect = w / h;
      persp.updateProjectionMatrix();
      if (camera === ortho) syncOrtho();
      materials.forEach(function (m) { m.resolution.set(w, h); });
    }

    // --- the view controls ---------------------------------------------------------
    //
    // The camera lives entirely on this side, so these do too: the Rust half calls
    // `__k2g_view` for its buttons and the key handler below calls it directly, which
    // means there is one implementation and no state to keep in step across the two.

    // The view the operator arranged, kept on `window` so it outlives the canvas.
    //
    // This canvas is destroyed and rebuilt constantly — switching the Job tab, pinning
    // the dock beside a profile screen, or navigating away and back (see the orphan
    // teardown at the foot of this script). Every rebuild handed back the default
    // three-quarter view, so splitting the screen threw away whatever the operator had
    // orbited to. `window` is the only thing here that survives a canvas.
    //
    // Session-scoped and deliberately not persisted: this is a way of looking at one
    // job, not a preference to carry into the next run.
    function saveView() {
      window.__K2G_VIEW = {
        pos: camera.position.toArray(),
        target: controls.target.toArray(),
        flat: camera === ortho,
        framed: framed && { centre: framed.centre.toArray(), span: framed.span },
      };
    }

    // Puts the remembered view back, but only when it still belongs to this work.
    //
    // The saved camera was arranged around a particular bounding box. Restoring it over a
    // different board — another job, another step with far more or less in it — would
    // leave the operator looking past the work with no clue why, so the same "materially
    // moved or resized" test `frameScene` uses decides it, and a genuine change keeps the
    // fresh framing instead.
    function restoreView(saved) {
      if (!saved || !saved.framed || !framed) return;
      const centre = new T.Vector3().fromArray(saved.framed.centre);
      if (centre.distanceTo(framed.centre) > framed.span * REFRAME_RATIO ||
          Math.abs(saved.framed.span - framed.span) > framed.span * REFRAME_RATIO) {
        return;
      }
      setProjection(!!saved.flat);
      camera.position.fromArray(saved.pos);
      controls.target.fromArray(saved.target);
      syncOrtho();
      controls.update();
      saveView();
    }

    // Swaps which camera the controls drive, keeping the eye where it is. `zoom` is reset
    // rather than carried across: the orbit dolly means different things to the two
    // projections (a distance to one, a scale factor to the other), and `syncOrtho` is
    // what makes the flat view match the distance.
    function setProjection(flat) {
      const next = flat ? ortho : persp;
      if (next === camera) return;
      next.position.copy(camera.position);
      next.up.copy(camera.up);
      next.near = camera.near;
      next.far = camera.far;
      next.zoom = 1;
      camera = next;
      controls.object = camera;
      syncOrtho();
      camera.updateProjectionMatrix();
      controls.update();
    }

    // Stands the camera off the wanted face of the board.
    //
    // The board's front is the red face and its back the green one *whichever way up it
    // is lying*, so on a back-face step — where the board has been turned over — the
    // front face is the one underneath and this looks up at it from below. That is the
    // point: it is the view that shows whether the mirrored artwork came out right.
    function faceView(wantFront) {
      const frontIsUp = !backFaceUp;
      const above = wantFront ? frontIsUp : !frontIsUp;
      const sign = above ? 1 : -1;
      const centre = controls.target;
      const distance = camera.position.distanceTo(centre) || 1;
      // A few degrees off the axis rather than straight down it. `up` is +Z, so a camera
      // exactly overhead sits on the singular axis of the orbit maths, where which way is
      // up on screen is whatever the azimuth happens to be. The tilt keeps it predictable
      // — X to the right, Y away — and still reads as face-on.
      camera.position.set(
        centre.x,
        centre.y - sign * distance * 0.08,
        centre.z + sign * distance * 0.997
      );
      syncOrtho();
      controls.update();
    }

    canvas.__k2g_view = function (command) {
      if (command === "reset") {
        // Clearing `framed` is what makes this a reset rather than a no-op: `frameScene`
        // holds still for small changes, and after an orbit the bounds have not changed
        // at all.
        framed = null;
        frameScene();
      } else if (command === "front") {
        faceView(true);
      } else if (command === "back") {
        faceView(false);
      } else if (command === "projection") {
        setProjection(camera !== ortho);
      }
      saveView();
    };

    // Number keys, matching the buttons. On `window` because the canvas is not focusable
    // and nobody clicks a 3D view before using it — but that means every text field in
    // the application would otherwise eat a digit, hence the guards: anything typed into
    // an editable element, or with a modifier held, is not for us.
    const onKey = function (event) {
      if (event.altKey || event.ctrlKey || event.metaKey) return;
      const target = event.target;
      if (target && (target.isContentEditable ||
                     /^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName || ""))) return;
      if (!canvas.isConnected || !canvas.__k2g_view) return;
      const command = { "0": "reset", "1": "front", "2": "back", "3": "projection" }[event.key];
      if (!command) return;
      event.preventDefault();
      canvas.__k2g_view(command);
    };
    window.addEventListener("keydown", onKey);

    // The whole of what a plan change changes. Kept on the canvas because that is the
    // only handle a later `eval` — which runs in page scope, with no reference to this
    // closure — can find it by.
    canvas.__k2g_draw = function (board, traces, fixture, highlight) {
      clearContent();
      addBoard(board);
      addFixture(fixture);
      addTraces(traces);
      addHighlight(highlight);
      resize();          // the new fat-line materials have no resolution until this runs
      frameScene();
    };

    new ResizeObserver(resize).observe(canvas);

    // On "end" rather than "change": the latter fires every frame of a drag, and this
    // only has to be right once the hand comes off.
    controls.addEventListener("end", saveView);

    // Read before the first draw, because that draw frames the scene and records the
    // framing it chose — which would overwrite the very thing being restored.
    const remembered = window.__K2G_VIEW;
    canvas.__k2g_draw(board, traces, fixture, highlight);
    // After the first draw, never before: that draw is what establishes the bounds the
    // remembered view is judged against.
    restoreView(remembered);

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
          canvas.__k2g_view = null;
          // Goes with the renderer: the listener is on `window`, which outlives every
          // canvas, so leaving it behind would keep this whole closure — scene, geometry
          // and all — reachable for the life of the page.
          window.removeEventListener("keydown", onKey);
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

/// The view buttons: `(command, key, label, tooltip)`.
///
/// The command strings are the script's `__k2g_view` vocabulary and the keys are the ones
/// its `keydown` handler maps, so this table and that handler have to agree — they are
/// twelve lines apart in the same file for exactly that reason.
const VIEW_CONTROLS: [(&str, &str, &str, &str); 4] = [
    ("reset", "0", "Reset", "Frame the whole job again, from the usual three-quarter view"),
    (
        "front",
        "1",
        "Front",
        "Look at the board's front (green) face — from underneath, on a step that machines the back",
    ),
    ("back", "2", "Back", "Look at the board's back (red) face"),
    (
        "projection",
        "3",
        "Flat / 3D",
        "Switch between a flat (orthographic) view, where paths that line up look like it, and the 3D one",
    ),
];

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
    /// The op the operator has picked in the list beside this, as its own serialised run
    /// list — or `null`. Drawn over the scene so a row in the table can be found among the
    /// lines on the canvas.
    ///
    /// Part of the payload rather than sent on a channel of its own, so the memo that
    /// suppresses no-op redraws covers a selection that has not changed too. It is also why
    /// changing the selection is cheap: the re-eval finds `__k2g_draw` already on the canvas
    /// and reuses the renderer, so a click costs a rebuild of the line objects and not of
    /// the scene.
    highlight: String,
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
    fn build(
        ctx: &AppCtx,
        step: usize,
        hidden: &BTreeSet<String>,
        picked: Option<crate::ui::screens::job::machining::OpRef>,
    ) -> Self {
        let plan = cached_plan(ctx);
        let all = plan.steps.get(step).map(scene::trace_step).unwrap_or_default();

        // Built from the plan rather than looked for in `all`: the traces have had their
        // op boundaries merged away for the renderer's sake, so the only way to point at
        // one op is to build that op again. See `scene::trace_op`.
        //
        // A hidden tool's op is not highlighted. Switching a tool off is a statement that
        // its lines should not be on screen, and an exception for the selected one would
        // draw the very line the operator has just hidden.
        let highlight = picked
            .filter(|at| {
                plan.steps
                    .get(step)
                    .and_then(|step| step.blocks.get(at.block))
                    .is_some_and(|block| !hidden.contains(&block.tool_id))
            })
            .and_then(|at| {
                let block = plan.steps.get(step)?.blocks.get(at.block)?;
                scene::trace_op(block, at.op)
            });

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
            highlight: serde_json::to_string(&highlight).unwrap_or_else(|_| "null".to_string()),
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
pub fn Machining3dView(
    state: Signal<AppCtx>,
    /// The op picked in the list beside this.
    ///
    /// **A signal, not the plain `Option` it looks like it could be.** The payload below is
    /// a [`use_memo`], which registers its closure once and re-runs it when something it
    /// *reads* changes — so a plain prop is captured at first render and frozen there. As a
    /// value this arrived as `None`, stayed `None`, and no click ever reached the canvas;
    /// as a signal, reading it inside the memo is what subscribes the memo to it. Exactly
    /// the trap the note on `use_effect` below describes, one layer up.
    ///
    /// Carries the *raw* selection rather than a validated one, because the memo has the
    /// plan in hand and `ScenePayload::build` bounds-checks it anyway. The list beside this
    /// does its own resolving for its own highlight.
    picked: Signal<Option<crate::ui::screens::job::machining::OpRef>>,
) -> Element {
    // Two sources have to be watched, because a plan has two ways of changing. The
    // fixture, stock and board come off `state`; the machining profile — the operation
    // toggles, the per-operation settings — is a schema-driven AppData edit that only
    // announces itself through the store revision.
    // Tools switched off in the legend, by stock id.
    //
    // Keyed by id and not by position: a step's blocks are ordered by the planner, and a
    // profile edit can reorder them. An index would then carry the hidden flag onto
    // whichever tool inherited the position.
    let mut hidden = use_signal(|| hidden_tools().lock().map(|set| set.clone()).unwrap_or_default());

    let payload = use_memo(move || {
        let _ = crate::ui::bindings::data_revision();
        let snapshot = state.read().clone();
        let step = snapshot.selected_step;
        // Both read inside the memo, and for one reason: a read in here is what subscribes
        // the memo to them. Read outside, ticking a legend box would restyle the legend and
        // never reach the canvas, and clicking an op row would do nothing at all.
        ScenePayload::build(&snapshot, step, &hidden.read(), *picked.read())
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
            let ScenePayload { traces, board, fixture, highlight, tool_count, point_count, .. } =
                payload;
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
                .replace("FIXTURE_PLACEHOLDER", &fixture)
                .replace("HIGHLIGHT_PLACEHOLDER", &highlight);
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

    // Whether the contours this view is waiting on are still being worked out.
    //
    // Read **after** the payload, and that order matters: building the payload is what
    // plans the step, and planning a step whose contours are not in hand is what asks for
    // them. Read first, this would miss the request it is reporting on and the widget
    // would appear one render late.
    //
    // It clears without anything watching it: the worker wakes the UI when it publishes,
    // the context re-syncs, and this renders again with nothing in flight.
    let working_on = crate::runtime::isolation::in_flight();

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

                // The same four commands the number keys run. Present because a 3D view
                // with no visible controls tells nobody it has any — the keys are the
                // fast path, these are how they are found.
                //
                // Stateless commands, deliberately. The projection one toggles rather
                // than showing which mode is on: the mode lives in the script (where the
                // key handler is), and a label mirrored over here would be one re-render
                // away from claiming the opposite of what is on screen. The picture is
                // the readout.
                div { class: "machining-3d-controls",
                    for (command , key , label , hint) in VIEW_CONTROLS {
                        button {
                            key: "{command}",
                            class: "machining-3d-control",
                            r#type: "button",
                            title: "{hint}",
                            onclick: move |_| {
                                spawn(async move {
                                    let script = format!(
                                        "const c = document.getElementById('{CANVAS_ID}');\
                                         if (c && c.__k2g_view) c.__k2g_view('{command}');",
                                    );
                                    if let Err(err) = document::eval(&script).await {
                                        log::debug!("3D view command '{command}' failed: {err}");
                                    }
                                });
                            },
                            span { class: "machining-3d-control-key", "{key}" }
                            "{label}"
                        }
                    }
                }

                // Over the canvas rather than instead of it: the rest of the step — the
                // drilling, the outline — is already drawn and worth looking at while the
                // engraving is worked out. This says a piece is still coming, not that
                // there is nothing there.
                if let Some(spec) = working_on.as_ref() {
                    div { class: "machining-3d-busy",
                        div { class: "machining-3d-spinner" }
                        div { class: "machining-3d-busy-title", "Working out the isolation contours" }
                        div { class: "machining-3d-busy-detail",
                            {
                                let face = if spec.layer_id == pcb::BACK_COPPER { "bottom" } else { "top" };
                                let width = spec.width_nm as f64 / 1e6;
                                format!(
                                    "Reading the {face} copper and re-pouring its zones, for a {width:.2} mm channel.",
                                )
                            }
                        }
                    }
                }
            }

            if !legend.is_empty() {
                aside { class: "machining-3d-legend",
                    div { class: "machining-3d-legend-title", "Tools" }
                    // Keyed by **position**, with the tool id only along for stability.
                    //
                    // A legend row is a tool *block*, not a tool, and two blocks can run
                    // the same tool: the route phase groups by tool within itself, but a
                    // router that mills a slot and a router that cuts an opening arrive as
                    // separate blocks, and they may be the same cutter. Keyed on the tool
                    // id alone that is two siblings with one key, which Dioxus does not
                    // tolerate — it panics the whole application out of the event loop
                    // mid-render (`keyed siblings must each have a unique key`), which is
                    // how this was found: four blocks, three tools, and the window gone.
                    //
                    // The rows stay one-per-block rather than being merged, because each
                    // one names a colour that is on the canvas — and the colour is picked
                    // by block order, so a tool used twice genuinely draws in two. The
                    // checkbox is still per tool: `hidden` is keyed by id, so switching one
                    // of a tool's rows off switches all of that tool's work off, which is
                    // what "hide this tool" should mean.
                    for (index , row) in legend.iter().enumerate() {
                        label {
                            key: "{index}-{row.tool_id}",
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
                                                // Through to the session store as well as
                                                // the signal: the signal is what redraws
                                                // now, the store is what the next mount of
                                                // this view starts from.
                                                if let Ok(mut kept) = hidden_tools().lock() {
                                                    kept.clone_from(off);
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
mod reactivity_tests {
    /// Everything this view draws from must reach it as a **signal**, and be read inside
    /// the memo that builds the scene.
    ///
    /// [`use_memo`] registers its closure once and re-runs it when something the closure
    /// *reads* changes. A prop that arrives as a plain value is therefore captured at first
    /// render and frozen: the memo goes on using whatever it was handed at mount, forever,
    /// and the canvas silently stops following it. Nothing about that is visible in a
    /// review — the code reads perfectly — and nothing about it is visible at runtime
    /// either, because the view keeps drawing a correct picture of a stale input.
    ///
    /// It has happened twice here. Once when the payload was derived in the component body
    /// and captured by the `use_effect`, which pinned the canvas to the plan as it stood on
    /// the first render. Once when `picked` was a plain `Option<OpRef>`, which meant every
    /// click on the op list changed the row's colour and nothing else. Both were found by
    /// someone using the application, not by a test — so this is the test.
    ///
    /// Read as source, because there is nothing else to read it as: the fault is in *how*
    /// the closure was written, and a virtual DOM would have to be driven through a real
    /// render cycle to catch what a regex catches here for nothing.
    #[test]
    fn every_scene_input_is_a_signal_read_inside_the_memo() {
        let source = include_str!("machining_3d.rs");

        let props = source
            .split_once("pub fn Machining3dView(")
            .expect("the component is declared")
            .1
            .split_once(") -> Element")
            .expect("its parameter list ends")
            .0;
        for prop in ["state", "picked"] {
            let declared = props
                .lines()
                .find_map(|line| line.trim().strip_prefix(&format!("{prop}: ")))
                .unwrap_or_else(|| panic!("`{prop}` is a prop of Machining3dView"));
            assert!(
                declared.starts_with("Signal<"),
                "`{prop}` must reach this view as a Signal, or the scene memo freezes it at \
                 the value it had on the first render — it was declared `{declared}`",
            );
        }

        // To the end of the statement, not to the first `)` — the arguments contain calls
        // of their own, which is rather the point of the assertions below.
        let build = source
            .split_once("ScenePayload::build(")
            .expect("the memo builds the payload")
            .1
            .split_once("\n    });")
            .expect("the memo closure ends")
            .0;
        assert!(
            build.contains("state") || build.contains("snapshot"),
            "the payload is built from the context: {build}",
        );
        for read in ["hidden.read()", "picked.read()"] {
            assert!(
                build.contains(read),
                "`{read}` must happen inside the memo — reading it in the component body \
                 subscribes nothing, and the canvas stops following it: {build}",
            );
        }
    }
}

#[cfg(test)]
mod legend_key_tests {
    /// **A legend row is a block, not a tool, and two blocks can run the same tool.**
    ///
    /// Keyed on the tool id alone that is two siblings with one key. Dioxus asserts on it
    /// mid-render and takes the whole application out of the event loop — no dialog, no
    /// log line, the window simply gone. It was reached by toggling a machining operation
    /// on a job whose step then had four blocks across three tools.
    ///
    /// A source test because the fault is in the *key expression* and there is nothing else
    /// to inspect: the panic lives in `dioxus-core`'s differ, which a unit test cannot
    /// reach without standing up a virtual DOM and driving a real render through a
    /// reconciliation that changes the list.
    #[test]
    fn the_legend_key_cannot_repeat_when_two_blocks_share_a_tool() {
        let source = include_str!("machining_3d.rs");
        let loop_ = source
            .split_once("for (index , row) in legend.iter().enumerate() {")
            .map(|(_, rest)| rest)
            .expect("the legend is rendered by enumerating its rows");
        let key = loop_
            .lines()
            .find_map(|line| line.trim().strip_prefix("key: "))
            .expect("each legend row carries a key");

        assert!(
            key.contains("{index}"),
            "the key must include the row's position, or a tool used by two blocks gives \
             two siblings one key and the differ panics: {key}",
        );
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
            z_min_depth: None,
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

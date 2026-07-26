// Bundle entry for k2g's vendored three.js.
//
// Named imports only, so esbuild can tree-shake away the two thirds of three.js
// we never touch (loaders, animation, audio, XR, post-processing).
//
// The list below is deliberately the *whole* set the machining view is planned to
// need, not just what it uses today — rebuilding the bundle is a manual step, so
// it is better to pay a few KB now than to rebuild for each new primitive.
import {
  // Core scene plumbing
  Scene,
  WebGLRenderer,
  OrthographicCamera,
  PerspectiveCamera,
  Group,
  Color,
  Fog,

  // Maths
  Vector2,
  Vector3,
  Box3,
  Matrix4,
  Euler,
  Quaternion,

  // The board: an extruded outline with its cutouts as holes.
  Shape,
  Path,
  ExtrudeGeometry,
  ShapeGeometry,

  // Drilled holes, drawn as instanced cylinders rather than as shape holes —
  // 300 circles in one Shape is a triangulation the earcut pass should not have
  // to do, and instancing draws them in a single call.
  CylinderGeometry,
  InstancedMesh,

  // Toolpaths and helpers
  BufferGeometry,
  BufferAttribute,
  Float32BufferAttribute,
  LineBasicMaterial,
  LineSegments,
  Line,
  Mesh,
  MeshBasicMaterial,
  MeshStandardMaterial,
  MeshLambertMaterial,
  DoubleSide,
  FrontSide,

  // Lighting — enough to read a slab's orientation at a glance.
  AmbientLight,
  DirectionalLight,
  HemisphereLight,

  // Orientation aids
  GridHelper,
  AxesHelper,
  Box3Helper,

  // Picking, for "what tool made this?" later.
  Raycaster,
} from 'three';

import { OrbitControls } from 'three/examples/jsm/controls/OrbitControls.js';

// Fat lines. WebGL's gl.lineWidth() is capped at 1.0 on the ANGLE/D3D path
// WebView2 uses, so a per-tool colour coded toolpath would otherwise be
// hairlines. Line2 expands each segment to a screen-space quad in the shader.
import { Line2 } from 'three/examples/jsm/lines/Line2.js';
import { LineGeometry } from 'three/examples/jsm/lines/LineGeometry.js';
import { LineMaterial } from 'three/examples/jsm/lines/LineMaterial.js';
import { LineSegments2 } from 'three/examples/jsm/lines/LineSegments2.js';
import { LineSegmentsGeometry } from 'three/examples/jsm/lines/LineSegmentsGeometry.js';

// One global, namespaced so it cannot collide with anything Dioxus puts on
// `window`. The app's own view script reads `window.K2G_THREE`.
window.K2G_THREE = {
  Scene, WebGLRenderer, OrthographicCamera, PerspectiveCamera, Group, Color, Fog,
  Vector2, Vector3, Box3, Matrix4, Euler, Quaternion,
  Shape, Path, ExtrudeGeometry, ShapeGeometry,
  CylinderGeometry, InstancedMesh,
  BufferGeometry, BufferAttribute, Float32BufferAttribute,
  LineBasicMaterial, LineSegments, Line,
  Mesh, MeshBasicMaterial, MeshStandardMaterial, MeshLambertMaterial,
  DoubleSide, FrontSide,
  AmbientLight, DirectionalLight, HemisphereLight,
  GridHelper, AxesHelper, Box3Helper,
  Raycaster,
  OrbitControls,
  Line2, LineGeometry, LineMaterial, LineSegments2, LineSegmentsGeometry,
};

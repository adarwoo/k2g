# Vendored third-party browser code

k2g has **no CDN at runtime** — the app is a WebView with every asset compiled into
the binary (`include_str!` / `include_bytes!`). Anything the page needs therefore
lives here, committed, and is frozen until somebody deliberately rebuilds it.

## `three.bundle.js`

The 3D renderer behind the Machining view. Serving it as one self-contained IIFE
rather than as the published package is not a preference: modern three.js is
ESM-only and `Line2`/`OrbitControls` are separate modules under `examples/jsm/`,
so an `import` inside an inline `<script>` would have nothing to resolve against.
Bundling flattens that into one file with no module resolution at all.

`three.entry.js` is the bundle's source of truth: it names, with reasons, exactly
which three.js exports k2g uses, and attaches them to the single global
`window.K2G_THREE`. Add an export there — not to the bundle — and rebuild.

### Rebuilding

Needs Node and npm; nothing is checked in but the output and the entry file.

```sh
mkdir three-build && cd three-build
printf '{ "name": "k2g-three-bundle", "private": true, "type": "module" }' > package.json
npm install three@0.185.1 esbuild
cp ../assets/vendor/three.entry.js entry.js

./node_modules/.bin/esbuild entry.js \
    --bundle --minify --format=iife --target=es2020 --legal-comments=none \
    --outfile=three.bundle.js
```

Then prepend the provenance header from the current `three.bundle.js` (updating the
version and date) and copy it over. `--format=iife` is what makes it loadable from
an inline script; `--target=es2020` is comfortably below what WebView2, WKWebView
and WebKitGTK all support.

Roughly 600 KB minified, 150 KB gzipped. Most of that is `WebGLRenderer`, which
tree-shaking cannot help with — it is the part we came for.

### Pinning

The version above is the one that was tested. Bumping it is a deliberate act:
rebuild, run the app, and check the Machining view actually renders — three.js
makes breaking changes between releases and there is no compiler to catch them,
because this is the one part of k2g that Rust does not type-check.

pub const APP_STYLE: &str = r#"
* {
    box-sizing: border-box;
}

:root {
    --bg: #111318;
    --bg-subtle: #181b22;
    --bg-elev: #202632;
    --bg-hover: #2b3343;
    --text: #e9edf5;
    --text-subtle: #98a4bb;
    --border: #2d3444;
    --accent: #2f7ae5;
    --accent-strong: #1d63c6;
    --ok: #2ca66d;
    --warn: #d7a03f;
    --err: #d05959;
    --shadow: 0 8px 30px rgba(0, 0, 0, 0.35);
}

.theme-light {
    --bg: #f4f7fb;
    --bg-subtle: #ffffff;
    --bg-elev: #ffffff;
    --bg-hover: #e9eef8;
    --text: #111827;
    --text-subtle: #667085;
    --border: #d2d8e4;
    --accent: #2f7ae5;
    --accent-strong: #1d63c6;
    --ok: #1c8d57;
    --warn: #b27611;
    --err: #b93636;
    --shadow: 0 8px 22px rgba(0, 0, 0, 0.1);
}

body {
    margin: 0;
    font-family: "Segoe UI", "Noto Sans", sans-serif;
    color: var(--text);
    background: radial-gradient(circle at 0% 0%, #2a354b 0%, var(--bg) 45%);
}

.app-shell {
    height: 100vh;
    width: 100vw;
    display: flex;
    flex-direction: column;
    background: var(--bg);
    color: var(--text);
    position: relative;
}

.top-bar {
    height: 56px;
    border-bottom: 1px solid var(--border);
    background: var(--bg-subtle);
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 0 14px;
}

.title {
    font-size: 14px;
    font-weight: 700;
    letter-spacing: 0.2px;
}

.divider {
    width: 1px;
    height: 20px;
    background: var(--border);
}

.top-control {
    display: flex;
    align-items: center;
    gap: 8px;
}

.top-control label {
    font-size: 11px;
    color: var(--text-subtle);
    text-transform: uppercase;
    letter-spacing: 0.4px;
}

.top-control select,
.panel select,
.panel input,
.gcode-editor {
    background: var(--bg-elev);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 7px 10px;
    font-size: 12px;
}

.top-control select {
    width: 220px;
}

.spacer {
    flex: 1;
}

.status-line {
    display: flex;
    align-items: center;
    gap: 8px;
}

.status-pill {
    border-radius: 999px;
    padding: 5px 9px;
    font-size: 11px;
    font-weight: 700;
}

.status-ok {
    background: color-mix(in srgb, var(--ok) 20%, transparent);
    color: var(--ok);
}

.status-warn {
    background: color-mix(in srgb, var(--warn) 20%, transparent);
    color: var(--warn);
}

.status-busy {
    background: color-mix(in srgb, var(--accent) 20%, transparent);
    color: var(--accent);
}

.btn {
    border: 1px solid var(--border);
    background: var(--bg-elev);
    color: var(--text);
    border-radius: 8px;
    padding: 8px 10px;
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    transition: all 160ms ease;
}

.btn:hover {
    background: var(--bg-hover);
}

.btn-primary {
    background: var(--accent);
    border-color: var(--accent-strong);
    color: #fff;
}

.btn-primary:hover {
    background: var(--accent-strong);
}

.btn-secondary {
    background: transparent;
}

.btn-danger {
    border-color: color-mix(in srgb, var(--err) 45%, var(--border));
    color: var(--err);
}

.btn-icon {
    padding: 7px 9px;
}

.btn-small {
    padding: 6px 8px;
}

.error-banner {
    border-bottom: 1px solid color-mix(in srgb, var(--warn) 35%, var(--border));
    background: color-mix(in srgb, var(--warn) 15%, transparent);
}

.error-toggle {
    width: 100%;
    text-align: left;
    background: transparent;
    border: none;
    color: var(--text);
    padding: 9px 14px;
    font-size: 12px;
    cursor: pointer;
}

.error-list {
    padding: 0 14px 12px;
    display: flex;
    flex-direction: column;
    gap: 8px;
}

.error-item {
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 8px 10px;
}

.error-item.error {
    border-color: color-mix(in srgb, var(--err) 40%, var(--border));
}

.error-item.warning {
    border-color: color-mix(in srgb, var(--warn) 45%, var(--border));
}

.error-title {
    font-size: 12px;
    font-weight: 700;
}

.error-details {
    margin-top: 4px;
    color: var(--text-subtle);
    font-size: 11px;
}

.work-area {
    flex: 1;
    display: flex;
    min-height: 0;
}

.left-nav {
    width: 190px;
    border-right: 1px solid var(--border);
    background: var(--bg-subtle);
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 10px;
}

.nav-item {
    text-align: left;
    border: 1px solid transparent;
    background: transparent;
    color: var(--text);
    border-radius: 8px;
    padding: 10px;
    font-size: 13px;
    cursor: pointer;
}

.nav-item:hover {
    background: var(--bg-hover);
}

.nav-item.active {
    background: var(--accent);
    color: #fff;
}

.main-content {
    flex: 1;
    min-width: 0;
    min-height: 0;
    overflow: auto;
}

.screen {
    height: 100%;
    padding: 14px;
}

.screen.single {
    display: flex;
    flex-direction: column;
    gap: 12px;
}

.screen.split {
    display: grid;
    grid-template-columns: 1fr 340px;
    gap: 12px;
}

.screen.single.centered {
    display: grid;
    place-items: center;
}

.job-layout {
    height: 100%;
    display: grid;
    grid-template-columns: 1fr 340px;
    gap: 12px;
}

.job-main {
    display: flex;
    flex-direction: column;
    gap: 10px;
    min-height: 0;
}

.job-view-tabs {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
}

.job-view-tab {
    border: 1px solid var(--border);
    background: var(--bg-elev);
    color: var(--text);
    border-radius: 999px;
    padding: 7px 11px;
    font-size: 12px;
    font-weight: 700;
    cursor: pointer;
}

.job-view-tab.active {
    border-color: color-mix(in srgb, var(--accent) 65%, var(--border));
    background: color-mix(in srgb, var(--accent) 20%, transparent);
    color: var(--text);
}

.machining-summary {
    display: grid;
    grid-template-columns: repeat(2, minmax(220px, 1fr));
    gap: 10px;
}

.panel {
    border: 1px solid var(--border);
    border-radius: 12px;
    background: var(--bg-subtle);
    box-shadow: var(--shadow);
    padding: 12px;
    min-height: 0;
}

.panel.fixed {
    overflow: auto;
}

.panel.grow {
    overflow: auto;
}

.panel-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
}

.panel-header .actions {
    display: flex;
    align-items: center;
    gap: 8px;
}

h3 {
    margin: 0;
    font-size: 14px;
}

h4 {
    margin: 0;
    font-size: 13px;
}

p {
    margin: 0;
    font-size: 12px;
    color: var(--text-subtle);
}

.field {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin-top: 12px;
}

.field label {
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.4px;
    color: var(--text-subtle);
    font-weight: 700;
}

.job-layout .panel.fixed .field > label {
    text-transform: none;
    letter-spacing: 0;
}

.inline-field {
    display: grid;
    grid-template-columns: 1fr 120px;
    gap: 8px;
    align-items: center;
}

.radio-group {
    display: flex;
    gap: 12px;
    margin-top: 8px;
}

.radio-group.vertical {
    flex-direction: column;
    gap: 8px;
}

.radio-option {
    display: flex;
    flex-direction: column;
    gap: 6px;
}

.radio-option label {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
    font-weight: 400;
    text-transform: none;
    letter-spacing: normal;
    color: var(--text);
    cursor: pointer;
    margin: 0;
}

.radio-option input[type="radio"] {
    margin: 0;
    cursor: pointer;
}

.radio-option select {
    margin-left: 4px;
}

.radio-option select:disabled {
    opacity: 0.5;
    cursor: not-allowed;
    background: color-mix(in srgb, var(--bg-elev) 70%, transparent);
}

.sub-field {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-left: 24px;
    margin-top: 4px;
    font-size: 12px;
}

.section-subfield {
    margin-top: 12px;
    padding-top: 4px;
}

.nested-radio-group {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-left: 24px;
    margin-top: 6px;
    padding: 8px 0;
}

.nested-radio-group label {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
    font-weight: 400;
    text-transform: none;
    letter-spacing: normal;
    color: var(--text);
    cursor: pointer;
    margin: 0;
}

.nested-radio-group input[type="radio"] {
    margin: 0;
    cursor: pointer;
}

.custom-angle-input {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-left: 28px;
    margin-top: 4px;
    font-size: 12px;
}

.custom-angle-input input[type="number"] {
    width: 80px;
    padding: 4px 6px;
}

.btn-op {
    text-align: left;
    border: 1px solid var(--border);
    background: var(--bg-elev);
    color: var(--text);
    border-radius: 8px;
    padding: 8px;
    font-size: 12px;
    cursor: pointer;
}

.btn-op.active {
    background: color-mix(in srgb, var(--accent) 18%, transparent);
    border-color: color-mix(in srgb, var(--accent) 45%, var(--border));
}

.checkbox-line {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
}

.card-grid {
    margin-top: 12px;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(230px, 1fr));
    gap: 10px;
}

.machine-card {
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 10px;
    display: flex;
    flex-direction: column;
    gap: 8px;
}

.machine-card.active {
    border-color: color-mix(in srgb, var(--accent) 55%, var(--border));
    background: color-mix(in srgb, var(--accent) 10%, transparent);
}

.empty-state {
    margin-top: 16px;
    border: 1px dashed var(--border);
    border-radius: 10px;
    padding: 18px;
    display: flex;
    flex-direction: column;
    gap: 6px;
    align-items: center;
}

.table-wrap {
    border: 1px solid var(--border);
    border-radius: 10px;
    overflow: auto;
}

table {
    width: 100%;
    border-collapse: collapse;
    font-size: 12px;
}

th,
td {
    text-align: left;
    padding: 9px;
    border-bottom: 1px solid var(--border);
}

th {
    font-size: 11px;
    text-transform: uppercase;
    color: var(--text-subtle);
    letter-spacing: 0.35px;
}

.th-sort-btn {
    border: none;
    background: transparent;
    padding: 0;
    margin: 0;
    color: inherit;
    font: inherit;
    letter-spacing: inherit;
    text-transform: inherit;
    cursor: pointer;
}

.th-sort-btn:hover {
    color: var(--text);
}

.stock-col-description {
    width: 12%;
    max-width: 12%;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
}

.stock-actions {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: nowrap;
}

.stock-actions-cell {
    min-width: 190px;
    width: 190px;
    white-space: nowrap;
}

.stock-actions .btn {
    min-width: 54px;
}

.stock-actions .btn-secondary {
    background: var(--bg-elev);
}

.edit-grid {
    display: grid;
    grid-template-columns: repeat(2, minmax(220px, 1fr));
    gap: 10px;
    overflow: auto;
}

.full-width {
    grid-column: 1 / -1;
}

.status-chip {
    border-radius: 999px;
    padding: 3px 8px;
    font-size: 11px;
    font-weight: 700;
}

.status-in-stock {
    color: var(--ok);
    background: color-mix(in srgb, var(--ok) 20%, transparent);
}

.status-in-rack {
    color: var(--accent);
    background: color-mix(in srgb, var(--accent) 20%, transparent);
}

.status-out-of-stock {
    color: var(--err);
    background: color-mix(in srgb, var(--err) 20%, transparent);
}

.status-new,
.status-not-preferred {
    color: var(--text-subtle);
    background: color-mix(in srgb, var(--text-subtle) 18%, transparent);
}

.board-preview {
    display: grid;
    place-items: center;
    text-align: center;
    gap: 10px;
}

.board-view-controls {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    align-items: center;
    gap: 6px;
}

.board-view-status {
    margin-left: 8px;
    color: var(--text-subtle);
    font-size: 12px;
}

.board-canvas {
    width: min(760px, 95%);
    aspect-ratio: 1 / 1;
    border-radius: 12px;
    border: 1px solid var(--border);
    background: color-mix(in srgb, var(--bg-elev) 82%, transparent);
    overflow: hidden;
    cursor: grab;
}

.board-canvas.is-panning {
    cursor: grabbing;
}

.board-svg {
    width: 100%;
    height: 100%;
    display: block;
}

.board-svg-frame {
    fill: color-mix(in srgb, var(--bg) 78%, transparent);
    stroke: color-mix(in srgb, var(--border) 88%, transparent);
    stroke-width: 1;
}

.board-hole-cross {
    stroke: var(--accent);
    stroke-width: 2;
    stroke-linecap: round;
    opacity: 0.95;
}

.board-hole-via {
    stroke: color-mix(in srgb, var(--accent) 85%, white);
}

.board-hole-pth {
    stroke: color-mix(in srgb, var(--ok) 78%, var(--text));
}

.board-hole-npth {
    stroke: color-mix(in srgb, var(--warn) 85%, var(--text));
}

.board-hole-other {
    stroke: color-mix(in srgb, var(--text-subtle) 70%, var(--text));
}

.board-hole-outline {
    fill: none;
    stroke-width: 2;
    opacity: 0.95;
    stroke-linecap: round;
    stroke-linejoin: round;
}

.board-hole-pth-box {
    stroke: color-mix(in srgb, var(--ok) 78%, var(--text));
}

.board-hole-npth-halfbox {
    stroke: color-mix(in srgb, var(--warn) 85%, var(--text));
}

.board-legend {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: 10px 14px;
    padding: 6px 8px;
    color: var(--text-subtle);
    font-size: 12px;
}

.board-legend-item {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 4px 8px;
    border: 1px solid color-mix(in srgb, var(--border) 80%, transparent);
    border-radius: 999px;
    background: color-mix(in srgb, var(--bg-elev) 75%, transparent);
}

.board-legend-icon {
    width: 16px;
    height: 16px;
    display: block;
}

.preview-box,
.canvas-mock {
    width: min(520px, 90%);
    aspect-ratio: 4 / 3;
    border-radius: 12px;
    border: 2px dashed var(--border);
    background: repeating-linear-gradient(
        -45deg,
        color-mix(in srgb, var(--bg-elev) 85%, transparent),
        color-mix(in srgb, var(--bg-elev) 85%, transparent) 14px,
        color-mix(in srgb, var(--bg-hover) 75%, transparent) 14px,
        color-mix(in srgb, var(--bg-hover) 75%, transparent) 28px
    );
    display: grid;
    place-items: center;
    font-size: 14px;
    color: var(--text-subtle);
}

.gcode-editor {
    width: 100%;
    min-height: 280px;
    flex: 1;
    font-family: "Cascadia Code", "Consolas", monospace;
    font-size: 12px;
    resize: none;
}

.cnc-template-editor {
    min-height: 0;
    flex: initial;
    resize: vertical;
}

.modified-banner {
    border: 1px solid color-mix(in srgb, var(--warn) 45%, var(--border));
    background: color-mix(in srgb, var(--warn) 15%, transparent);
    border-radius: 8px;
    padding: 8px 10px;
    font-size: 12px;
}

.program-stats {
    display: flex;
    gap: 18px;
    font-size: 11px;
    color: var(--text-subtle);
}

.rack-grid {
    margin-top: 10px;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
    gap: 10px;
}

.rack-slot {
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 10px;
    display: flex;
    flex-direction: column;
    gap: 8px;
}

.rack-slot.assigned {
    border-color: color-mix(in srgb, var(--accent) 50%, var(--border));
    background: color-mix(in srgb, var(--accent) 12%, transparent);
}

.rack-slot.disabled {
    opacity: 0.65;
}

.rack-slot-title {
    font-size: 12px;
    font-weight: 700;
}

.impact-list {
    margin-top: 10px;
    display: flex;
    flex-direction: column;
    gap: 8px;
}

.impact-item {
    border-radius: 8px;
    border: 1px solid var(--border);
    padding: 9px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
}

.impact-item.ok {
    border-color: color-mix(in srgb, var(--ok) 45%, var(--border));
}

.impact-item.missing {
    border-color: color-mix(in srgb, var(--err) 45%, var(--border));
}

.impact-name {
    font-size: 12px;
    font-weight: 600;
}

.impact-state {
    font-size: 11px;
    color: var(--text-subtle);
}

.footer-line {
    border-top: 1px solid var(--border);
    background: var(--bg-subtle);
    padding: 8px 14px;
    display: flex;
    align-items: center;
    gap: 16px;
    font-size: 11px;
}

.kicad-ok {
    color: var(--ok);
    font-weight: 700;
}

.kicad-err {
    color: var(--err);
    font-weight: 700;
}

.env-summary {
    color: var(--text-subtle);
}

.empty-hint {
    position: absolute;
    right: 14px;
    bottom: 44px;
    background: var(--bg-elev);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 8px 10px;
    font-size: 11px;
    color: var(--text-subtle);
}

.wizard-overlay {
    position: absolute;
    inset: 0;
    background: rgba(0, 0, 0, 0.45);
    z-index: 99;
    display: grid;
    place-items: center;
}

.wizard-dialog {
    width: min(520px, 90vw);
    background: var(--bg-subtle);
    border: 1px solid var(--border);
    border-radius: 14px;
    box-shadow: var(--shadow);
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 12px;
}

.wizard-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
}

.catalog-picker-dialog {
    width: min(820px, 92vw);
    max-height: 82vh;
    overflow: hidden;
    background: var(--bg-subtle);
    border: 1px solid var(--border);
    border-radius: 14px;
    box-shadow: var(--shadow);
    padding: 14px;
    display: flex;
    flex-direction: column;
    gap: 12px;
}

.catalog-picker-list {
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--bg-elev);
    overflow: auto;
    padding: 8px;
    display: flex;
    flex-direction: column;
    gap: 8px;
}

.catalog-node {
    border: 1px solid var(--border);
    border-radius: 8px;
    background: color-mix(in srgb, var(--bg-subtle) 70%, transparent);
}

.section-node {
    margin: 8px;
}

.catalog-node-summary {
    cursor: pointer;
    padding: 8px 10px;
    font-size: 12px;
    font-weight: 700;
    user-select: none;
}

.catalog-tool-list {
    border-top: 1px solid var(--border);
    padding: 4px 8px;
    display: flex;
    flex-direction: column;
    gap: 0;
}

.catalog-tool-header {
    display: grid;
    grid-template-columns: auto minmax(220px, 1fr) 92px 180px;
    gap: 6px;
    align-items: center;
    padding: 4px 4px 6px;
    border-bottom: 1px solid var(--border);
    margin-bottom: 2px;
}

.catalog-tool-col-label,
.catalog-tool-col-type,
.catalog-tool-col-diameter {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.35px;
    color: var(--text-subtle);
    font-weight: 700;
}

.catalog-tool-col-label {
    grid-column: 2;
}

.catalog-tool-col-type {
    grid-column: 3;
}

.catalog-tool-col-diameter {
    grid-column: 4;
}

.catalog-tool-row {
    display: grid;
    grid-template-columns: auto minmax(220px, 1fr) 92px 180px;
    gap: 6px;
    align-items: center;
    padding: 2px 4px;
    border-radius: 4px;
}

.catalog-tool-row:hover {
    background: color-mix(in srgb, var(--accent) 8%, transparent);
}

.catalog-tool-row input {
    width: 13px;
    height: 13px;
    margin: 0;
}

.catalog-tool-main {
    font-size: 12px;
}

.catalog-tool-label {
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
}

.catalog-tool-type,
.catalog-tool-diameter {
    font-size: 11px;
    color: var(--text-subtle);
    white-space: nowrap;
}

.size-cell {
    display: flex;
    flex-direction: column;
    gap: 1px;
    line-height: 1.3;
}

.size-alt {
    font-size: 10px;
    color: var(--text-subtle);
}

.section-controls {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 3px 10px;
    border-bottom: 1px solid var(--border);
    background: color-mix(in srgb, var(--bg-subtle) 60%, transparent);
}

.section-controls-sep {
    color: var(--text-subtle);
    font-size: 11px;
}

.btn-link {
    background: none;
    border: none;
    color: var(--accent);
    cursor: pointer;
    font-size: 11px;
    padding: 0;
    text-decoration: underline;
}

.diagnostics {
    margin-top: 14px;
    border-top: 1px solid var(--border);
    padding-top: 12px;
    display: flex;
    flex-direction: column;
    gap: 8px;
}

.diag-status {
    font-size: 11px;
}

details {
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 8px;
    background: var(--bg-elev);
}

summary {
    cursor: pointer;
    font-size: 12px;
    font-weight: 700;
}

.env-table {
    margin-top: 8px;
    max-height: 220px;
    overflow: auto;
}

@media (max-width: 1024px) {
    .screen.split {
        grid-template-columns: 1fr;
    }

    .job-layout {
        grid-template-columns: 1fr;
    }

    .left-nav {
        width: 150px;
    }

    .top-control select {
        width: 160px;
    }
}

@media (max-width: 760px) {
    .work-area {
        flex-direction: column;
    }

    .left-nav {
        width: 100%;
        flex-direction: row;
        overflow: auto;
        border-right: none;
        border-bottom: 1px solid var(--border);
    }

    .nav-item {
        white-space: nowrap;
    }

    .top-bar {
        flex-wrap: wrap;
        height: auto;
        padding: 10px;
    }

    .footer-line {
        flex-direction: column;
        align-items: flex-start;
        gap: 5px;
    }
}
"#;

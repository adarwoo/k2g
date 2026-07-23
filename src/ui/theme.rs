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

.shell-theme-dark {
    --bg: #0c0e12;
    --bg-subtle: #0e1016;
    --bg-elev: #10131a;
    --bg-hover: #171b24;
    --text: #cdd2de;
    --text-subtle: #8a94a8;
    --border: rgba(255, 255, 255, 0.07);
    --accent: #f59e0b;
    --accent-strong: #d97706;
    --ok: #4ade80;
    --warn: #fbbf24;
    --err: #f87171;
}

.shell-theme-light {
    --bg: #f5f7fb;
    --bg-subtle: #ffffff;
    --bg-elev: #ffffff;
    --bg-hover: #eef2f8;
    --text: #1f2937;
    --text-subtle: #667085;
    --border: rgba(15, 23, 42, 0.12);
    --accent: #c97a09;
    --accent-strong: #a86004;
    --ok: #15803d;
    --warn: #b45309;
    --err: #b91c1c;
    --shadow: 0 12px 34px rgba(15, 23, 42, 0.08);
}

html,
body {
    margin: 0;
    width: 100%;
    height: 100%;
    overflow: hidden;
    font-family: "Segoe UI", "Noto Sans", sans-serif;
    color: var(--text);
    background: radial-gradient(circle at 0% 0%, #232933 0%, var(--bg) 45%);
}

#main {
    width: 100%;
    height: 100%;
    min-height: 100%;
}

.app-shell {
    height: 100vh;
    width: 100vw;
    max-height: 100vh;
    max-width: 100vw;
    display: flex;
    flex-direction: column;
    background: var(--bg);
    color: var(--text);
    position: relative;
    overflow: hidden;
}

.shell-topbar {
    min-height: 58px;
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 0 16px;
    border-bottom: 1px solid var(--border);
    background: var(--bg-subtle);
}

.brand-block {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: max-content;
}

.brand-mark-image {
    width: 28px;
    height: 28px;
    border-radius: 8px;
    object-fit: cover;
    display: block;
    border: 1px solid color-mix(in srgb, var(--border) 75%, transparent);
}

.brand-copy {
    display: flex;
    flex-direction: column;
    gap: 2px;
}

.brand-title {
    font-size: 14px;
    line-height: 1;
    font-weight: 700;
    letter-spacing: 0.02em;
}

.brand-subtitle {
    font-size: 10px;
    line-height: 1;
    color: var(--text-subtle);
    text-transform: uppercase;
    letter-spacing: 0.12em;
}

.topbar-board {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 220px;
}

/* Name-or-picker and the refresh glyph, side by side, under the "Board" label. */
.topbar-board-row {
    display: flex;
    align-items: center;
    gap: 6px;
}

.topbar-label,
.summary-chip-label,
.diag-banner-subtitle,
.status-meta,
.status-summary {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.12em;
    color: var(--text-subtle);
}

.topbar-value {
    font-size: 12px;
    color: var(--text);
}

.topbar-value-missing {
    color: var(--err);
}

.mono {
    font-family: "Consolas", "Cascadia Mono", monospace;
}

.topbar-chip-row {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
}

.summary-chip {
    display: flex;
    flex-direction: column;
    gap: 3px;
    min-width: 88px;
    padding: 8px 10px;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: color-mix(in srgb, var(--bg-elev) 88%, transparent);
}

.summary-chip-value {
    font-size: 12px;
    color: var(--text);
}

.unit-toggle {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 4px;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: color-mix(in srgb, var(--bg-elev) 88%, transparent);
}

.unit-toggle-btn {
    border: 1px solid transparent;
    border-radius: 8px;
    background: transparent;
    color: var(--text-subtle);
    padding: 6px 10px;
    font-size: 11px;
    font-weight: 700;
    letter-spacing: 0.02em;
    cursor: pointer;
}

.unit-toggle-btn:hover {
    color: var(--text);
    background: color-mix(in srgb, var(--bg-hover) 85%, transparent);
}

.unit-toggle-btn.active {
    color: var(--accent);
    border-color: color-mix(in srgb, var(--accent) 40%, var(--border));
    background: color-mix(in srgb, var(--accent) 16%, transparent);
}

.shell-spacer {
    flex: 1;
}

.topbar-status-group {
    display: flex;
    align-items: center;
    gap: 10px;
}

.icon-button,
.text-button {
    border: 1px solid var(--border);
    background: var(--bg-elev);
    color: var(--text);
    border-radius: 9px;
    padding: 8px 12px;
    font-size: 12px;
    cursor: pointer;
    transition: background 160ms ease, border-color 160ms ease, color 160ms ease;
}

.icon-button:hover,
.text-button:hover {
    background: var(--bg-hover);
    border-color: color-mix(in srgb, var(--accent) 30%, var(--border));
}

.shell-body {
    flex: 1;
    min-height: 0;
    display: flex;
    overflow: hidden;
}

.shell-rail {
    width: 132px;
    padding: 12px 10px;
    border-right: 1px solid var(--border);
    background: var(--bg-subtle);
    display: flex;
    flex-direction: column;
    gap: 8px;
}

.rail-button {
    border: 1px solid transparent;
    border-radius: 12px;
    background: transparent;
    color: var(--text-subtle);
    padding: 12px 8px;
    text-align: left;
    cursor: pointer;
    transition: background 160ms ease, color 160ms ease, border-color 160ms ease;
}

.rail-button-content {
    display: flex;
    align-items: center;
    gap: 8px;
}

.rail-button-icon {
    width: 35px;
    height: 35px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: 11px;
    background: color-mix(in srgb, var(--bg-elev) 80%, transparent);
    border: 1px solid color-mix(in srgb, var(--border) 85%, transparent);
    flex: 0 0 auto;
}

.rail-icon-svg {
    width: 24px;
    height: 24px;
    fill: none;
    stroke: currentColor;
    stroke-width: 1.7;
    stroke-linecap: round;
    stroke-linejoin: round;
}

.rail-button:hover {
    background: color-mix(in srgb, var(--bg-hover) 90%, transparent);
    color: var(--text);
}

.rail-button.active {
    background: color-mix(in srgb, var(--accent) 14%, var(--bg-elev));
    border-color: color-mix(in srgb, var(--accent) 35%, var(--border));
    color: var(--accent);
}

.rail-button.active .rail-button-icon {
    background: color-mix(in srgb, var(--accent) 22%, transparent);
    border-color: color-mix(in srgb, var(--accent) 45%, var(--border));
}

.rail-button-text {
    display: block;
    font-size: 10px;
    line-height: 1.25;
    font-weight: 700;
    text-transform: none;
    letter-spacing: 0.04em;
}

.rail-separator {
    height: 1px;
    background: color-mix(in srgb, var(--border) 85%, transparent);
    margin: 2px 4px;
}

.shell-content {
    flex: 1;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
    display: flex;
    flex-direction: column;
    background:
        radial-gradient(circle at top left, rgba(245, 158, 11, 0.08), transparent 22%),
        linear-gradient(180deg, color-mix(in srgb, var(--bg) 92%, #000 8%) 0%, var(--bg) 100%);
}

.screen-host {
    flex: 1;
    min-height: 0;
    overflow: auto;
}

.diag-banner-wrap {
    border-bottom: 1px solid var(--border);
    background: color-mix(in srgb, var(--bg-subtle) 95%, transparent);
}

.diag-banner {
    min-height: 42px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 0 16px;
}

.diag-banner-main {
    display: flex;
    align-items: center;
    gap: 10px;
}

.diag-banner-dot {
    width: 9px;
    height: 9px;
    border-radius: 999px;
    background: currentColor;
    box-shadow: 0 0 0 5px color-mix(in srgb, currentColor 16%, transparent);
}

.diag-banner-error {
    color: var(--err);
    background: color-mix(in srgb, var(--err) 8%, transparent);
}

.diag-banner-warning {
    color: var(--warn);
    background: color-mix(in srgb, var(--warn) 8%, transparent);
}

.diag-banner-copy {
    display: flex;
    flex-direction: column;
    gap: 2px;
}

.diag-banner-title,
.diag-detail-title {
    font-size: 12px;
    font-weight: 700;
}

.diag-detail-list {
    padding: 0 16px 16px;
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
    gap: 12px;
}

.diag-detail-card {
    padding: 12px;
    border-radius: 12px;
    border: 1px solid var(--border);
    background: var(--bg-elev);
}

.diag-detail-card.is-error {
    border-color: color-mix(in srgb, var(--err) 40%, var(--border));
}

.diag-detail-card.is-warning {
    border-color: color-mix(in srgb, var(--warn) 40%, var(--border));
}

.diag-detail-text {
    margin-top: 6px;
    font-size: 11px;
    line-height: 1.5;
    color: var(--text-subtle);
}

.event-toast-stack {
    position: fixed;
    right: 16px;
    bottom: 40px;
    z-index: 1000;
    display: flex;
    flex-direction: column;
    gap: 8px;
    max-width: 420px;
    pointer-events: none;
}

.event-toast {
    padding: 10px 12px;
    border-radius: 10px;
    border: 1px solid color-mix(in srgb, var(--accent) 32%, var(--border));
    background: color-mix(in srgb, var(--bg-elev) 92%, black 8%);
    color: var(--text);
    font-size: 12px;
    box-shadow: var(--shadow);
    animation: toast-lifetime 4s ease forwards;
}

@keyframes toast-lifetime {
    0% {
        opacity: 0;
        transform: translateY(8px);
    }
    10% {
        opacity: 1;
        transform: translateY(0);
    }
    82% {
        opacity: 1;
        transform: translateY(0);
    }
    100% {
        opacity: 0;
        transform: translateY(8px);
    }
}

.shell-statusbar {
    height: 30px;
    min-height: 30px;
    flex: 0 0 30px;
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 0 16px;
    border-top: 1px solid var(--border);
    background: color-mix(in srgb, var(--bg-subtle) 96%, black 4%);
    overflow: hidden;
}

.status-connection {
    font-size: 11px;
    white-space: nowrap;
}

.status-connection.ok {
    color: var(--ok);
}

.status-connection.err {
    color: var(--err);
}

.status-summary {
    margin-left: auto;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
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

.status-err {
    background: color-mix(in srgb, var(--err) 20%, transparent);
    color: var(--err);
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

.setup-shell {
    display: flex;
    gap: 0;
    padding: 0;
}

.setup-sidebar {
    width: 240px;
    border-right: 1px solid var(--border);
    background: color-mix(in srgb, var(--bg-subtle) 92%, transparent);
    display: flex;
    flex-direction: column;
}

.setup-sidebar-header {
    padding: 18px 16px 14px;
    border-bottom: 1px solid var(--border);
}

.setup-eyebrow {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.14em;
    color: var(--text-subtle);
}

.setup-sidebar-title {
    margin-top: 6px;
    font-size: 15px;
    font-weight: 700;
}

.setup-sidebar-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 12px;
}

.setup-sidebar-button {
    text-align: left;
    border: 1px solid transparent;
    border-radius: 12px;
    background: transparent;
    color: var(--text-subtle);
    padding: 12px;
    cursor: pointer;
    transition: background 160ms ease, border-color 160ms ease, color 160ms ease;
}

.setup-sidebar-button:hover {
    background: var(--bg-hover);
    color: var(--text);
}

.setup-sidebar-button.active {
    background: color-mix(in srgb, var(--accent) 12%, var(--bg-elev));
    border-color: color-mix(in srgb, var(--accent) 35%, var(--border));
    color: var(--text);
}

.setup-sidebar-button-title {
    display: block;
    font-size: 12px;
    font-weight: 700;
}

.setup-sidebar-button-caption {
    display: block;
    margin-top: 4px;
    font-size: 10px;
    color: var(--text-subtle);
}

.setup-main {
    flex: 1;
    min-width: 0;
    min-height: 0;
    overflow: auto;
}

.setup-stage {
    padding: 22px;
    display: flex;
    flex-direction: column;
    gap: 16px;
}

.setup-stage-header h2 {
    margin: 0;
    font-size: 20px;
}

.setup-stage-header p {
    margin: 8px 0 0;
    color: var(--text-subtle);
    font-size: 13px;
}

.setup-card-grid {
    display: grid;
    gap: 16px;
}

.setup-card-grid.two-up {
    grid-template-columns: repeat(auto-fit, minmax(260px, 1fr));
}

.setup-card {
    border: 1px solid var(--border);
    border-radius: 16px;
    background: color-mix(in srgb, var(--bg-elev) 92%, transparent);
    padding: 16px;
    box-shadow: var(--shadow);
}

.setup-card h3 {
    margin: 0 0 12px;
}

.setup-card p {
    color: var(--text-subtle);
    font-size: 12px;
}

.setup-card-list {
    display: flex;
    flex-direction: column;
    gap: 12px;
}

.profile-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
}

.profile-list-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 12px;
    border: 1px solid var(--border);
    border-radius: 12px;
    background: color-mix(in srgb, var(--bg) 86%, transparent);
}

.profile-list-item.active {
    border-color: color-mix(in srgb, var(--accent) 35%, var(--border));
    background: color-mix(in srgb, var(--accent) 9%, var(--bg-elev));
}

.profile-list-title {
    font-size: 13px;
    font-weight: 700;
}

.profile-list-meta {
    margin-top: 4px;
    font-size: 11px;
    color: var(--text-subtle);
}

.cnc-manager-shell {
    display: flex;
    flex-direction: column;
    gap: 12px;
}

.cnc-manager-grid {
    display: grid;
    grid-template-columns: minmax(280px, 340px) minmax(420px, 1fr);
    gap: 12px;
}

.cnc-profile-list-panel,
.cnc-profile-details-panel {
    display: flex;
    flex-direction: column;
    gap: 10px;
}

.cnc-profile-list-panel .profile-list {
    max-height: 360px;
    overflow: auto;
}

.profile-list-item.built-in {
    border-color: color-mix(in srgb, var(--border) 86%, var(--text-subtle));
}

.profile-list-item.editable {
    border-color: color-mix(in srgb, var(--accent) 20%, var(--border));
}

.profile-list-item.built-in.active {
    background: color-mix(in srgb, var(--bg-elev) 88%, var(--accent));
}

.profile-list-item.editable.active {
    background: color-mix(in srgb, var(--accent) 12%, var(--bg-elev));
}

.stock-shell {
    padding: 18px;
    gap: 16px;
}

.stock-toolbar {
    display: flex;
    align-items: end;
    justify-content: space-between;
    gap: 16px;
    flex-wrap: wrap;
}

.stock-toolbar h3 {
    margin: 0;
    font-size: 18px;
}

.stock-toolbar-actions {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
}

.stock-filter-input {
    min-width: 280px;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--bg-elev);
    color: var(--text);
    padding: 9px 12px;
    font-size: 12px;
}

.stock-toolbar-select {
    min-width: 140px;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--bg-elev);
    color: var(--text);
    padding: 9px 12px;
    font-size: 12px;
}

.stock-table-wrap {
    border: 1px solid var(--border);
    border-radius: 16px;
    background: color-mix(in srgb, var(--bg-elev) 92%, transparent);
    overflow: auto;
}

.stock-row {
    cursor: default;
}

.stock-row.selected td {
    background: color-mix(in srgb, var(--accent) 10%, var(--bg-elev));
}

.stock-name-cell {
    font-weight: 600;
}

.stock-detail-page {
    display: flex;
    flex-direction: column;
    min-height: 0;
}

.stock-detail-panel {
    display: flex;
    flex-direction: column;
    gap: 16px;
    min-height: 0;
    overflow: hidden;
}

.stock-detail-form {
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-height: 0;
    overflow-y: auto;
    padding-right: 6px;
}

.stock-field-popup {
    border: 1px solid color-mix(in srgb, var(--err) 45%, var(--border));
    background: color-mix(in srgb, var(--err) 12%, var(--bg-elev));
    color: var(--err);
    border-radius: 10px;
    padding: 8px 12px;
    font-size: 12px;
    line-height: 1.35;
}

.stock-detail-row {
    display: grid;
    grid-template-columns: minmax(150px, 220px) minmax(220px, 1fr);
    gap: 12px;
    align-items: center;
}

.stock-detail-label {
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.4px;
    color: var(--text-subtle);
    font-weight: 700;
}

.stock-detail-input {
    min-height: 38px;
    display: inline-flex;
    align-items: center;
    padding: 0 12px;
    border-radius: 10px;
    border: 1px solid var(--border);
    background: var(--bg-elev);
    color: var(--text);
    font-size: 12px;
}

.stock-detail-trigger {
    justify-content: flex-start;
    text-align: left;
    cursor: text;
}

.stock-detail-field-value {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    width: 100%;
}

.stock-detail-original-group {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    margin-left: auto;
}

.stock-detail-original-value {
    color: #c2410c;
    font-size: 11px;
    white-space: nowrap;
}

.stock-detail-revert-btn {
    border: 1px solid color-mix(in srgb, #c2410c 45%, var(--border));
    border-radius: 8px;
    background: color-mix(in srgb, #c2410c 12%, var(--bg-elev));
    color: #c2410c;
    min-width: 28px;
    min-height: 28px;
    cursor: pointer;
    line-height: 1;
}

.stock-detail-revert-btn:hover {
    background: color-mix(in srgb, #c2410c 18%, var(--bg-elev));
}

.stock-detail-readonly {
    color: var(--text-subtle);
    font-size: 12px;
    line-height: 1.4;
    min-height: 20px;
    display: inline-flex;
    align-items: center;
}

.stock-inline-select {
    min-width: 120px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--bg-elev);
    color: var(--text);
    padding: 5px 8px;
    font-size: 11px;
}

.stock-inline-select.status-in-stock {
    color: var(--ok);
    border-color: color-mix(in srgb, var(--ok) 45%, var(--border));
    background: color-mix(in srgb, var(--ok) 14%, var(--bg-elev));
}

.stock-inline-select.status-out-of-stock {
    color: var(--err);
    border-color: color-mix(in srgb, var(--err) 45%, var(--border));
    background: color-mix(in srgb, var(--err) 14%, var(--bg-elev));
}

.toolset-slot-select.state-spare {
    color: var(--ok);
    border-color: color-mix(in srgb, var(--ok) 45%, var(--border));
    background: color-mix(in srgb, var(--ok) 14%, var(--bg-elev));
}

.toolset-slot-select.state-do-not-use {
    color: #7a1f3d;
    border-color: color-mix(in srgb, #7a1f3d 45%, var(--border));
    background: color-mix(in srgb, #7a1f3d 14%, var(--bg-elev));
}

.toolset-slot-select option.toolset-slot-option-spare {
    color: var(--ok);
}

.toolset-slot-select option.toolset-slot-option-do-not-use {
    color: #7a1f3d;
}

.project-ref-select.broken-ref-select {
    color: #c56a10;
    border-color: color-mix(in srgb, #c56a10 55%, var(--border));
    background: color-mix(in srgb, #c56a10 16%, var(--bg-elev));
}

.project-layout {
    height: 100%;
    display: grid;
    grid-template-columns: 1fr 340px;
    gap: 12px;
}

.project-main {
    display: flex;
    flex-direction: column;
    gap: 10px;
    min-height: 0;
}

.project-view-tabs {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
}

.project-view-tab {
    border: 1px solid var(--border);
    background: var(--bg-elev);
    color: var(--text);
    border-radius: 999px;
    padding: 7px 11px;
    font-size: 12px;
    font-weight: 700;
    cursor: pointer;
}

.project-view-tab.active {
    border-color: color-mix(in srgb, var(--accent) 65%, var(--border));
    background: color-mix(in srgb, var(--accent) 20%, transparent);
    color: var(--text);
}

.machining-summary {
    display: grid;
    grid-template-columns: repeat(2, minmax(220px, 1fr));
    gap: 10px;
}

/* Tooling plan view: per-step tool-selection + requirements tables. */
.tooling-view {
    display: flex;
    flex-direction: column;
    gap: 14px;
    align-items: stretch;
    text-align: left;
}

.tooling-step {
    display: flex;
    flex-direction: column;
    gap: 8px;
}

.tooling-step-title {
    font-size: 14px;
    margin: 0;
}

.tooling-subtitle {
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.4px;
    color: var(--text-subtle);
    margin: 6px 0 0;
}

.tooling-separator {
    border: none;
    border-top: 1px solid var(--border);
    margin: 6px 0;
}

.tooling-table {
    width: 100%;
    border-collapse: collapse;
    font-size: 12px;
}

.tooling-table th,
.tooling-table td {
    text-align: left;
    padding: 7px 9px;
    border-bottom: 1px solid var(--border);
}

.tooling-table th {
    font-size: 11px;
    text-transform: uppercase;
    color: var(--text-subtle);
    letter-spacing: 0.35px;
}

.tooling-slot-col {
    width: 5rem;
}

.tooling-count-col {
    width: 6rem;
}

.tooling-slot {
    font-variant-numeric: tabular-nums;
    font-weight: 700;
    white-space: nowrap;
}

.tooling-count {
    font-variant-numeric: tabular-nums;
    text-align: right;
}

/* One line per tool within a requirement cell (an oblong/slot may list two). */
.tooling-tool-line {
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
}

.tooling-role {
    color: var(--text-subtle);
    font-size: 11px;
}

/* Highlight a requirement that is milled by a router rather than drilled. */
.tooling-req-routed {
    background: color-mix(in srgb, var(--accent) 8%, transparent);
}

.tooling-routed-badge {
    margin-left: 6px;
    padding: 0 6px;
    border-radius: 999px;
    font-size: 10px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.3px;
    color: var(--accent);
    background: color-mix(in srgb, var(--accent) 16%, transparent);
    border: 1px solid color-mix(in srgb, var(--accent) 40%, var(--border));
}

/* Size delta colouring: within 2 % = good, beyond = attention. */
.tooling-delta-ok {
    color: var(--ok);
}

.tooling-delta-warn {
    color: var(--warn);
    font-weight: 600;
}

.tooling-error {
    border: 1px solid color-mix(in srgb, var(--err) 55%, var(--border));
    background: color-mix(in srgb, var(--err) 12%, var(--bg-elev));
    border-radius: 10px;
    padding: 10px 12px;
    display: flex;
    flex-direction: column;
    gap: 4px;
}

.tooling-error-title {
    font-weight: 700;
    color: var(--err);
}

.tooling-error ul {
    margin: 0;
    padding-left: 18px;
    font-size: 12px;
}

.tooling-warnings {
    display: flex;
    flex-direction: column;
    gap: 2px;
    margin-top: 2px;
}

.tooling-warning {
    font-size: 12px;
    color: var(--warn);
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

.profile-screen-panel {
    display: flex;
    flex-direction: column;
    overflow: hidden;
}

.profile-manager-shell {
    flex: 1;
    min-height: 0;
    overflow: hidden;
}

.profile-editor-shell {
    flex: 1;
    min-height: 0;
    overflow: hidden;
}

.profile-editor-top {
    flex: 0 0 auto;
    border-bottom: 1px solid var(--border);
    padding-bottom: 8px;
}

.profile-editor-scroll {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding-right: 4px;
}

.profile-editor-scroll .edit-grid {
    overflow: visible;
}

.profile-editor-scroll .edit-grid.process-edit-grid {
    grid-template-columns: minmax(0, 1fr);
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

/* The editable control(s) of one field. Capped to a readable width so a field
   never stretches across a full wide column; the width is a percentage with a
   floor, so shrinking the window stops it at the minimum rather than collapsing
   it. Multiline editors (CNC GTL templates) opt out and fill the width;
   checkboxes shrink to their natural size. */
.field-control {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 30%;
    min-width: 12rem;
    max-width: 32rem;
}

.field-control > input:not([type="checkbox"]),
.field-control > select {
    flex: 1 1 auto;
    min-width: 0;
    width: 100%;
}

.field-control-wide {
    width: 100%;
    max-width: none;
}

.field-control-wide > textarea {
    flex: 1 1 auto;
    width: 100%;
}

.field-control-check {
    width: auto;
    min-width: 0;
}

/* Schema-driven fields (SchemaField) ------------------------------------- */
.field-hint {
    margin: 2px 0 0;
    font-size: 12px;
    line-height: 1.4;
    opacity: 0.62;
}

.field > label > .field-required {
    color: #d64545;
    font-weight: 700;
}

.field.field-invalid > label {
    color: #d64545;
}

.field.field-invalid .field-control > input,
.field.field-invalid .field-control > select {
    border-color: #d64545;
    box-shadow: 0 0 0 1px color-mix(in srgb, #d64545 35%, transparent);
}

/* A stock field whose value has been edited away from its catalog original: the
   label is tinted and an orange revert (↺) control sits at the field's corner. */
.field.field-changed {
    position: relative;
}

.field.field-changed > label {
    color: #e08a00;
}

.stock-revert-btn {
    flex: 0 0 auto;
    border: none;
    background: transparent;
    color: #e08a00;
    cursor: pointer;
    font-size: 15px;
    line-height: 1;
    padding: 1px 5px;
    border-radius: 6px;
}

.stock-revert-btn:hover {
    background: color-mix(in srgb, #e08a00 16%, transparent);
}

/* Reload-PCB affordance next to the board name: the same tiny glyph-button style
   as the stock revert control, but inline and neutrally tinted. */
.board-reload-btn {
    border: none;
    background: transparent;
    color: inherit;
    cursor: pointer;
    font-size: 15px;
    line-height: 1;
    padding: 1px 6px;
    border-radius: 6px;
    opacity: 0.6;
}

.board-reload-btn:hover {
    opacity: 1;
    background: color-mix(in srgb, currentColor 14%, transparent);
}

/* Schema-driven machining detail: sections, nested subsections, and pickers. */
.schema-section {
    grid-column: 1 / -1;
    margin-top: 10px;
    padding-top: 10px;
    border-top: 1px solid color-mix(in srgb, currentColor 12%, transparent);
}

.schema-section > .section-title {
    margin: 0 0 8px;
    font-size: 13px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    opacity: 0.75;
}

/* Multi-step machining editor: each step is a bordered card with reorder/remove
   controls; the whole set can grow via the dashed add-step button. */
.step-card {
    grid-column: 1 / -1;
    border: 1px solid color-mix(in srgb, currentColor 14%, transparent);
    border-radius: 8px;
    padding: 12px 14px;
    margin-top: 12px;
}

.step-card-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
}

.step-card-actions {
    display: flex;
    gap: 4px;
}

.icon-btn {
    border: 1px solid color-mix(in srgb, currentColor 18%, transparent);
    background: transparent;
    color: inherit;
    border-radius: 6px;
    width: 26px;
    height: 26px;
    line-height: 1;
    cursor: pointer;
    opacity: 0.7;
}

.icon-btn:hover:not(:disabled) {
    opacity: 1;
    background: color-mix(in srgb, currentColor 8%, transparent);
}

.icon-btn:disabled {
    opacity: 0.3;
    cursor: default;
}

.icon-btn-danger:hover:not(:disabled) {
    color: #d9534f;
    border-color: #d9534f;
}

.add-step-btn {
    grid-column: 1 / -1;
    margin-top: 12px;
    padding: 8px 12px;
    border: 1px dashed color-mix(in srgb, currentColor 30%, transparent);
    background: transparent;
    color: inherit;
    border-radius: 8px;
    cursor: pointer;
    font-weight: 600;
    opacity: 0.85;
}

.add-step-btn:hover {
    opacity: 1;
    background: color-mix(in srgb, currentColor 6%, transparent);
}

.diag-warning {
    color: #b8860b;
    font-weight: 600;
}

/* Job summary: an aligned two-column (label · value) table. Muted uppercase
   labels line up in the first column; values are emphasized in the second. */
.job-summary {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 5px 14px;
    margin-top: 6px;
    align-items: baseline;
}

.job-summary-label {
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.03em;
    opacity: 0.55;
    white-space: nowrap;
}

.job-summary-value {
    font-size: 12.5px;
    font-weight: 600;
    word-break: break-word;
}

.schema-subsection {
    margin: 6px 0 6px 6px;
    padding-left: 10px;
    border-left: 2px solid color-mix(in srgb, currentColor 10%, transparent);
}

.schema-subsection-title {
    margin: 0 0 4px;
    font-size: 12px;
    font-weight: 600;
    opacity: 0.7;
}

.binding-picker,
.operations-editor {
    display: flex;
    flex-direction: column;
    gap: 4px;
}

.binding-row {
    display: flex;
    align-items: center;
    gap: 8px;
}

.binding-row .binding-name {
    font-size: 13px;
}

/* Toolset "rack" editor: T1..Tn rows stacked in a single vertical column.
 * Distinct from the job screen's `.rack-grid` card grid, which wraps into
 * multiple columns — sharing that class here forced these rows sideways. */
.rack-slot-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
}

.rack-slot-row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 8px;
    border-radius: 8px;
    border-left: 3px solid transparent;
}

/* Slot state colours: assigned (fixed) = green, spare = neutral, do-not-use = red. */
.rack-slot-fixed {
    border-left-color: var(--ok);
    background: color-mix(in srgb, var(--ok) 10%, transparent);
}

.rack-slot-spare {
    border-left-color: color-mix(in srgb, var(--text-subtle) 45%, transparent);
    background: color-mix(in srgb, var(--text-subtle) 6%, transparent);
}

.rack-slot-donotuse {
    border-left-color: var(--err);
    background: color-mix(in srgb, var(--err) 10%, transparent);
    opacity: 0.7;
}

.rack-slot-donotuse .rack-slot-label {
    color: var(--err);
}

.rack-slot-label {
    min-width: 42px;
    font-weight: 600;
    font-size: 13px;
}

.rack-slot-row select {
    flex: 1;
}

.binding-selector {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 8px;
}

.binding-selector.open {
    z-index: 60;
}

.binding-selector-backdrop {
    position: fixed;
    inset: 0;
    z-index: 50;
    background: transparent;
}

.binding-selector-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
}

.binding-selector-body {
    border-radius: 6px;
    background: color-mix(in srgb, var(--bg-elev) 70%, transparent);
    min-height: 42px;
    padding: 6px 8px;
}

.binding-selector-body.open {
    position: relative;
    z-index: 61;
    outline: 1px solid color-mix(in srgb, var(--accent) 60%, var(--border));
    box-shadow: 0 0 0 1px color-mix(in srgb, var(--accent) 20%, transparent);
}

.binding-selector-body.pending {
    background: color-mix(in srgb, var(--warn) 18%, var(--bg-elev));
}

.binding-selector-body.open.pending {
    outline-color: color-mix(in srgb, var(--warn) 70%, var(--border));
    box-shadow: 0 0 0 1px color-mix(in srgb, var(--warn) 24%, transparent);
}

.binding-selector-empty {
    font-size: 12px;
    color: var(--text-subtle);
}

.binding-selector-summary-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
}

.binding-list-row {
    display: grid;
    grid-template-columns: 16px 1fr;
    align-items: center;
    gap: 8px;
    padding: 2px 0;
    font-size: 12px;
    color: var(--text);
}

.binding-list-row.default {
    color: var(--accent);
}

.binding-list-row-tick {
    font-size: 12px;
    color: transparent;
}

.binding-list-row-tick.default {
    color: var(--accent);
}

.binding-list-row-label.default {
    font-weight: 700;
}

.binding-selector-editor {
    margin-top: 2px;
    padding-top: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
}

.binding-edit-row {
    display: grid;
    grid-template-columns: 16px 16px 1fr;
    align-items: center;
    gap: 8px;
    padding: 2px 0;
}

.binding-edit-row.selected {
    background: transparent;
}

.binding-edit-row.default {
    box-shadow: none;
}

.binding-edit-row-label {
    font-size: 12px;
    color: var(--text-subtle);
    font-weight: 500;
}

.binding-edit-row-label.selected {
    color: var(--text);
    font-weight: 700;
}

.binding-edit-row-label.default {
    font-weight: 700;
}

.binding-edit-row-default-tick {
    font-size: 12px;
    color: transparent;
}

.binding-edit-row-default-tick.default {
    color: var(--accent);
    font-weight: 700;
}

.field label {
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.4px;
    color: var(--text-subtle);
    font-weight: 700;
}

.required-pending > label,
.required-pending .checkbox-line,
.required-pending-help {
    color: var(--warn);
}

.required-pending > .sub-field > input,
.required-pending > .sub-field > select,
.required-pending > .sub-field > textarea,
.required-pending > .sub-field > .stock-detail-input {
    border-color: color-mix(in srgb, var(--warn) 65%, var(--border));
    background: color-mix(in srgb, var(--warn) 10%, var(--bg-elev));
}

.project-layout .panel.fixed .field > label {
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

.edit-grid .field.section-subfield:not(.section-block) {
    display: grid;
    grid-template-columns: minmax(150px, 220px) minmax(220px, 1fr);
    gap: 12px;
    align-items: center;
}

.edit-grid .field.section-subfield:not(.section-block) > label {
    margin: 0;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.4px;
    color: var(--text-subtle);
    font-weight: 700;
}

.edit-grid .field.section-subfield:not(.section-block) > .sub-field {
    min-width: 0;
    margin-left: 0;
    margin-top: 0;
}

.edit-grid .field.section-subfield:not(.section-block) > .sub-field > input,
.edit-grid .field.section-subfield:not(.section-block) > .sub-field > select,
.edit-grid .field.section-subfield:not(.section-block) > .sub-field > textarea,
.edit-grid .field.section-subfield:not(.section-block) > .sub-field > .stock-detail-input,
.edit-grid .field.section-subfield:not(.section-block) > .sub-field > .diag-status {
    width: 100%;
}

.edit-grid.read-only {
    pointer-events: none;
    opacity: 0.58;
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

.tool-type-chip {
    display: inline-flex;
    align-items: center;
    border-radius: 999px;
    padding: 3px 8px;
    font-size: 11px;
    font-weight: 700;
}

.tool-type-drill {
    color: var(--accent);
    background: color-mix(in srgb, var(--accent) 18%, transparent);
}

.tool-type-router {
    color: var(--ok);
    background: color-mix(in srgb, var(--ok) 18%, transparent);
}

.tool-type-vbit {
    color: var(--warn);
    background: color-mix(in srgb, var(--warn) 20%, transparent);
}

.tool-type-engraving {
    color: var(--err);
    background: color-mix(in srgb, var(--err) 18%, transparent);
}

.status-in-stock {
    color: var(--ok);
    background: color-mix(in srgb, var(--ok) 20%, transparent);
}

.status-out-of-stock {
    color: var(--err);
    background: color-mix(in srgb, var(--err) 20%, transparent);
}

.atc-indicator {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    color: var(--ok);
    font-weight: 600;
    font-size: 12px;
}

.atc-dot {
    display: inline-block;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--ok);
}

.atc-empty {
    display: inline-block;
    color: var(--text-subtle);
    font-size: 12px;
}

.status-preferred {
    color: var(--ok);
    background: color-mix(in srgb, var(--ok) 16%, transparent);
}

.status-neutral,
.status-new {
    color: var(--text-subtle);
    background: color-mix(in srgb, var(--text-subtle) 18%, transparent);
}

.status-not-preferred {
    color: color-mix(in srgb, var(--err) 88%, white);
    background: color-mix(in srgb, var(--err) 14%, transparent);
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
    border-radius: 0;
    border: 1px solid var(--border);
    background: color-mix(in srgb, var(--bg-elev) 82%, transparent);
    overflow: hidden;
    cursor: grab;
}

.board-canvas.is-panning {
    cursor: grabbing;
}

.board-preview-layout {
    display: grid;
    grid-template-columns: minmax(420px, 1fr) 260px;
    gap: 12px;
    width: min(1040px, 98%);
    align-items: start;
}

.board-drill-legend-panel {
    border: 1px solid var(--border);
    background: color-mix(in srgb, var(--bg-elev) 82%, transparent);
    padding: 10px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    text-align: left;
}

.board-drill-legend-panel h4 {
    margin: 0;
    font-size: 13px;
}

.board-drill-legend-item {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
    color: var(--text-subtle);
}

.board-drill-legend-icon {
    width: 22px;
    height: 22px;
    display: block;
    flex: 0 0 22px;
}

.board-drill-legend-note {
    margin-top: 4px;
    font-size: 11px;
    color: var(--text-subtle);
}

.board-svg {
    width: 100%;
    height: auto;
    display: block;
}

.board-svg-frame {
    fill: color-mix(in srgb, var(--bg) 78%, transparent);
    stroke: color-mix(in srgb, var(--border) 88%, transparent);
    stroke-width: 1;
}

.board-hole-cross {
    stroke: currentColor;
    stroke-width: 2;
    stroke-linecap: round;
    vector-effect: non-scaling-stroke;
    opacity: 0.95;
}

.board-hole-via {
    color: color-mix(in srgb, var(--accent) 85%, white);
}

.board-hole-pth {
    color: color-mix(in srgb, var(--ok) 78%, var(--text));
}

.board-hole-npth {
    color: color-mix(in srgb, var(--warn) 85%, var(--text));
}

.board-hole-legend {
    color: #111111;
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

.board-edge-shape {
    fill: none;
    stroke: color-mix(in srgb, var(--ok) 70%, var(--accent));
    stroke-width: 3;
    stroke-linecap: butt;
    stroke-linejoin: miter;
    stroke-miterlimit: 10;
    vector-effect: non-scaling-stroke;
    opacity: 0.85;
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

@media (max-width: 1100px) {
    .board-preview-layout {
        grid-template-columns: 1fr;
    }
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

/* Clickable catalog rows — click a row to view its tools; active row is tinted. */
.catalog-row {
    cursor: pointer;
}

.catalog-row:hover td {
    background: color-mix(in srgb, var(--accent) 7%, transparent);
}

.catalog-row.active td {
    background: color-mix(in srgb, var(--accent) 15%, transparent);
}

/* Section header row inside the read-only catalog contents table. */
.catalog-section-row td {
    background: var(--bg-subtle);
    font-weight: 700;
    font-size: 10.5px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--text-subtle);
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

    .project-layout {
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

/* In-context help ------------------------------------------------------- */
.stock-toolbar-heading {
    display: flex;
    align-items: center;
    gap: 10px;
}

.help-trigger {
    white-space: nowrap;
}

.help-overlay {
    position: absolute;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    z-index: 120;
    display: grid;
    place-items: center;
    padding: 24px;
}

.help-panel {
    width: min(760px, 94vw);
    max-height: 86vh;
    background: var(--bg-subtle);
    border: 1px solid var(--border);
    border-radius: 14px;
    box-shadow: var(--shadow);
    display: flex;
    flex-direction: column;
    overflow: hidden;
}

.help-panel-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 14px 16px;
    border-bottom: 1px solid var(--border);
}

.help-panel-head h2 {
    margin: 0;
    font-size: 16px;
}

.help-markdown {
    overflow: auto;
    padding: 4px 20px 20px;
    font-size: 13px;
    line-height: 1.6;
    color: var(--text);
}

.help-markdown h1 {
    font-size: 18px;
    margin: 20px 0 8px;
}

.help-markdown h2 {
    font-size: 15px;
    margin: 20px 0 8px;
}

.help-markdown h3 {
    font-size: 13px;
    margin: 16px 0 6px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-subtle);
}

.help-markdown p {
    margin: 8px 0;
}

.help-markdown ul,
.help-markdown ol {
    margin: 8px 0;
    padding-left: 22px;
}

.help-markdown li {
    margin: 3px 0;
}

.help-markdown a {
    color: var(--accent);
    text-decoration: none;
}

.help-markdown a:hover {
    text-decoration: underline;
}

.help-markdown code {
    font-family: "Cascadia Code", "Consolas", monospace;
    font-size: 12px;
    background: var(--bg-elev);
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 1px 5px;
}

.help-markdown pre {
    background: var(--bg-elev);
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 12px 14px;
    overflow-x: auto;
    margin: 10px 0;
}

.help-markdown pre code {
    background: none;
    border: none;
    padding: 0;
    line-height: 1.5;
}

.help-markdown table {
    border-collapse: collapse;
    width: 100%;
    margin: 12px 0;
    font-size: 12px;
}

.help-markdown th,
.help-markdown td {
    border: 1px solid var(--border);
    padding: 6px 10px;
    text-align: left;
    vertical-align: top;
}

.help-markdown th {
    background: var(--bg-elev);
    font-weight: 600;
}

.help-markdown blockquote {
    margin: 10px 0;
    padding: 4px 14px;
    border-left: 3px solid var(--border);
    color: var(--text-subtle);
}
"#;


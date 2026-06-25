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

body {
    margin: 0;
    font-family: "Segoe UI", "Noto Sans", sans-serif;
    color: var(--text);
    background: radial-gradient(circle at 0% 0%, #232933 0%, var(--bg) 45%);
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
    width: 22px;
    height: 22px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: 7px;
    background: color-mix(in srgb, var(--bg-elev) 80%, transparent);
    border: 1px solid color-mix(in srgb, var(--border) 85%, transparent);
    flex: 0 0 auto;
}

.rail-icon-svg {
    width: 15px;
    height: 15px;
    fill: none;
    stroke: currentColor;
    stroke-width: 1.8;
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

.shell-content {
    flex: 1;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
    background:
        radial-gradient(circle at top left, rgba(245, 158, 11, 0.08), transparent 22%),
        linear-gradient(180deg, color-mix(in srgb, var(--bg) 92%, #000 8%) 0%, var(--bg) 100%);
}

.screen-host {
    height: 100%;
    min-height: 0;
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

.shell-statusbar {
    min-height: 30px;
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

.board-edge-shape {
    fill: none;
    stroke: color-mix(in srgb, var(--ok) 70%, var(--accent));
    stroke-width: 3;
    stroke-linecap: butt;
    stroke-linejoin: miter;
    stroke-miterlimit: 10;
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

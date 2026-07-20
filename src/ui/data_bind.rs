//! Reactive binding layer between the Dioxus UI and the global [`AppData`] store.
//!
//! `AppData` isn't `Clone` (it owns a writer thread), so — unlike the legacy
//! `Signal<AppCtx>` snapshot — the UI cannot hold it in a signal. Instead the
//! store lives in a process-wide `RwLock` (see [`crate::data`]) and reactivity is
//! driven by a single render-counter [`GlobalSignal`]: reads subscribe to it,
//! writes bump it. Components read fields through [`use_field`] and mutate
//! through [`set_input`] / [`set_bool`], keeping RSX free of data plumbing.

#![allow(dead_code)]

use dioxus::prelude::*;
use uuid::Uuid;

use crate::data::{with_appdata, with_appdata_mut, appdata_ready};
use crate::domain::UnitSystem;
use crate::ui::unit_service;
use datastore::{FieldKind, Node, NodeValue, RemoveError, UnitValue};
use serde_json::Value;

/// Monotonic reactivity tick. Any component that reads it (via [`use_field`])
/// re-renders when a mutation bumps it (via [`bump_render`]).
static RENDER_TICK: GlobalSignal<u64> = Signal::global(|| 0);

/// Subscribes the calling component to store mutations.
fn subscribe() {
    let _ = RENDER_TICK();
}

/// Signals that the store changed, triggering re-render of subscribed components.
pub fn bump_render() {
    *RENDER_TICK.write() += 1;
}

/// A leaf node projected into what a widget needs — owned, so no store lock is
/// held while rendering.
#[derive(Clone, PartialEq)]
pub struct FieldView {
    pub label: String,
    pub description: Option<String>,
    pub required: bool,
    pub kind: FieldKind,
    pub value: NodeValue,
    pub display: String,
    pub enum_options: Vec<String>,
    pub default_applied: bool,
    pub incomplete: bool,
}

/// Where a bound field lives: a profile document addressed by root identity, or
/// the identity-less **stock singleton** addressed by its file. Lets one field
/// widget and one form renderer serve both the id-based profile screens and the
/// path-based stock screen without duplicating the widget logic.
#[derive(Clone, Copy, PartialEq)]
pub enum FieldAddr {
    Doc(Uuid),
    Stock,
}

/// Projects a resolved node into the owned [`FieldView`] a widget renders from
/// (so no store lock is held while rendering).
fn project_field(node: &Node) -> FieldView {
    FieldView {
        label: node
            .meta
            .title
            .clone()
            .unwrap_or_else(|| titleize(&node.meta.name)),
        description: node.meta.description.clone(),
        required: node.meta.required,
        kind: node.meta.kind.clone(),
        value: node.value.clone(),
        display: node_display(&node.value),
        enum_options: match &node.meta.kind {
            FieldKind::Enum(options) => options.clone(),
            _ => Vec::new(),
        },
        default_applied: node.meta.default_applied,
        incomplete: !node.status.is_complete(),
    }
}

/// Reads the field at `ptr` under `addr` (no subscription — reactive callers go
/// through [`use_field`]/[`use_stock_field`]). `None` if the store isn't ready or
/// the field is absent.
fn addr_field(addr: FieldAddr, ptr: &str) -> Option<FieldView> {
    if !appdata_ready() {
        return None;
    }
    with_appdata(|data| {
        let doc = match addr {
            FieldAddr::Doc(id) => data.get(id)?,
            FieldAddr::Stock => data.stock()?,
        };
        doc.root.get_pointer(ptr).map(project_field)
    })
}

/// The child property names of the object at `ptr` under `addr`, in schema order.
fn addr_object_children(addr: FieldAddr, ptr: &str) -> Vec<String> {
    if !appdata_ready() {
        return Vec::new();
    }
    with_appdata(|data| {
        let doc = match addr {
            FieldAddr::Doc(id) => data.get(id),
            FieldAddr::Stock => data.stock(),
        };
        doc.and_then(|doc| doc.root.get_pointer(ptr))
            .map(|node| match &node.value {
                NodeValue::Object(map) => map.keys().cloned().collect(),
                _ => Vec::new(),
            })
            .unwrap_or_default()
    })
}

/// Sets a field from a raw input string (schema-decoded) under `addr`, bumping the
/// render tick.
fn addr_set_input(addr: FieldAddr, ptr: &str, raw: &str) {
    with_appdata_mut(|data| match addr {
        FieldAddr::Doc(id) => data.set_str(id, ptr, raw),
        FieldAddr::Stock => data.set_stock_str(ptr, raw),
    });
    bump_render();
}

/// Sets a typed value directly under `addr` (checkbox/enum/unit), bumping the tick.
fn addr_set_value(addr: FieldAddr, ptr: &str, value: NodeValue) {
    with_appdata_mut(|data| match addr {
        FieldAddr::Doc(id) => data.set_field(id, ptr, value),
        FieldAddr::Stock => data.set_stock_value(ptr, value),
    });
    bump_render();
}

/// Reads one field of the document `id` at JSON Pointer `ptr`, subscribing the
/// component to future mutations. Returns `None` if the store isn't ready or the
/// field doesn't exist.
pub fn use_field(id: Uuid, ptr: &str) -> Option<FieldView> {
    subscribe();
    addr_field(FieldAddr::Doc(id), ptr)
}

/// Reads one field of the **stock singleton** at `ptr`, subscribing to mutations.
pub fn use_stock_field(ptr: &str) -> Option<FieldView> {
    subscribe();
    addr_field(FieldAddr::Stock, ptr)
}

/// Sets a field from a raw input string (schema-decoded) and triggers re-render.
pub fn set_input(id: Uuid, ptr: &str, raw: String) {
    addr_set_input(FieldAddr::Doc(id), ptr, &raw);
}

/// Sets a boolean field directly (from a checkbox) and triggers re-render.
pub fn set_bool(id: Uuid, ptr: &str, on: bool) {
    addr_set_value(FieldAddr::Doc(id), ptr, NodeValue::Bool(on));
}

/// Lists profiles of `kind` as `(id, name)`, subscribing to store mutations.
pub fn use_profiles(kind: crate::data::Profile) -> Vec<(Uuid, String)> {
    subscribe();
    if !appdata_ready() {
        return Vec::new();
    }
    with_appdata(|data| {
        data.list(kind)
            .into_iter()
            .map(|(id, doc)| {
                let name = doc
                    .root
                    .get_pointer("/name")
                    .and_then(|node| match &node.value {
                        NodeValue::Str(s) => Some(s.clone()),
                        _ => None,
                    })
                    .unwrap_or_else(|| id.to_string());
                (id, name)
            })
            .collect()
    })
}

/// Creates a new profile of `kind` from schema defaults; returns its id.
pub fn create_profile(kind: crate::data::Profile) -> Option<Uuid> {
    let id = with_appdata_mut(|data| data.create(kind).ok());
    bump_render();
    id
}

/// Removes a profile by id (no-op if it is still referenced elsewhere).
pub fn remove_profile(id: Uuid) {
    let _ = with_appdata_mut(|data| data.remove(id));
    bump_render();
}

/// Removes a profile, returning a user-facing message if it is blocked because
/// something still references it (or was not found).
pub fn remove_profile_result(id: Uuid) -> Result<(), String> {
    let result = with_appdata_mut(|data| data.remove(id));
    bump_render();
    result.map_err(|error| match error {
        RemoveError::InUse { referrers, .. } => format!(
            "Cannot delete: still referenced by {} item(s).",
            referrers.len()
        ),
        RemoveError::NotFound(_) => "Profile not found.".to_string(),
    })
}

/// Creates a named profile, seeding from a template when `kind` supports one and
/// `template_key` is non-empty (currently CNC). Returns the new id.
pub fn create_named_from_template(
    kind: crate::data::Profile,
    template_key: &str,
    name: &str,
) -> Option<Uuid> {
    let id = if kind == crate::data::Profile::Cnc && !template_key.is_empty() {
        with_appdata_mut(|data| data.create_cnc_from_template(template_key).ok())?
    } else {
        with_appdata_mut(|data| data.create(kind).ok())?
    };
    with_appdata_mut(|data| data.set_field(id, "/name", NodeValue::Str(name.to_string())));
    bump_render();
    Some(id)
}

/// The current store-revision counter, subscribing the caller to store
/// mutations without reading a specific field. Lets a screen react to *any*
/// AppData change — e.g. to keep a legacy in-memory projection coherent.
pub fn data_revision() -> u64 {
    RENDER_TICK()
}

/// The bundled CNC templates as `(key, label)` pairs for the ProfileManager add
/// dialog. Subscribes to store mutations for consistency with the other reads.
pub fn use_cnc_templates() -> Vec<(String, String)> {
    subscribe();
    if !appdata_ready() {
        return Vec::new();
    }
    with_appdata(|data| {
        data.cnc_templates()
            .into_iter()
            .map(|template| (template.key, template.name))
            .collect()
    })
}

/// Creates a CNC profile from the bundled template `key`, keeping the template's
/// own name (the setup screen's quick-add, which does not prompt for a name).
/// Returns the new id.
pub fn create_cnc_template(key: &str) -> Option<Uuid> {
    let id = with_appdata_mut(|data| data.create_cnc_from_template(key).ok())?;
    bump_render();
    Some(id)
}

/// Rebuilds the legacy in-memory `machines` projection from the AppData-owned CNC
/// documents. AppData is the file writer for the CNC realm; this mirrors the data
/// back into the legacy copy read by the GCode generator and the active machine
/// selection, so a session stays coherent. Does not persist (AppData already
/// wrote the files).
pub fn refresh_legacy_cnc() {
    let values: Vec<Value> = with_appdata(|data| {
        data.list(crate::data::Profile::Cnc)
            .into_iter()
            .map(|(_, doc)| doc.to_value())
            .collect()
    });
    crate::app_state_impl::with_ctx_mut(|ctx| ctx.refresh_machines(&values));
}

/// Rebuilds the legacy in-memory `process_profiles` projection from the
/// AppData-owned machining documents, keeping the GCode generator and the active
/// selection coherent. Does not persist (AppData already wrote the files).
pub fn refresh_legacy_machining() {
    let values: Vec<Value> = with_appdata(|data| {
        data.list(crate::data::Profile::Machining)
            .into_iter()
            .map(|(_, doc)| doc.to_value())
            .collect()
    });
    crate::app_state_impl::with_ctx_mut(|ctx| ctx.refresh_process_profiles(&values));
}

// ---------------------------------------------------------------------------
// Schema-driven form rendering.
//
// Rather than hand-authoring every field, a screen can render a document's
// object subtree straight from the schema: nested objects become subsections and
// leaves become `SchemaField`s. References and reference-arrays (the machining
// cnc/fixture/toolset bindings) and enum arrays (operations) get dedicated
// pickers, since they aren't expressible as plain fields.
// ---------------------------------------------------------------------------

/// The machining operation keys and labels — the `operation_key` enum from
/// `machining.yaml`, in schema order.
const MACHINING_OPERATIONS: &[(&str, &str)] = &[
    ("drill_locating_pins", "Drill locating pins"),
    ("drill_pth", "Drill plated holes (PTH)"),
    ("drill_npth", "Drill non-plated holes (NPTH)"),
    ("route_board", "Route board edge"),
    ("mill_board", "Mill board edge"),
];

/// The machining operations as `(key, label)` pairs, for screens that lay out
/// per-operation configuration.
pub fn machining_operations() -> &'static [(&'static str, &'static str)] {
    MACHINING_OPERATIONS
}

/// The child property names of the object node at `ptr`, in schema order (empty
/// if the node is missing or not an object). Subscribes to store mutations.
pub fn object_children(id: Uuid, ptr: &str) -> Vec<String> {
    subscribe();
    addr_object_children(FieldAddr::Doc(id), ptr)
}

/// Recursively renders the object subtree at `ptr` as a form: nested objects
/// become titled subsections, leaves become [`SchemaField`]s — the form is
/// generated from the schema, not hand-authored. References and reference/enum
/// arrays are not rendered here; use the dedicated pickers for those.
#[component]
pub fn SchemaForm(id: Uuid, ptr: String) -> Element {
    let children = object_children(id, &ptr);
    rsx! {
        for name in children {
            SchemaFormNode { id, ptr: format!("{ptr}/{name}") }
        }
    }
}

/// One node within a [`SchemaForm`]: an object recurses into a subsection; any
/// other kind renders as a [`SchemaField`].
#[component]
fn SchemaFormNode(id: Uuid, ptr: String) -> Element {
    let Some(field) = use_field(id, &ptr) else {
        return rsx! {};
    };
    if matches!(field.kind, FieldKind::Object) {
        rsx! {
            div { class: "schema-subsection",
                h5 { class: "schema-subsection-title", "{field.label}" }
                SchemaForm { id, ptr: ptr.clone() }
            }
        }
    } else {
        rsx! {
            SchemaField { id, ptr: ptr.clone() }
        }
    }
}

/// The stock-singleton twin of [`SchemaForm`]: recursively renders the object
/// subtree at `ptr` within the identity-less `stock.yaml` document. Used to drive
/// a stock tool's editable properties (`/tools/{i}/base`, …) straight from the
/// schema instead of a hand-written buffered editor.
#[component]
pub fn StockForm(ptr: String) -> Element {
    subscribe();
    let children = addr_object_children(FieldAddr::Stock, &ptr);
    rsx! {
        for name in children {
            StockFormNode { ptr: format!("{ptr}/{name}") }
        }
    }
}

/// One node within a [`StockForm`]: an object recurses into a subsection; any
/// other kind renders as a [`StockField`].
#[component]
fn StockFormNode(ptr: String) -> Element {
    let Some(field) = use_stock_field(&ptr) else {
        return rsx! {};
    };
    if matches!(field.kind, FieldKind::Object) {
        rsx! {
            div { class: "schema-subsection",
                h5 { class: "schema-subsection-title", "{field.label}" }
                StockForm { ptr: ptr.clone() }
            }
        }
    } else {
        rsx! {
            StockField { ptr: ptr.clone() }
        }
    }
}

/// A machining reference binding (`default` + `choices`), read from the document.
#[derive(Clone, Default, PartialEq)]
pub struct BindingView {
    pub default: Option<Uuid>,
    pub choices: Vec<Uuid>,
}

/// Extracts a UUID from a reference/id/string node value.
fn ref_uuid(value: &NodeValue) -> Option<Uuid> {
    match value {
        NodeValue::Ref(reference) => Some(reference.raw),
        NodeValue::Id(id) => Some(*id),
        NodeValue::Str(s) => Uuid::parse_str(s).ok(),
        _ => None,
    }
}

/// Reads the `default`/`choices` binding for machining `field` from document `id`.
fn read_binding_inner(id: Uuid, field: &str) -> BindingView {
    if !appdata_ready() {
        return BindingView::default();
    }
    with_appdata(|data| {
        let Some(doc) = data.get(id) else {
            return BindingView::default();
        };
        let default = doc
            .root
            .get_pointer(&format!("/{field}/default"))
            .and_then(|node| ref_uuid(&node.value));
        let choices = doc
            .root
            .get_pointer(&format!("/{field}/choices"))
            .map(|node| match &node.value {
                NodeValue::Array(items) => items.iter().filter_map(|it| ref_uuid(&it.value)).collect(),
                _ => Vec::new(),
            })
            .unwrap_or_default();
        BindingView { default, choices }
    })
}

/// Reads a machining binding, subscribing the caller to store mutations.
pub fn use_binding(id: Uuid, field: &str) -> BindingView {
    subscribe();
    read_binding_inner(id, field)
}

/// Reads a machining binding without subscribing — for use inside event handlers.
pub fn read_binding(id: Uuid, field: &str) -> BindingView {
    read_binding_inner(id, field)
}

/// Writes a machining binding (`default` + `choices`) and triggers re-render.
pub fn set_binding(id: Uuid, field: &str, default: Option<Uuid>, choices: &[Uuid]) {
    with_appdata_mut(|data| data.set_machining_binding(id, field, default, choices));
    bump_render();
}

/// Reads the enabled `operations` list of document `id`.
fn read_operations_inner(id: Uuid) -> Vec<String> {
    if !appdata_ready() {
        return Vec::new();
    }
    with_appdata(|data| {
        data.get(id)
            .and_then(|doc| doc.root.get_pointer("/operations"))
            .map(|node| match &node.value {
                NodeValue::Array(items) => items
                    .iter()
                    .filter_map(|it| match &it.value {
                        NodeValue::Str(s) => Some(s.clone()),
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            })
            .unwrap_or_default()
    })
}

/// Reads the enabled operations, subscribing the caller to store mutations.
pub fn use_operations(id: Uuid) -> Vec<String> {
    subscribe();
    read_operations_inner(id)
}

/// Reads the enabled operations without subscribing — for use in event handlers.
pub fn read_operations(id: Uuid) -> Vec<String> {
    read_operations_inner(id)
}

/// Writes the enabled `operations` and triggers re-render.
pub fn set_operations(id: Uuid, operations: &[String]) {
    with_appdata_mut(|data| data.set_machining_operations(id, operations));
    bump_render();
}

/// A reference-binding editor for a machining `field` (`"cnc"`/`"fixture"`/
/// `"toolset"`): tick the allowed `choices`, pick the active `default` (a radio
/// among the ticked). Options are the available profiles of `kind`.
#[component]
pub fn BindingPicker(id: Uuid, field: String, kind: crate::data::Profile, label: String) -> Element {
    let binding = use_binding(id, &field);
    let options = use_profiles(kind);

    rsx! {
        div { class: "field binding-picker",
            label { "{label}" }
            if options.is_empty() {
                p { class: "field-hint", "No profiles available yet." }
            }
            for (pid, name) in options {
                BindingRow {
                    id,
                    field: field.clone(),
                    pid,
                    name,
                    checked: binding.choices.contains(&pid),
                    is_default: binding.default == Some(pid),
                }
            }
        }
    }
}

/// One selectable profile within a [`BindingPicker`].
#[component]
fn BindingRow(id: Uuid, field: String, pid: Uuid, name: String, checked: bool, is_default: bool) -> Element {
    let field_toggle = field.clone();
    let field_default = field;
    rsx! {
        div { class: "binding-row",
            input {
                r#type: "checkbox",
                checked,
                onchange: move |evt| {
                    let current = read_binding(id, &field_toggle);
                    let mut next = current.choices;
                    let mut next_default = current.default;
                    if evt.checked() {
                        if !next.contains(&pid) {
                            next.push(pid);
                        }
                        if next_default.is_none() {
                            next_default = Some(pid);
                        }
                    } else {
                        next.retain(|c| *c != pid);
                        if next_default == Some(pid) {
                            next_default = next.first().copied();
                        }
                    }
                    set_binding(id, &field_toggle, next_default, &next);
                },
            }
            input {
                r#type: "radio",
                name: "binding-{field_default}-{id}",
                checked: is_default,
                disabled: !checked,
                onchange: move |_| {
                    let current = read_binding(id, &field_default);
                    set_binding(id, &field_default, Some(pid), &current.choices);
                },
            }
            span { class: "binding-name", "{name}" }
        }
    }
}

/// The machining operations toggle set: enables/disables each operation, keeping
/// the stored `operations` array in schema order.
#[component]
pub fn OperationsEditor(id: Uuid) -> Element {
    let enabled = use_operations(id);
    rsx! {
        div { class: "field operations-editor",
            label { "Operations" }
            for (key , op_label) in MACHINING_OPERATIONS.iter().copied() {
                OperationToggle {
                    id,
                    op_key: key.to_string(),
                    label: op_label.to_string(),
                    checked: enabled.iter().any(|op| op == key),
                }
            }
        }
    }
}

/// One operation checkbox within an [`OperationsEditor`].
#[component]
fn OperationToggle(id: Uuid, op_key: String, label: String, checked: bool) -> Element {
    rsx! {
        label { class: "checkbox-line",
            input {
                r#type: "checkbox",
                checked,
                onchange: move |evt| {
                    let mut current = read_operations(id);
                    if evt.checked() {
                        if !current.iter().any(|op| op == &op_key) {
                            current.push(op_key.clone());
                        }
                    } else {
                        current.retain(|op| op != &op_key);
                    }
                    // Persist in schema order regardless of click order.
                    let ordered: Vec<String> = MACHINING_OPERATIONS
                        .iter()
                        .filter(|(k, _)| current.iter().any(|op| op == k))
                        .map(|(k, _)| (*k).to_string())
                        .collect();
                    set_operations(id, &ordered);
                },
            }
            span { "{label}" }
        }
    }
}

/// Rebuilds the legacy in-memory `toolsets` projection (and the active rack) from
/// the AppData-owned toolset documents. Does not persist.
pub fn refresh_legacy_toolsets() {
    let values: Vec<Value> = with_appdata(|data| {
        data.list(crate::data::Profile::Toolset)
            .into_iter()
            .map(|(_, doc)| doc.to_value())
            .collect()
    });
    crate::app_state_impl::with_ctx_mut(|ctx| ctx.refresh_toolsets(&values));
}

/// Rebuilds the legacy in-memory `tools` (stock inventory) from the AppData-owned
/// stock singleton. Does not persist.
pub fn refresh_legacy_stock() {
    let value = with_appdata(|data| data.stock().map(|doc| doc.to_value()));
    if let Some(value) = value {
        crate::app_state_impl::with_ctx_mut(|ctx| ctx.refresh_tools(&value));
    }
}

/// Adds the catalog-picker selection to stock: builds the additions from the
/// legacy catalog (dedup vs current stock), projects them to stock-item values,
/// and appends them to the AppData document (the sole writer). Refreshes the
/// legacy projection and returns how many were added.
pub fn add_stock_from_catalog(selected_keys: &[String]) -> usize {
    let new_tools =
        crate::app_state_impl::with_ctx(|ctx| ctx.build_catalog_tool_additions(selected_keys));
    if new_tools.is_empty() {
        return 0;
    }
    let projected = crate::domain::stock::stock_value_from_tools(&new_tools);
    let items: Vec<Value> = projected
        .get("tools")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let added = with_appdata_mut(|data| data.append_stock_tool_values(&items));
    if added > 0 {
        refresh_legacy_stock();
    }
    bump_render();
    added
}

/// Removes stock tools by id from the AppData document (the sole writer), rebuilds
/// the legacy projection, and re-validates current-job references (a deleted tool
/// still used by the job becomes a reported broken reference). Returns the count
/// removed.
pub fn remove_stock_tools(ids: &[String]) -> usize {
    let removed = with_appdata_mut(|data| data.remove_stock_tools_by_ids(ids));
    if removed > 0 {
        refresh_legacy_stock();
        crate::app_state_impl::with_ctx_mut(|ctx| ctx.validate_current_job_references());
    }
    bump_render();
    removed
}

/// Sets the availability enum of the stock tool at array `index` (the table's
/// inline status toggle), decoding against the schema, then refreshes the
/// projection.
pub fn set_stock_availability(index: usize, in_stock: bool) {
    let raw = if in_stock { "in_stock" } else { "out_of_stock" };
    with_appdata_mut(|data| {
        data.set_stock_str(&format!("/tools/{index}/availability"), raw);
    });
    refresh_legacy_stock();
    bump_render();
}

/// Clones the stock tool at array `index` in the AppData document (fresh ids),
/// refreshes the legacy projection, and returns the new tool's id.
pub fn clone_stock_tool(index: usize) -> Option<String> {
    let new_index = with_appdata_mut(|data| data.clone_stock_item(index))?;
    let new_id = with_appdata(|data| {
        data.stock()
            .and_then(|doc| doc.root.get_pointer(&format!("/tools/{new_index}/id")))
            .and_then(|node| match &node.value {
                NodeValue::Id(id) => Some(id.to_string()),
                NodeValue::Str(s) => Some(s.clone()),
                _ => None,
            })
    });
    refresh_legacy_stock();
    bump_render();
    new_id
}

/// One rack slot projected for the [`RackGrid`].
#[derive(Clone, PartialEq)]
pub struct SlotView {
    pub pos: usize,
    pub index: u64,
    pub mode: String,
    pub tool_id: Option<Uuid>,
}

/// The rack slots of toolset `id`, in array order, subscribing to store mutations.
pub fn use_toolset_slots(id: Uuid) -> Vec<SlotView> {
    subscribe();
    if !appdata_ready() {
        return Vec::new();
    }
    with_appdata(|data| {
        let Some(node) = data.get(id).and_then(|doc| doc.root.get_pointer("/slots")) else {
            return Vec::new();
        };
        match &node.value {
            NodeValue::Array(items) => items
                .iter()
                .enumerate()
                .map(|(pos, item)| {
                    let index = item
                        .get_pointer("/index")
                        .and_then(|n| match &n.value {
                            NodeValue::Int(i) => Some(*i as u64),
                            _ => None,
                        })
                        .unwrap_or((pos + 1) as u64);
                    let mode = item
                        .get_pointer("/mode")
                        .and_then(|n| match &n.value {
                            NodeValue::Str(s) => Some(s.clone()),
                            _ => None,
                        })
                        .unwrap_or_else(|| "spare".to_string());
                    let tool_id = item.get_pointer("/tool_id").and_then(|n| ref_uuid(&n.value));
                    SlotView { pos, index, mode, tool_id }
                })
                .collect(),
            _ => Vec::new(),
        }
    })
}

/// Sets a rack slot's mode/tool and triggers re-render.
pub fn set_toolset_slot_mode(id: Uuid, slot_pos: usize, mode: &str, tool_id: Option<Uuid>) {
    with_appdata_mut(|data| data.set_toolset_slot_mode(id, slot_pos, mode, tool_id));
    bump_render();
}

/// Resizes a toolset's rack and triggers re-render.
pub fn set_toolset_slot_count(id: Uuid, count: usize) {
    with_appdata_mut(|data| data.set_toolset_slot_count(id, count));
    bump_render();
}

/// A toolset rack editor: a slot-count control plus one row per `T{n}` slot, each
/// choosing `spare` / `do_not_use` / a fixed stock tool. The tool options
/// (`(tool_id, label)`) are supplied by the screen (stock is not on the datastore
/// yet).
#[component]
pub fn RackGrid(id: Uuid, tools: Vec<(String, String)>) -> Element {
    let slots = use_toolset_slots(id);
    let count = slots.len();
    rsx! {
        div { class: "field",
            label { "Slot count" }
            input {
                r#type: "number",
                min: "1",
                max: "64",
                value: "{count}",
                onchange: move |evt| {
                    let n = evt.value().parse::<usize>().unwrap_or(1).clamp(1, 64);
                    set_toolset_slot_count(id, n);
                },
            }
        }
        div { class: "field rack-grid",
            label { "Slots" }
            for slot in slots {
                RackSlotRow {
                    id,
                    pos: slot.pos,
                    index: slot.index,
                    mode: slot.mode,
                    tool_id: slot.tool_id,
                    tools: tools.clone(),
                }
            }
        }
    }
}

/// One `T{index}` row in a [`RackGrid`].
#[component]
fn RackSlotRow(
    id: Uuid,
    pos: usize,
    index: u64,
    mode: String,
    tool_id: Option<Uuid>,
    tools: Vec<(String, String)>,
) -> Element {
    let selected_value = if mode == "do_not_use" {
        "do_not_use".to_string()
    } else if mode == "fixed" {
        tool_id.map(|t| format!("tool:{t}")).unwrap_or_else(|| "spare".to_string())
    } else {
        "spare".to_string()
    };
    let tool_missing = mode == "fixed"
        && tool_id.is_some()
        && !tools.iter().any(|(tid, _)| Uuid::parse_str(tid).ok() == tool_id);
    let missing_value = tool_id.map(|t| format!("tool:{t}")).unwrap_or_default();

    rsx! {
        div { class: "rack-slot-row",
            span { class: "rack-slot-label", "T{index}" }
            select {
                value: selected_value,
                onchange: move |evt| {
                    let value = evt.value();
                    if let Some(tid) = value.strip_prefix("tool:").and_then(|s| Uuid::parse_str(s).ok()) {
                        set_toolset_slot_mode(id, pos, "fixed", Some(tid));
                    } else if value == "do_not_use" {
                        set_toolset_slot_mode(id, pos, "do_not_use", None);
                    } else {
                        set_toolset_slot_mode(id, pos, "spare", None);
                    }
                },
                option { value: "spare", "Spare" }
                option { value: "do_not_use", "Do not use" }
                if tool_missing {
                    option { value: "{missing_value}", "Missing tool" }
                }
                for (tid , name) in tools {
                    option { value: "tool:{tid}", "{name}" }
                }
            }
        }
    }
}

/// The value rendered into an input — units use their canonical source form.
fn node_display(value: &NodeValue) -> String {
    match value {
        NodeValue::Str(s) => s.clone(),
        NodeValue::Int(i) => i.to_string(),
        NodeValue::Float(f) => f.to_string(),
        NodeValue::Bool(b) => b.to_string(),
        NodeValue::Unit(u) => u.to_source_string(),
        _ => String::new(),
    }
}

/// `max_feed_rate` → `Max feed rate`, used when a schema field has no `title`.
fn titleize(name: &str) -> String {
    let mut spaced = name.replace('_', " ");
    if let Some(first) = spaced.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    spaced
}

/// A schema-driven form field for a profile document: renders the right widget
/// for the node at `ptr`, with label/help/validation from its schema `Meta`, and
/// writes edits straight back to the store. Replaces hand-written
/// label+read+parse+persist blocks in the profile screens.
#[component]
pub fn SchemaField(id: Uuid, ptr: String) -> Element {
    field_widget(FieldAddr::Doc(id), ptr)
}

/// The stock-singleton twin of [`SchemaField`], editing the identity-less
/// `stock.yaml` document at `ptr` (e.g. `/tools/{i}/base/diameter`).
#[component]
pub fn StockField(ptr: String) -> Element {
    field_widget(FieldAddr::Stock, ptr)
}

/// Shared field-widget body behind [`SchemaField`] and [`StockField`]: reads the
/// node at `ptr` under `addr` and renders the widget matching its kind, writing
/// edits back through the address-dispatched setters. Called exactly once per
/// component instance, so its two `use_signal` hooks keep a stable order.
fn field_widget(addr: FieldAddr, ptr: String) -> Element {
    // Local editing state for text/number fields: a buffer edited in place and
    // committed on Enter/blur, reverted on Escape. Declared before the early
    // return so the hook order stays stable.
    let mut editing = use_signal(|| false);
    let mut buffer = use_signal(String::new);

    subscribe();
    let Some(field) = addr_field(addr, &ptr) else {
        return rsx! {};
    };

    let field_class = if field.incomplete {
        "field field-invalid"
    } else {
        "field"
    };
    let sys = system_unit();
    let unit = if let NodeValue::Unit(value) = &field.value {
        Some(*value)
    } else {
        None
    };

    let input = match &field.kind {
        // Fixed value set → dropdown (commits immediately).
        FieldKind::Enum(_) => {
            let options = field.enum_options.clone();
            let current = field.display.clone();
            let ptr = ptr.clone();
            rsx! {
                select {
                    onchange: move |evt| addr_set_input(addr, &ptr, &evt.value()),
                    for opt in options {
                        option { value: "{opt}", selected: current == opt, "{opt}" }
                    }
                }
            }
        }
        // Boolean → checkbox (commits immediately).
        _ if matches!(field.value, NodeValue::Bool(_)) => {
            let checked = matches!(field.value, NodeValue::Bool(true));
            let ptr = ptr.clone();
            rsx! {
                input {
                    r#type: "checkbox",
                    checked,
                    onchange: move |evt| addr_set_value(addr, &ptr, NodeValue::Bool(evt.checked())),
                }
            }
        }
        // Multi-line string (e.g. a G-code primitive) → textarea (live commit,
        // since Enter must insert a newline rather than commit).
        _ if matches!(&field.value, NodeValue::Str(s) if s.contains('\n')) => {
            let value = field.display.clone();
            let ptr = ptr.clone();
            let rows = field.display.lines().count().clamp(2, 16);
            rsx! {
                textarea {
                    class: "gcode-editor cnc-template-editor",
                    rows: "{rows}",
                    value: "{value}",
                    oninput: move |evt| addr_set_input(addr, &ptr, &evt.value()),
                }
            }
        }
        // Unit / number / string → buffered text edit. Units are displayed via
        // the shared unit_service: converted to the active system unit, with the
        // native value shown in `[...]` when they differ. Editing seeds the
        // native value — stripped of its unit when it already matches the system
        // unit, kept with its unit otherwise (to avoid confusion). Enter/blur
        // commits, Escape reverts.
        _ => {
            let is_number = matches!(field.value, NodeValue::Int(_) | NodeValue::Float(_));
            let display = if *editing.read() {
                buffer.read().clone()
            } else if let Some(value) = unit {
                unit_display(&value, sys)
            } else {
                field.display.clone()
            };
            let edit_seed = match unit {
                Some(value) => unit_edit_display(&value, sys),
                None => field.display.clone(),
            };
            let (ptr_commit, ptr_blur) = (ptr.clone(), ptr.clone());
            rsx! {
                input {
                    r#type: if is_number { "number" } else { "text" },
                    value: "{display}",
                    onfocusin: move |_| {
                        buffer.set(edit_seed.clone());
                        editing.set(true);
                    },
                    oninput: move |evt| buffer.set(evt.value()),
                    onkeydown: move |evt| {
                        let key = evt.key().to_string().to_ascii_lowercase();
                        if key == "enter" || key == "numpadenter" {
                            let buf = buffer.read().clone();
                            commit_value(addr, &ptr_commit, unit, &buf, sys);
                            editing.set(false);
                        } else if key == "escape" || key == "esc" {
                            editing.set(false);
                        }
                    },
                    onfocusout: move |_| {
                        if *editing.read() {
                            let buf = buffer.read().clone();
                            commit_value(addr, &ptr_blur, unit, &buf, sys);
                            editing.set(false);
                        }
                    },
                }
            }
        }
    };

    rsx! {
        div { class: "{field_class}",
            label {
                "{field.label}"
                if field.required {
                    span { class: "field-required", " *" }
                }
            }
            {input}
            if let Some(desc) = field.description.clone() {
                p { class: "field-hint", "{desc}" }
            }
        }
    }
}

/// The active display unit system. Read live from the legacy context during the
/// migration (the settings screen is not on the datastore yet); the unit toggle
/// bumps the render counter (see `dispatch_ui_command`) so fields reconvert.
fn system_unit() -> UnitSystem {
    crate::app_state_impl::with_ctx(|ctx| ctx.app.unit_system)
}

/// Display text for a typed unit value: converted to `sys`, with the native
/// value shown in `[...]` when it differs (via the shared unit_service).
fn unit_display(value: &UnitValue, sys: UnitSystem) -> String {
    match value {
        UnitValue::Length(length) => unit_service::format_length_display(*length, sys),
        UnitValue::Feed(feed) => unit_service::format_feed_display(*feed, sys),
        UnitValue::Angle(angle) => unit_service::format_angle_display(*angle),
        UnitValue::Rpm(speed) => unit_service::format_rotational_speed_display(*speed),
    }
}

/// The value seeded into the editor: the native value with its unit stripped
/// when it already matches `sys`, kept with its unit otherwise.
fn unit_edit_display(value: &UnitValue, sys: UnitSystem) -> String {
    match value {
        UnitValue::Length(length) => unit_service::format_length_edit_display(*length, sys),
        UnitValue::Feed(feed) => unit_service::format_feed_edit_display(*feed, sys),
        UnitValue::Angle(angle) => unit_service::format_angle_edit_display(*angle),
        UnitValue::Rpm(speed) => unit_service::format_rotational_speed_edit_display(*speed),
    }
}

/// Commits an edited buffer. Typed unit fields are parsed with a system-unit
/// preference (so a bare number is read in the system unit, an explicit unit
/// overrides) and stored as a typed value; other fields decode by schema type.
/// A value that fails to parse is left unchanged (the next render reverts it).
fn commit_value(addr: FieldAddr, ptr: &str, unit: Option<UnitValue>, edited: &str, sys: UnitSystem) {
    let Some(value) = unit else {
        addr_set_input(addr, ptr, edited);
        return;
    };
    let parsed = match value {
        UnitValue::Length(_) => {
            unit_service::parse_length_with_preference(edited, sys).map(UnitValue::Length)
        }
        UnitValue::Feed(_) => {
            unit_service::parse_feed_with_preference(edited, sys).map(UnitValue::Feed)
        }
        UnitValue::Angle(_) => unit_service::parse_angle(edited).map(UnitValue::Angle),
        UnitValue::Rpm(_) => unit_service::parse_rotational_speed(edited).map(UnitValue::Rpm),
    };
    if let Ok(unit_value) = parsed {
        addr_set_value(addr, ptr, NodeValue::Unit(unit_value));
    }
}

/// Creates a new profile of `kind` with the given name; returns its id.
pub fn create_named(kind: crate::data::Profile, name: &str) -> Option<Uuid> {
    let id = with_appdata_mut(|data| data.create(kind).ok())?;
    with_appdata_mut(|data| data.set_field(id, "/name", NodeValue::Str(name.to_string())));
    bump_render();
    Some(id)
}

/// Clones the profile `id` under a new name; returns the new id.
pub fn clone_named(id: Uuid, name: &str) -> Option<Uuid> {
    let new_id = with_appdata_mut(|data| data.clone(id).ok())?;
    with_appdata_mut(|data| data.set_field(new_id, "/name", NodeValue::Str(name.to_string())));
    bump_render();
    Some(new_id)
}

/// Serializes the profile `id` to YAML for export/download.
pub fn export_yaml(id: Uuid) -> Option<String> {
    with_appdata(|data| data.document_yaml(id))
}

/// Imports a profile of `kind` from YAML text (assigning a fresh id); returns
/// the new id, or `None` if the text is not a valid profile.
pub fn import_yaml(kind: crate::data::Profile, text: &str) -> Option<Uuid> {
    let yaml: serde_yaml::Value = serde_yaml::from_str(text).ok()?;
    let value: Value = serde_json::to_value(yaml).ok()?;
    let id = with_appdata_mut(|data| data.create_from_value(kind, &value).ok())?;
    bump_render();
    Some(id)
}

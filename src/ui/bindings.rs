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
use crate::data::model::{
    conflicting_operations, operation_once_per_face, step_reference, MachiningOperation,
    OperationConflict, UserUnitSystem, MACHINING_OPERATIONS,
};
use units::user_format as unit_format;
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
    /// The values this field may hold, each with the label to show for it. Empty unless
    /// the field is an enum.
    pub enum_options: Vec<datastore::EnumVariant>,
    pub default_applied: bool,
    pub incomplete: bool,
}

/// Where a bound field lives: a profile document addressed by root identity, or
/// one of the identity-less **singletons** addressed by its file. Lets one field
/// widget and one form renderer serve the id-based profile screens, the stock
/// screen and the settings dialog without duplicating the widget logic.
///
/// `Settings` earns its place for the same reason `Stock` did. The settings dialog is
/// otherwise hand-written toggles, which suits a boolean and suits nothing else: a length
/// needs a label, help text, unit parsing in whatever system the operator works in, and a
/// schema range to reject nonsense against. All four already exist in `settings.yaml` and
/// all four would have to be written again by hand to put a field there any other way.
#[derive(Clone, Copy, PartialEq)]
pub enum FieldAddr {
    Doc(Uuid),
    Stock,
    Settings,
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
            FieldAddr::Settings => data.settings()?,
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
            FieldAddr::Settings => data.settings(),
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
        FieldAddr::Settings => data.set_setting_str(ptr, raw),
    });
    bump_render();
}

/// Sets a typed value directly under `addr` (checkbox/enum/unit), bumping the tick.
fn addr_set_value(addr: FieldAddr, ptr: &str, value: NodeValue) {
    with_appdata_mut(|data| match addr {
        FieldAddr::Doc(id) => data.set_field(id, ptr, value),
        FieldAddr::Stock => data.set_stock_value(ptr, value),
        FieldAddr::Settings => data.set_setting(ptr, value),
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

/// Reverts every edited field of the stock tool at `index` back to its immutable
/// `base` (catalog) values by replacing its `overrides` with a copy of `base`.
pub fn revert_stock_tool(index: usize) {
    with_appdata_mut(|data| {
        let Some(mut value) = data.stock().map(|doc| doc.to_value()) else {
            return;
        };
        let Some(tool) = value
            .get_mut("tools")
            .and_then(|tools| tools.get_mut(index))
            .and_then(Value::as_object_mut)
        else {
            return;
        };
        if let Some(base) = tool.get("base").cloned() {
            tool.insert("overrides".to_string(), base);
            data.replace_stock_from_value(&value);
        }
    });
    bump_render();
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

/// The machining profile the live job references, if any (subscribes to store
/// mutations).
///
/// The job's reference is the operator's own answer to "which profile am I working on",
/// so the machining screen opens on it rather than on whichever profile happens to sort
/// first. Not validated here — the caller has the profile list in hand and filters
/// against it, because a job left pointing at a deleted profile must fall back to a real
/// one rather than select nothing.
pub fn use_job_machining_profile() -> Option<Uuid> {
    subscribe();
    if !appdata_ready() {
        return None;
    }
    with_appdata(|data| data.job_machining_profile())
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
/// Rebuilds **every** legacy projection from AppData, in one context mutation.
///
/// The single bridge between the two reactive systems this application runs. Writes go
/// to AppData and bump [`RENDER_TICK`]; the Job views read the legacy `Signal<AppCtx>`,
/// which knows nothing about that tick. This is what carries one to the other, and it is
/// mounted once at the root ([`crate::ui::screens::AppRoot`]) rather than per screen.
///
/// It used to be six copies of the same effect, one in each editor screen, each
/// refreshing only its own realm. That made a view's freshness depend on which screen the
/// operator happened to be standing on: with the Job view docked beside the CNC screen,
/// an edit refreshed `machines` and left `toolsets` as they were, and anything reading
/// both — the Stock screen's ATC column resolves a rack from *both* lists — silently saw
/// half a configuration. Refreshing every realm together costs one projection pass per
/// store write and removes the question.
///
/// One [`crate::runtime::with_ctx_mut`] for all five, so `sync_after_mutation` — and with
/// it the regeneration trigger — runs once per store write rather than five times.
/// Bindings first, machining last: a machining profile is read against the machines,
/// fixtures, toolsets and tools it references.
pub fn refresh_legacy_projections() {
    if !appdata_ready() {
        return;
    }
    let (machines, fixtures, toolsets, machining, stock) = with_appdata(|data| {
        let list = |profile| {
            data.list(profile)
                .into_iter()
                .map(|(_, doc)| doc.to_value())
                .collect::<Vec<Value>>()
        };
        (
            list(crate::data::Profile::Cnc),
            list(crate::data::Profile::Fixture),
            list(crate::data::Profile::Toolset),
            list(crate::data::Profile::Machining),
            data.stock().map(|doc| doc.to_value()),
        )
    });
    crate::runtime::with_ctx_mut(|ctx| {
        ctx.refresh_machines(&machines);
        ctx.refresh_fixtures(&fixtures);
        ctx.refresh_toolsets(&toolsets);
        if let Some(stock) = &stock {
            ctx.refresh_tools(stock);
        }
        ctx.refresh_process_profiles(&machining);
    });
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

/// The machining operations, in schema order — display order, persisted order, and
/// the order per-operation configuration sections are laid out in.
///
/// The table itself lives in [`crate::data::model::operations`] rather than here: the
/// readiness gate needs the same list to refuse a profile that claims one operation
/// twice, and a second copy under `ui` is how the two would come to disagree.
pub fn machining_operations() -> &'static [MachiningOperation] {
    MACHINING_OPERATIONS
}

/// Whether the field at `ptr` is relevant given its siblings, per the schema's
/// `x-show-when` (see [`crate::ui::show_when`]). Fields with no declaration are always
/// relevant, which is nearly all of them.
fn is_relevant(id: Uuid, ptr: &str) -> bool {
    let Some(condition) = crate::ui::show_when::show_when(ptr) else {
        return true;
    };
    let Some((parent, _)) = ptr.rsplit_once('/') else {
        return true;
    };
    let sibling = with_appdata(|data| {
        data.get(id)
            .and_then(|doc| doc.root.get_pointer(&format!("{parent}/{}", condition.sibling)))
            .map(|node| node_to_json(&node.value))
    });
    condition.matches(sibling.as_ref())
}

/// A node value as plain JSON, for comparing against a schema-declared condition.
fn node_to_json(value: &NodeValue) -> serde_json::Value {
    match value {
        NodeValue::Str(s) => serde_json::Value::from(s.clone()),
        NodeValue::Bool(b) => serde_json::Value::from(*b),
        NodeValue::Int(i) => serde_json::Value::from(*i),
        NodeValue::Float(f) => serde_json::Value::from(*f),
        other => serde_json::Value::from(node_display(other)),
    }
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
    // A field the schema says is irrelevant right now (`x-show-when`) is not rendered —
    // a scored board offers no tab count, a routed one no V-groove depth. Display only:
    // the value stays in the document, so nothing is lost by toggling back.
    if !is_relevant(id, &ptr) {
        return rsx! {};
    }
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

/// Extracts a UUID from a reference/id/string node value.
fn ref_uuid(value: &NodeValue) -> Option<Uuid> {
    match value {
        NodeValue::Ref(reference) => Some(reference.raw),
        NodeValue::Id(id) => Some(*id),
        NodeValue::Str(s) => Uuid::parse_str(s).ok(),
        _ => None,
    }
}

/// Reads the profile `field` (`"cnc"`/`"fixture"`/`"toolset"`) on `step` of document
/// `id`. `None` is a real answer, not a failure: a step with no profile chosen.
fn read_binding_inner(id: Uuid, step: usize, field: &str) -> Option<Uuid> {
    if !appdata_ready() {
        return None;
    }
    with_appdata(|data| {
        data.get(id)?
            .root
            .get_pointer(&format!("/steps/{step}/{field}"))
            .and_then(|node| ref_uuid(&node.value))
    })
}

/// Reads a step's profile reference, subscribing the caller to store mutations.
pub fn use_binding(id: Uuid, step: usize, field: &str) -> Option<Uuid> {
    subscribe();
    read_binding_inner(id, step, field)
}

/// Writes a step's profile reference (`None` clears it) and triggers re-render.
pub fn set_binding(id: Uuid, step: usize, field: &str, target: Option<Uuid>) {
    with_appdata_mut(|data| data.set_step_reference(id, step, field, target));
    bump_render();
}

/// Reads the enabled `operations` of `step` on document `id`.
fn read_operations_inner(id: Uuid, step: usize) -> Vec<String> {
    if !appdata_ready() {
        return Vec::new();
    }
    with_appdata(|data| {
        data.get(id)
            .and_then(|doc| doc.root.get_pointer(&format!("/steps/{step}/operations")))
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

/// Reads a step's enabled operations, subscribing the caller to store mutations.
pub fn use_operations(id: Uuid, step: usize) -> Vec<String> {
    subscribe();
    read_operations_inner(id, step)
}

/// Reads a step's enabled operations without subscribing — for event handlers.
pub fn read_operations(id: Uuid, step: usize) -> Vec<String> {
    read_operations_inner(id, step)
}

/// Writes a step's enabled `operations` and triggers re-render.
pub fn set_operations(id: Uuid, step: usize, operations: &[String]) {
    with_appdata_mut(|data| data.set_step_operations(id, step, operations));
    bump_render();
}

/// Sets a step's board face to the front, and re-renders.
///
/// The one face a step can be *put* onto rather than asked about: a step that drills
/// locating pins machines the front by definition, since the pins are what the board is
/// later turned over against.
pub fn set_step_face_front(id: Uuid, step: usize) {
    with_appdata_mut(|data| data.set_str(id, &format!("/steps/{step}/board_face"), "front"));
    bump_render();
}

/// The number of steps in machining profile `id` (subscribes to store mutations).
pub fn use_step_count(id: Uuid) -> usize {
    subscribe();
    if !appdata_ready() {
        return 0;
    }
    with_appdata(|data| {
        data.get(id)
            .and_then(|doc| doc.root.get_pointer("/steps"))
            .map(|node| match &node.value {
                NodeValue::Array(items) => items.len(),
                _ => 0,
            })
            .unwrap_or(0)
    })
}

/// Appends a fresh machining step and triggers re-render.
pub fn add_step(id: Uuid) {
    with_appdata_mut(|data| data.add_step(id));
    bump_render();
}

/// Removes the step at `step` (a profile keeps at least one) and re-renders.
pub fn remove_step(id: Uuid, step: usize) {
    with_appdata_mut(|data| data.remove_step(id, step));
    bump_render();
}

/// Reorders a step from `from` to `to` and triggers re-render.
pub fn move_step(id: Uuid, from: usize, to: usize) {
    with_appdata_mut(|data| data.move_step(id, from, to));
    bump_render();
}

/// One step of `id` as the uniqueness rule sees it: what it is called, which side it
/// machines, and what it claims to do.
///
/// Read straight from the document rather than from the `JobProfile` projection,
/// which carries `steps[0]` only — a clash between steps 2 and 3 is exactly what that
/// projection cannot see.
fn step_operation_claims(id: Uuid) -> Vec<(String, bool, Vec<String>)> {
    if !appdata_ready() {
        return Vec::new();
    }
    with_appdata(|data| {
        let Some(doc) = data.get(id) else {
            return Vec::new();
        };
        let count = match doc.root.get_pointer("/steps").map(|node| &node.value) {
            Some(NodeValue::Array(items)) => items.len(),
            _ => 0,
        };
        (0..count)
            .map(|i| {
                let name = match doc
                    .root
                    .get_pointer(&format!("/steps/{i}/name"))
                    .map(|node| &node.value)
                {
                    Some(NodeValue::Str(name)) if !name.trim().is_empty() => name.clone(),
                    // An unnamed step still has to be nameable in a message, and its
                    // ordinal is what the editor shows beside it.
                    _ => format!("Step {}", i + 1),
                };
                let back = matches!(
                    doc.root
                        .get_pointer(&format!("/steps/{i}/board_face"))
                        .map(|node| &node.value),
                    Some(NodeValue::Str(face)) if face == "back"
                );
                let operations = match doc
                    .root
                    .get_pointer(&format!("/steps/{i}/operations"))
                    .map(|node| &node.value)
                {
                    Some(NodeValue::Array(ops)) => ops
                        .iter()
                        .filter_map(|op| match &op.value {
                            NodeValue::Str(key) => Some(key.clone()),
                            _ => None,
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                (name, back, operations)
            })
            .collect()
    })
}

/// Operations claimed by more than one step of `id` on the same board side.
///
/// The editor blocks the click that would create one, so this should only ever fire
/// for a profile that arrived some other way — hand-edited, imported, or written by a
/// build with different rules. Subscribes to store mutations.
pub fn use_conflicting_operations(id: Uuid) -> Vec<OperationConflict> {
    subscribe();
    let claims = step_operation_claims(id);
    conflicting_operations(
        claims
            .iter()
            .map(|(name, bottom, ops)| (name.as_str(), *bottom, ops.as_slice())),
    )
}

/// The step already running `key` on the same side as `step`, as `(index, name)`.
///
/// Drives the disabled checkbox: the editor names the owner rather than merely refusing,
/// because "you cannot tick this" without saying why is indistinguishable from a bug.
/// `None` for a repeatable operation, for `step` itself, and for the other side.
fn operation_owner(id: Uuid, step: usize, key: &str) -> Option<(usize, String)> {
    if !operation_once_per_face(key) {
        return None;
    }
    let claims = step_operation_claims(id);
    let side = claims.get(step)?.1;
    claims
        .iter()
        .enumerate()
        .find(|(index, (_, bottom, ops))| {
            *index != step && *bottom == side && ops.iter().any(|op| op == key)
        })
        .map(|(index, (name, _, _))| (index, name.clone()))
}

/// The `<option>` value standing for "no profile chosen".
///
/// A sentinel rather than an empty string so it cannot be confused with a profile whose
/// id failed to render; the handler maps it straight back to `None`.
const NO_PROFILE: &str = "__none__";

/// The profile picker for a machining step's `field` (`"cnc"`/`"fixture"`/`"toolset"`):
/// a single dropdown over the available profiles of `kind`, plus a "None" entry.
///
/// One reference, not a set. A step is one physical setup, so a second machine or fixture
/// for it is a second step — see the note in `schemas/machining.yaml`. Choosing None is
/// deliberately allowed and deliberately blocking: it leaves the step unrunnable and the
/// planner says which binding is missing, which is better than generating a program for a
/// machine the operator does not have.
#[component]
pub fn BindingPicker(id: Uuid, step: usize, field: String, kind: crate::data::Profile, label: String) -> Element {
    let selected = use_binding(id, step, &field);
    let options = use_profiles(kind);
    let on_field = field.clone();

    rsx! {
        div { class: "field binding-picker",
            label { "{label}" }
            if options.is_empty() {
                p { class: "field-hint", "No profiles available yet — create one first." }
            } else {
                select {
                    onchange: move |evt| {
                        let value = evt.value();
                        let target = if value == NO_PROFILE { None } else { Uuid::parse_str(&value).ok() };
                        set_binding(id, step, &on_field, target);
                    },
                    // Dioxus does not reflect a <select>'s `value:` on first render, so
                    // each option carries an explicit `selected:`. Without it a saved
                    // profile reads back as "None" until something else forces a
                    // re-render — and then appears to set itself.
                    option { value: NO_PROFILE, selected: selected.is_none(), "— None —" }
                    for (pid , name) in options {
                        option { value: "{pid}", selected: selected == Some(pid), "{name}" }
                    }
                }
                if selected.is_none() {
                    p { class: "field-hint field-hint-warn",
                        "No profile selected — this step cannot be machined."
                    }
                }
            }
        }
    }
}

/// The machining operations toggle set for a step: enables/disables each
/// operation, keeping the stored `operations` array in schema order.
#[component]
pub fn OperationsEditor(id: Uuid, step: usize) -> Element {
    let enabled = use_operations(id, step);
    rsx! {
        div { class: "field operations-editor",
            label { "Operations" }
            for op in MACHINING_OPERATIONS.iter() {
                {
                    let checked = enabled.iter().any(|enabled_key| enabled_key == op.key);
                    // Only an *unticked* box is ever blocked. If two steps somehow both
                    // hold the operation — a hand-edited profile — both stay tickable,
                    // because otherwise neither could be unticked and the conflict would
                    // be unfixable from the editor that reported it.
                    let owner = (!checked)
                        .then(|| operation_owner(id, step, op.key))
                        .flatten();
                    rsx! {
                        OperationToggle {
                            id,
                            step,
                            op_key: op.key.to_string(),
                            label: op.label.to_string(),
                            checked,
                            blocked_by: owner,
                        }
                    }
                }
            }
        }
    }
}

/// One operation checkbox within an [`OperationsEditor`].
///
/// `blocked_by` is the `(index, name)` of the step that already runs this operation on
/// the same board side, which makes the box unavailable — the board has the feature
/// once, so cutting it in a second step cuts it twice.
#[component]
fn OperationToggle(
    id: Uuid,
    step: usize,
    op_key: String,
    label: String,
    checked: bool,
    blocked_by: Option<(usize, String)>,
) -> Element {
    let title = match blocked_by.as_ref() {
        Some((index, name)) => format!(
            "Already run by {}, which machines the same side",
            step_reference(*index, name)
        ),
        None => String::new(),
    };
    rsx! {
        label {
            class: if blocked_by.is_some() { "checkbox-line is-blocked" } else { "checkbox-line" },
            title: "{title}",
            input {
                r#type: "checkbox",
                checked,
                disabled: blocked_by.is_some(),
                onchange: move |evt| {
                    let mut current = read_operations(id, step);
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
                        .filter(|op| current.iter().any(|enabled_key| enabled_key == op.key))
                        .map(|op| op.key.to_string())
                        .collect();
                    set_operations(id, step, &ordered);
                    // Enabling locating pins settles the step's board face: pins are what
                    // lets the board be turned over, so they are drilled before it ever is.
                    // Written rather than merely hidden, because the *document* is what the
                    // planner and the readiness gate read — a step left saying "back" with
                    // no control to change it would be unfixable from the editor.
                    if evt.checked() && op_key == "drill_locating_pins" {
                        set_step_face_front(id, step);
                    }
                },
            }
            span { "{label}" }
            // The ordinal alone beside the box; the full reference is in the tooltip.
            // Steps often share a name, so the number is the part that identifies one.
            if let Some((index, _)) = blocked_by.as_ref() {
                span { class: "operation-blocked-note", "in step {index + 1}" }
            }
        }
    }
}

/// Rebuilds the legacy in-memory `tools` (stock inventory) from the AppData-owned
/// stock singleton. Does not persist.
///
/// The one per-realm refresh that survives [`refresh_legacy_projections`], because the
/// structural stock operations below need the projection current *before* they return —
/// they report what they did from it. Everything else is the root's business.
pub fn refresh_legacy_stock() {
    let value = with_appdata(|data| data.stock().map(|doc| doc.to_value()));
    if let Some(value) = value {
        crate::runtime::with_ctx_mut(|ctx| ctx.refresh_tools(&value));
    }
}

/// What an add-from-catalog actually did.
///
/// `skipped` exists because "added 0" is not an explanation. Selecting five tools you
/// already own reported exactly that and left the operator to guess whether the button
/// was broken, the selection was lost, or the tools were already there.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StockAddition {
    pub added: usize,
    /// Already in stock, so not added again. Always 0 for a forced single add.
    pub skipped: usize,
}

/// Adds the catalog-picker selection to stock: builds the additions from the
/// legacy catalog (skipping what is already owned), projects them to stock-item
/// values, and appends them to the AppData document (the sole writer). Refreshes the
/// legacy projection.
pub fn add_stock_from_catalog(selected_keys: &[String]) -> StockAddition {
    add_catalog_tools(selected_keys, false)
}

/// Adds one catalog tool to stock **even if it is already there**, and reports the name
/// it landed under.
///
/// The deliberate counterpart to the bulk picker: naming one tool and pressing Add is a
/// specific request, and a second copy of a tool is a real thing to own. The copy is
/// named apart (`… (2)`) by `build_catalog_tool_additions` so the two can be told apart
/// in the rack picker and in the tooling plan. Returns `None` when the key resolves to
/// no catalog tool.
pub fn add_catalog_tool_to_stock(tool_key: &str) -> Option<String> {
    let keys = [tool_key.to_string()];
    let new_tools = crate::runtime::with_ctx(|ctx| ctx.build_catalog_tool_additions(&keys, true));
    let name = new_tools.first()?.composite_name.clone();
    append_stock_tools(&new_tools);
    Some(name)
}

/// Whether this catalog tool is already in stock — the question the single-tool add
/// warns about, answered by the same predicate the bulk picker skips on.
pub fn catalog_tool_already_in_stock(tool_key: &str) -> bool {
    crate::runtime::with_ctx(|ctx| {
        ctx.catalogs
            .iter()
            .flat_map(|catalog| catalog.sections.iter())
            .flat_map(|section| section.tools.iter())
            .find(|tool| tool.key == tool_key)
            .is_some_and(|tool| ctx.catalog_tool_in_stock(tool, &[]))
    })
}

fn add_catalog_tools(selected_keys: &[String], allow_duplicates: bool) -> StockAddition {
    let new_tools = crate::runtime::with_ctx(|ctx| {
        ctx.build_catalog_tool_additions(selected_keys, allow_duplicates)
    });
    // Everything the caller asked for that the builder did not return was already in
    // stock; it resolves keys itself, so this is the only place the two counts meet.
    let skipped = selected_keys.len().saturating_sub(new_tools.len());
    StockAddition { added: append_stock_tools(&new_tools), skipped }
}

/// Projects built tools to stock-item values and appends them to the AppData document.
fn append_stock_tools(new_tools: &[crate::data::model::Tool]) -> usize {
    if new_tools.is_empty() {
        bump_render();
        return 0;
    }
    let projected = crate::data::model::stock::stock_value_from_tools(new_tools);
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
        crate::runtime::with_ctx_mut(|ctx| ctx.validate_current_job_references());
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

/// Sets the preference enum of the stock tool at array `index` (the table's inline
/// preference selector), decoding against the schema, then refreshes the projection.
///
/// Takes the schema's own key rather than a bool or an enum: preference has three
/// states, and `availability` already spells itself three ways between storage, the DOM
/// and the label — a fourth vocabulary here would earn nothing.
pub fn set_stock_preference(index: usize, raw: &str) {
    with_appdata_mut(|data| {
        data.set_stock_str(&format!("/tools/{index}/preference"), raw);
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
        div { class: "field rack-slot-list",
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

    // State-coloured row: assigned (fixed) / spare / do-not-use.
    let state_class = if mode == "do_not_use" {
        "rack-slot-row rack-slot-donotuse"
    } else if mode == "fixed" {
        "rack-slot-row rack-slot-fixed"
    } else {
        "rack-slot-row rack-slot-spare"
    };

    rsx! {
        div { class: "{state_class}",
            span { class: "rack-slot-label", "T{index}" }
            select {
                value: "{selected_value}",
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
                // Dioxus does not reflect a <select>'s `value:` on first render, so each
                // option carries an explicit `selected:`. Without it a persisted `fixed`
                // (or `do_not_use`) slot falls back to the first option ("Spare") — the
                // display-only bug that made saved fixed slots look unsaved. (Same fix as
                // the machining-profile picker.)
                option { value: "spare", selected: selected_value == "spare", "Spare" }
                option { value: "do_not_use", selected: selected_value == "do_not_use", "Do not use" }
                if tool_missing {
                    option { value: "{missing_value}", selected: selected_value == missing_value, "Missing tool" }
                }
                for (tid , name) in tools {
                    option { value: "tool:{tid}", selected: selected_value == format!("tool:{tid}"), "{name}" }
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

/// `spindle_rpm_min` → `Spindle rpm min`, used when a schema field has no `title`.
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

/// The settings-singleton twin of [`SchemaField`], editing `settings.yaml` at `ptr`
/// (e.g. `/engrave_penetration_min`).
///
/// The Settings dialog is hand-written toggles elsewhere, which is right for a boolean and
/// wrong for a measurement: the label, the help text, the unit the operator works in and
/// the values that are refusable all live in the schema already, and a hand-rolled input
/// would restate every one of them in a second place.
#[component]
pub fn SettingsField(ptr: String) -> Element {
    field_widget(FieldAddr::Settings, ptr)
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
    // Whether the primitive-template modal editor is open (primitive fields only).
    let mut modal_open = use_signal(|| false);

    subscribe();
    let Some(field) = addr_field(addr, &ptr) else {
        return rsx! {};
    };

    // Stock override fields carry a rollback affordance: when the current
    // (override) value differs from the immutable `base` original, an orange revert
    // control resets it. The base value is compared by, and reverted through, the
    // same schema-decoding display used for edits.
    let revert_display: Option<String> =
        if matches!(addr, FieldAddr::Stock) && ptr.contains("/overrides/") {
            let base_ptr = ptr.replacen("/overrides/", "/base/", 1);
            addr_field(FieldAddr::Stock, &base_ptr)
                .map(|base| base.display)
                .filter(|base_display| *base_display != field.display)
        } else {
            None
        };
    let field_class = if field.incomplete {
        "field field-invalid"
    } else if revert_display.is_some() {
        "field field-changed"
    } else {
        "field"
    };
    let sys = system_unit();
    let unit = if let NodeValue::Unit(value) = &field.value {
        Some(*value)
    } else {
        None
    };

    // CNC primitive templates are GTL scripts that routinely span several lines,
    // so they always edit in a textarea — even when the current value is a single
    // line. Other string fields stay single-line until they contain a newline.
    let is_primitive_template = ptr.contains("/primitives/");

    let input = match &field.kind {
        // Fixed value set → dropdown (commits immediately).
        FieldKind::Enum(_) => {
            let options = field.enum_options.clone();
            let current = field.display.clone();
            let ptr = ptr.clone();
            rsx! {
                select {
                    onchange: move |evt| addr_set_input(addr, &ptr, &evt.value()),
                    // The key is the value, the label is the text. What reaches the
                    // document is unchanged — this is the same `<select>` it always was,
                    // with the storage key no longer doubling as the words an operator
                    // reads. `allow_hybrid` is a file format, not a sentence.
                    for opt in options {
                        option {
                            key: "{opt.key}",
                            value: "{opt.key}",
                            selected: current == opt.key,
                            "{opt.label}"
                        }
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
        // CNC primitive template → a compact summary + an "Edit…" button that opens
        // the modal editor (GTL editor + the primitive's variable reference + a live
        // validate/preview). Editing inline made every keystroke commit and
        // re-trigger generation; the modal commits once, on Save.
        _ if is_primitive_template && matches!(&field.value, NodeValue::Str(_)) => {
            let primitive = crate::gcode::primitive_vars::primitive_name_from_pointer(&ptr)
                .unwrap_or("")
                .to_string();
            let text = field.display.clone();
            let is_empty = text.trim().is_empty();
            let ptr_modal = ptr.clone();
            rsx! {
                div { class: "primitive-field",
                    if is_empty {
                        code { class: "primitive-summary primitive-summary-empty", "(empty)" }
                    } else {
                        // Read-only review of the template: grows to fit up to five
                        // lines, then scrolls — so the primitive can be read in place
                        // without opening the editor.
                        pre { class: "primitive-summary", "{text}" }
                    }
                    button {
                        class: "btn btn-secondary primitive-edit-btn",
                        r#type: "button",
                        onclick: move |_| modal_open.set(true),
                        "Edit…"
                    }
                    if *modal_open.read() {
                        PrimitiveEditorModal {
                            addr,
                            ptr: ptr_modal.clone(),
                            primitive: primitive.clone(),
                            open: modal_open,
                        }
                    }
                }
            }
        }
        // Other multi-line string → textarea. Buffered exactly like the single-line
        // editor below: edits accumulate in `buffer` and commit only on blur — Enter
        // inserts a newline here, so it cannot commit — while Escape reverts.
        _ if matches!(&field.value, NodeValue::Str(s) if s.contains('\n')) => {
            let display = if *editing.read() {
                buffer.read().clone()
            } else {
                field.display.clone()
            };
            let seed = field.display.clone();
            let rows = display.lines().count().clamp(3, 16);
            let ptr_blur = ptr.clone();
            rsx! {
                textarea {
                    class: "gcode-editor cnc-template-editor",
                    rows: "{rows}",
                    value: "{display}",
                    onfocusin: move |_| {
                        buffer.set(seed.clone());
                        editing.set(true);
                    },
                    oninput: move |evt| buffer.set(evt.value()),
                    onkeydown: move |evt| {
                        let key = evt.key().to_string().to_ascii_lowercase();
                        if key == "escape" || key == "esc" {
                            // Revert: leaving edit mode falls the value back to the
                            // last committed `field.display`.
                            editing.set(false);
                        }
                    },
                    onfocusout: move |_| {
                        if *editing.read() {
                            let buf = buffer.read().clone();
                            addr_set_input(addr, &ptr_blur, &buf);
                            editing.set(false);
                        }
                    },
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

    // The editable control is wrapped so it can be width-capped (a field never
    // spans a whole wide column) with the inline revert affordance beside it.
    // Multiline editors (CNC GTL templates, wrapped strings) take the full width;
    // checkboxes keep their natural size.
    // A profile's own name is the exception to the width cap. It is free text the operator
    // writes rather than a value with a natural size, and the names people actually use —
    // "Genmitsu 3018 PRO, 300W spindle" — ran off the end of a box sized for a diameter.
    // Matched on the pointer because that is what makes it *the name*; every other string
    // field on the screen is still capped.
    let is_profile_name = ptr == "/name";
    let control_class = if is_profile_name {
        "field-control field-control-name"
    } else if matches!(&field.kind, FieldKind::Enum(_)) {
        "field-control"
    } else if matches!(&field.value, NodeValue::Bool(_)) {
        "field-control field-control-check"
    } else if matches!(&field.value, NodeValue::Str(text) if is_primitive_template || text.contains('\n')) {
        "field-control field-control-wide"
    } else {
        "field-control"
    };

    rsx! {
        div { class: "{field_class}",
            label {
                "{field.label}"
                if field.required {
                    span { class: "field-required", " *" }
                }
            }
            div { class: "{control_class}",
                {input}
                if let Some(base_display) = revert_display.clone() {
                    button {
                        class: "stock-revert-btn",
                        r#type: "button",
                        title: "Revert to catalog value",
                        onclick: {
                            let ptr = ptr.clone();
                            move |_| addr_set_input(FieldAddr::Stock, &ptr, &base_display)
                        },
                        "\u{21ba}"
                    }
                }
            }
            if let Some(desc) = field.description.clone() {
                p { class: "field-hint", "{desc}" }
            }
        }
    }
}

/// Modal editor for a CNC primitive's GTL template. Three panes: the GTL editor,
/// a **reference** of the variables in scope for this primitive (from the schema's
/// `x-variables` — see [`crate::gcode::primitive_vars`]), and a **live
/// validate/preview** that renders the template against representative sample
/// values. The preview surfaces both syntax errors and references to variables the
/// primitive does not declare (the `z_safe`-not-found class) *before* the template
/// is ever generated. Edits are local until **Save**; Cancel / Escape / clicking
/// the backdrop discard them.
#[component]
fn PrimitiveEditorModal(
    addr: FieldAddr,
    ptr: String,
    primitive: String,
    open: Signal<bool>,
) -> Element {
    let mut open = open;
    let current = addr_field(addr, &ptr).map(|field| field.display).unwrap_or_default();
    let mut buffer = use_signal(|| current.clone());

    let vars = crate::gcode::primitive_vars::variables_for(&primitive);
    let source = buffer.read().clone();
    // Live validate + preview against a sample scope holding only the declared
    // variables — rebuilt each keystroke so feedback is immediate.
    let preview = crate::gcode::coder::Coder::new().preview(&primitive, &source, &vars);

    let save = {
        let ptr = ptr.clone();
        move |_| {
            let value = buffer.read().clone();
            addr_set_input(addr, &ptr, &value);
            open.set(false);
        }
    };

    rsx! {
        div {
            // A dedicated fixed-position overlay (not the screen-rooted
            // `.wizard-overlay`): this modal renders deep inside a nested field, so
            // it must be viewport-anchored to escape any positioned ancestor.
            class: "primitive-modal-overlay",
            onclick: move |_| open.set(false),
            onkeydown: move |evt| {
                if evt.key().to_string().eq_ignore_ascii_case("escape") {
                    open.set(false);
                }
            },
            div {
                class: "catalog-picker-dialog primitive-modal",
                // Clicks inside the dialog must not fall through to the backdrop.
                onclick: move |evt| evt.stop_propagation(),
                div { class: "primitive-modal-head",
                    h2 { "Edit primitive" }
                    code { class: "primitive-modal-name", "{primitive}" }
                    // How this primitive is invoked. The single most useful thing to say
                    // here: a Callable does nothing at all until another template calls
                    // it, and nothing else on this screen would tell the author that.
                    if let Some(kind) = crate::gcode::primitive_vars::kind_of(&primitive) {
                        span {
                            class: match kind {
                                crate::gcode::primitive_vars::PrimitiveKind::Callable => "primitive-kind is-callable",
                                crate::gcode::primitive_vars::PrimitiveKind::Filter => "primitive-kind is-filter",
                                crate::gcode::primitive_vars::PrimitiveKind::Generator => "primitive-kind",
                            },
                            title: "{kind.hint()}",
                            "{kind.label()}"
                        }
                    }
                }
                if let Some(kind) = crate::gcode::primitive_vars::kind_of(&primitive) {
                    div { class: "primitive-modal-hint", "{kind.hint()}" }
                }
                // What leaving it blank does. Without this an author faced with an empty
                // `cut_arc` has no way to tell "not supported yet" from "your machine has
                // no word for this, and the application handles it" — and blank means
                // something different on every other primitive, where it emits nothing.
                if let Some(fallback) = crate::gcode::primitive_vars::fallback_for(&primitive) {
                    div { class: "primitive-modal-hint",
                        "Leave blank if this machine has no such word: the move is \
                         approximated with {fallback} instead, to the CNC's curve tolerance."
                    }
                }

                div { class: "primitive-modal-body",
                    div { class: "primitive-editor-pane",
                        label { class: "primitive-pane-label", "Template" }
                        textarea {
                            class: "gcode-editor primitive-modal-editor",
                            value: "{source}",
                            onmounted: move |evt| async move {
                                let _ = evt.set_focus(true).await;
                            },
                            oninput: move |evt| buffer.set(evt.value()),
                        }
                        match &preview {
                            Ok(rendered) => rsx! {
                                div { class: "primitive-preview ok",
                                    div { class: "primitive-preview-head", "Preview · sample values" }
                                    pre { class: "primitive-preview-body", "{rendered}" }
                                }
                            },
                            Err(err) => rsx! {
                                div { class: "primitive-preview err",
                                    div { class: "primitive-preview-head", "Invalid template" }
                                    div { class: "primitive-preview-error", "{err}" }
                                }
                            },
                        }
                    }

                    aside { class: "primitive-vars-pane",
                        div { class: "primitive-pane-label", "Variables in scope" }
                        if vars.is_empty() {
                            div { class: "primitive-vars-empty", "This primitive takes no variables." }
                        } else {
                            ul { class: "primitive-vars-list",
                                for var in vars.iter() {
                                    li { key: "{var.name}", class: "primitive-var",
                                        div { class: "primitive-var-head",
                                            code { class: "primitive-var-name", "{var.name}" }
                                            span { class: "primitive-var-type", "{var.var_type.label()}" }
                                        }
                                        div { class: "primitive-var-desc", "{var.description}" }
                                    }
                                }
                            }
                        }
                        div { class: "primitive-pane-label", "Always available" }
                        ul { class: "primitive-vars-list",
                            li { class: "primitive-var",
                                code { class: "primitive-var-name", "metric() / imperial()" }
                                div { class: "primitive-var-desc", "Set the output unit and emit the set_unit primitive." }
                            }
                            li { class: "primitive-var",
                                code { class: "primitive-var-name", "set_origin()" }
                                div { class: "primitive-var-desc",
                                    "Emit the validated work-origin selection for the step's fixture."
                                }
                            }
                            li { class: "primitive-var",
                                code { class: "primitive-var-name", "comment(t) · message(t) · pause(t)" }
                                div { class: "primitive-var-desc",
                                    "Emit the matching operator primitive with your text."
                                }
                            }
                            li { class: "primitive-var",
                                code { class: "primitive-var-name", "`…`  ·  {{expr}}" }
                                div { class: "primitive-var-desc",
                                    "Backtick lines emit GCode; {{expr}} is evaluated and unit-formatted."
                                }
                            }
                        }
                    }
                }

                div { class: "wizard-actions",
                    button {
                        class: "btn btn-secondary",
                        r#type: "button",
                        onclick: move |_| open.set(false),
                        "Cancel"
                    }
                    button {
                        class: "btn btn-primary",
                        r#type: "button",
                        onclick: save,
                        "Save"
                    }
                }
            }
        }
    }
}

/// The active display unit system. Read live from the legacy context during the
/// migration (the settings screen is not on the datastore yet); the unit toggle
/// bumps the render counter (see `dispatch_ui_command`) so fields reconvert.
fn system_unit() -> UserUnitSystem {
    crate::runtime::with_ctx(|ctx| ctx.app.unit_system)
}

/// Display text for a typed unit value: converted to `sys`, with the native
/// value shown in `[...]` when it differs (via the shared unit_service).
fn unit_display(value: &UnitValue, sys: UserUnitSystem) -> String {
    match value {
        UnitValue::Length(length) => unit_format::format_length_display(*length, sys),
        UnitValue::Feed(feed) => unit_format::format_feed_display(*feed, sys),
        UnitValue::Angle(angle) => unit_format::format_angle_display(*angle),
        UnitValue::Rpm(speed) => unit_format::format_rotational_speed_display(*speed),
    }
}

/// The value seeded into the editor: the native value with its unit stripped
/// when it already matches `sys`, kept with its unit otherwise.
fn unit_edit_display(value: &UnitValue, sys: UserUnitSystem) -> String {
    match value {
        UnitValue::Length(length) => unit_format::format_length_edit_display(*length, sys),
        UnitValue::Feed(feed) => unit_format::format_feed_edit_display(*feed, sys),
        UnitValue::Angle(angle) => unit_format::format_angle_edit_display(*angle),
        UnitValue::Rpm(speed) => unit_format::format_rotational_speed_edit_display(*speed),
    }
}

/// Commits an edited buffer. Typed unit fields are parsed with a system-unit
/// preference (so a bare number is read in the system unit, an explicit unit
/// overrides) and stored as a typed value; other fields decode by schema type.
/// A value that fails to parse is left unchanged (the next render reverts it).
fn commit_value(addr: FieldAddr, ptr: &str, unit: Option<UnitValue>, edited: &str, sys: UserUnitSystem) {
    let Some(value) = unit else {
        addr_set_input(addr, ptr, edited);
        return;
    };
    let parsed = match value {
        UnitValue::Length(_) => {
            unit_format::parse_length_with_preference(edited, sys).map(UnitValue::Length)
        }
        UnitValue::Feed(_) => {
            unit_format::parse_feed_with_preference(edited, sys).map(UnitValue::Feed)
        }
        UnitValue::Angle(_) => unit_format::parse_angle(edited).map(UnitValue::Angle),
        UnitValue::Rpm(_) => unit_format::parse_rotational_speed(edited).map(UnitValue::Rpm),
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

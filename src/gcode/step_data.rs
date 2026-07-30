//! One machining step's own record, in the form a program template sees it.
//!
//! `program_begin` and `program_end` are given the whole machining profile's `steps`
//! array plus the `step_index` of the program being written, so a header can say which
//! setup of which job it is — "step 2 of 3, Drill NPTH, on the MASSO" — rather than
//! being distinguishable only by file name.
//!
//! # Why a private tree and not just a Rhai value
//!
//! Two constraints meet here and only this shape satisfies both.
//!
//! [`GenerationInput`](crate::runtime::GenerationInput) is captured on the UI thread and
//! rendered on the generation worker, so everything it carries must be `Send`. `rhai` is
//! built here without its `sync` feature, which makes `Dynamic` deliberately *not* `Send`
//! — so the step tree cannot be built as a Rhai value where the datastore is readable.
//!
//! [`serde_json::Value`] is `Send` and the datastore already emits one
//! (`Node::to_value`), but it renders a `2.0mm` as the *string* `"2.0mm"`. That loses
//! precisely what makes a unit worth carrying: a template writing
//! `` `(finish {steps[step_index].route_board.finishing}) `` gets `0.1` after `metric()`
//! and `0.0039` after `imperial()` only while the value is still a [`Length`].
//!
//! So: a plain `Send` tree that keeps the four unit kinds typed, built where the
//! datastore is (`runtime::tooling::read_step_values`) and converted to Rhai on the
//! worker, at the point of use.

use gtl::rhai::{Array, Dynamic, Map};
use units::{Angle, FeedRate, Length, RotationalSpeed};

/// A value inside a machining step's record, mirroring `machining.yaml`'s `$defs/step`.
///
/// Deliberately a superset of nothing: it is whatever the document holds, so a field
/// added to the step schema reaches templates without a change here.
#[derive(Clone, Debug, PartialEq)]
pub enum StepValue {
    Null,
    Bool(bool),
    Int(i64),
    Number(f64),
    Text(String),
    Length(Length),
    Feed(FeedRate),
    Angle(Angle),
    Rpm(RotationalSpeed),
    List(Vec<StepValue>),
    /// Field order as the schema declares it, not sorted: a template that iterates a
    /// step sees its fields in the order the YAML reads, which is the order the editor
    /// shows them in.
    Map(Vec<(String, StepValue)>),
}

impl StepValue {
    /// Converts to the Rhai value a template indexes into.
    ///
    /// The four unit kinds become the custom types [`crate::gcode::dialect`] registers,
    /// so `fmt()`, the comparison operators and the `.mm`/`.inch`/`.rpm` accessors work
    /// on a step's fields exactly as they already do on `z_safe`.
    ///
    /// [`StepValue::Null`] becomes Rhai's unit `()`. That is not an oversight: a field
    /// the operator has not filled in has no value to print, and `fmt(())` is not
    /// registered, so interpolating one is a render error naming the line rather than a
    /// silent empty word in the middle of a G-code block.
    pub fn to_dynamic(&self) -> Dynamic {
        match self {
            Self::Null => Dynamic::UNIT,
            Self::Bool(v) => Dynamic::from(*v),
            Self::Int(v) => Dynamic::from(*v),
            Self::Number(v) => Dynamic::from(*v),
            Self::Text(v) => Dynamic::from(v.clone()),
            Self::Length(v) => Dynamic::from(*v),
            Self::Feed(v) => Dynamic::from(*v),
            Self::Angle(v) => Dynamic::from(*v),
            Self::Rpm(v) => Dynamic::from(*v),
            Self::List(items) => {
                Dynamic::from(items.iter().map(Self::to_dynamic).collect::<Array>())
            }
            Self::Map(fields) => {
                let mut map = Map::new();
                for (key, value) in fields {
                    map.insert(key.as_str().into(), value.to_dynamic());
                }
                Dynamic::from(map)
            }
        }
    }

    /// The value of a field of this map, if this is a map and it has one.
    pub fn field(&self, name: &str) -> Option<&StepValue> {
        match self {
            Self::Map(fields) => fields.iter().find(|(key, _)| key == name).map(|(_, v)| v),
            _ => None,
        }
    }

    /// This value as text, if it is text. For reading an id out of a step in order to
    /// look up what it names.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(value) => Some(value),
            _ => None,
        }
    }

    /// Sets (or replaces) a field on this map, keeping it a map. A no-op on anything
    /// else, because there is no sensible field to set on a scalar.
    pub fn set_field(&mut self, name: &str, value: StepValue) {
        let Self::Map(fields) = self else { return };
        match fields.iter_mut().find(|(key, _)| key == name) {
            Some(slot) => slot.1 = value,
            None => fields.push((name.to_string(), value)),
        }
    }
}

/// The whole `steps` array as the Rhai array a template indexes with `step_index`.
pub fn to_array(steps: &[StepValue]) -> Array {
    steps.iter().map(StepValue::to_dynamic).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> StepValue {
        StepValue::Map(vec![
            ("name".into(), StepValue::Text("Drill PTH".into())),
            (
                "operations".into(),
                StepValue::List(vec![StepValue::Text("drill_pth".into())]),
            ),
            ("finishing".into(), StepValue::Length(Length::from_mm(0.1))),
            ("cnc".into(), StepValue::Null),
        ])
    }

    #[test]
    fn map_keeps_declaration_order_and_types() {
        let dynamic = sample().to_dynamic();
        let map = dynamic.cast::<Map>();
        assert_eq!(map.get("name").unwrap().clone().cast::<String>(), "Drill PTH");
        assert_eq!(map.get("operations").unwrap().clone().cast::<Array>().len(), 1);
        // The unit survives as a unit, which is the whole reason this type exists.
        assert_eq!(map.get("finishing").unwrap().clone().cast::<Length>(), Length::from_mm(0.1));
        assert!(map.get("cnc").unwrap().is_unit());
    }

    #[test]
    fn set_field_appends_then_replaces() {
        let mut step = sample();
        step.set_field("cnc_name", StepValue::Text("MASSO".into()));
        assert_eq!(step.field("cnc_name").and_then(StepValue::as_text), Some("MASSO"));
        step.set_field("cnc_name", StepValue::Text("Genmitsu".into()));
        assert_eq!(step.field("cnc_name").and_then(StepValue::as_text), Some("Genmitsu"));
        // Replaced, not duplicated.
        let StepValue::Map(fields) = &step else { panic!("not a map") };
        assert_eq!(fields.iter().filter(|(key, _)| key == "cnc_name").count(), 1);
    }

    #[test]
    fn set_field_on_a_scalar_does_nothing() {
        let mut value = StepValue::Text("not a map".into());
        value.set_field("name", StepValue::Int(1));
        assert_eq!(value, StepValue::Text("not a map".into()));
    }
}

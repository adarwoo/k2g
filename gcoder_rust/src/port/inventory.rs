use std::collections::BTreeMap;

use super::model::{Feature, Hole, OblongHole};
use super::operations::Operations;

#[derive(Clone, Debug, Default)]
pub struct Inventory {
    pth: BTreeMap<i64, Vec<Feature>>,
    npth: BTreeMap<i64, Vec<Feature>>,
}

impl Inventory {
    pub fn add_round_hole(&mut self, hole: Hole) {
        self.push_by_plating(hole.diameter_nm, hole.plated, Feature::Hole(hole));
    }

    pub fn add_oblong_hole(&mut self, hole: OblongHole) {
        self.push_by_plating(hole.slot_width_nm, hole.plated, Feature::Oblong(hole));
    }

    pub fn add_route(&mut self, diameter_nm: i64, route: super::model::RouteSegment) {
        self.push_by_plating(diameter_nm, false, Feature::Route(route));
    }

    pub fn get_features(&self, ops: Operations) -> BTreeMap<i64, Vec<Feature>> {
        let mut out = BTreeMap::new();

        if ops.contains(Operations::PTH) {
            merge_features(&mut out, &self.pth);
        }

        if ops.contains(Operations::NPTH) {
            merge_features(&mut out, &self.npth);
        }

        out
    }

    fn push_by_plating(&mut self, diameter_nm: i64, plated: bool, feature: Feature) {
        let target = if plated {
            &mut self.pth
        } else {
            &mut self.npth
        };

        target.entry(diameter_nm).or_default().push(feature);
    }
}

fn merge_features(target: &mut BTreeMap<i64, Vec<Feature>>, source: &BTreeMap<i64, Vec<Feature>>) {
    for (diameter, features) in source {
        target
            .entry(*diameter)
            .or_default()
            .extend(features.iter().cloned());
    }
}

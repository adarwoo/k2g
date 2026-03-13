#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CoordinateNm {
    pub x_nm: i64,
    pub y_nm: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hole {
    pub center: CoordinateNm,
    pub diameter_nm: i64,
    pub plated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OblongHole {
    pub start: CoordinateNm,
    pub end: CoordinateNm,
    pub slot_width_nm: i64,
    pub plated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteSegment {
    pub start: CoordinateNm,
    pub end: CoordinateNm,
    pub diameter_nm: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Feature {
    Hole(Hole),
    Oblong(OblongHole),
    Route(RouteSegment),
}

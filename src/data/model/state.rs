/// Domain rack slot assignment independent from UI view concerns.
#[derive(Clone)]
pub struct RackSlot {
    pub tool_id: Option<String>,
    pub locked: bool,
    pub disabled: bool,
}

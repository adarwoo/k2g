#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Setup,
    Generator,
    Output,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SetupTab {
    Job,
    Machine,
    Tools,
    Rack,
    Output,
}

#[derive(Clone)]
pub struct MachineConfig {
    pub name: String,
    pub tool_change: String,
    pub max_size_x: f64,
    pub max_size_y: f64,
    pub max_speed_xy: f64,
    pub max_speed_z: f64,
    pub acceleration: f64,
}

#[derive(Clone)]
pub struct Tool {
    pub id: String,
    pub tool_type: String,
    pub diameter: f64,
    pub name: String,
    pub flutes: Option<u32>,
    pub status: String,
}

#[derive(Clone)]
pub struct RackSlot {
    pub slot_number: usize,
    pub tool_id: Option<String>,
    pub disabled: bool,
}

#[derive(Clone)]
pub struct WorkOrigin {
    pub x: f64,
    pub y: f64,
    pub coordinate_system: String,
}

#[derive(Clone)]
pub struct RackOptions {
    pub management: String,
    pub optimize_for_fewer_changes: bool,
    pub allow_mid_job_reload: bool,
}

#[derive(Clone)]
pub struct OperationTemplate {
    pub id: usize,
    pub name: String,
    pub template: String,
}

#[derive(Clone)]
pub struct OutputConfig {
    pub operations: Vec<OperationTemplate>,
}

#[derive(Clone)]
pub struct AppConfig {
    pub job_type: String,
    pub machine: MachineConfig,
    pub tool_stock: Vec<Tool>,
    pub rack_config: Vec<RackSlot>,
    pub output_config: OutputConfig,
    pub work_origin: WorkOrigin,
    pub rack_options: RackOptions,
}

#[derive(Clone)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone)]
pub struct DrillHole {
    pub x: f64,
    pub y: f64,
    pub diameter: f64,
    pub hole_type: String,
}

#[derive(Clone)]
pub struct Route {
    pub points: Vec<Point>,
    pub width: f64,
}

#[derive(Clone)]
pub struct PCBData {
    pub file_name: String,
    pub drill_holes: Vec<DrillHole>,
    pub routes: Vec<Route>,
    pub board_outline: Vec<Point>,
}

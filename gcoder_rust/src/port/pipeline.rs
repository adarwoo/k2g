#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkStatus {
    Todo,
    InProgress,
    Ready,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortingWorkItem {
    pub python_module: &'static str,
    pub rust_target: &'static str,
    pub status: WorkStatus,
    pub notes: &'static str,
}

pub fn default_porting_work_items() -> Vec<PortingWorkItem> {
    vec![
        PortingWorkItem {
            python_module: "k2g/operations.py",
            rust_target: "src/port/operations.rs",
            status: WorkStatus::Ready,
            notes: "Bitflag-compatible operation selection scaffolded.",
        },
        PortingWorkItem {
            python_module: "k2g/pcb_inventory.py",
            rust_target: "src/port/model.rs + src/port/inventory.rs",
            status: WorkStatus::Ready,
            notes: "Feature and inventory domain model scaffolded.",
        },
        PortingWorkItem {
            python_module: "k2g/board_processor.py",
            rust_target: "src/kicad_adapter.rs (planned)",
            status: WorkStatus::Todo,
            notes: "Map KiCad IPC board entities into Inventory.",
        },
        PortingWorkItem {
            python_module: "k2g/machining.py",
            rust_target: "src/machining.rs (planned)",
            status: WorkStatus::Todo,
            notes: "Tool selection, path ordering, and GCode emission pipeline.",
        },
        PortingWorkItem {
            python_module: "k2g/rack.py + k2g/cutting_tools.py",
            rust_target: "src/tooling.rs + src/rack.rs (planned)",
            status: WorkStatus::Todo,
            notes: "Stock normalization, rack merge, and ATC/manual tooling strategy.",
        },
    ]
}

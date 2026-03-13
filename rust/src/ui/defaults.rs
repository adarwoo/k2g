use super::model::*;

pub fn default_config() -> AppConfig {
    AppConfig {
        job_type: "drill-pth".to_string(),
        machine: MachineConfig {
            name: "CNC Mill 3020".to_string(),
            tool_change: "manual".to_string(),
            max_size_x: 300.0,
            max_size_y: 200.0,
            max_speed_xy: 3000.0,
            max_speed_z: 1000.0,
            acceleration: 500.0,
        },
        tool_stock: vec![
            Tool {
                id: "t1".to_string(),
                tool_type: "drill".to_string(),
                diameter: 0.8,
                name: "0.8mm Drill".to_string(),
                flutes: None,
                status: "in-stock-preferred".to_string(),
            },
            Tool {
                id: "t2".to_string(),
                tool_type: "drill".to_string(),
                diameter: 1.0,
                name: "1.0mm Drill".to_string(),
                flutes: None,
                status: "in-stock-preferred".to_string(),
            },
            Tool {
                id: "t3".to_string(),
                tool_type: "drill".to_string(),
                diameter: 1.5,
                name: "1.5mm Drill".to_string(),
                flutes: None,
                status: "in-stock-preferred".to_string(),
            },
            Tool {
                id: "t4".to_string(),
                tool_type: "router".to_string(),
                diameter: 2.0,
                name: "2.0mm End Mill".to_string(),
                flutes: Some(2),
                status: "in-stock-preferred".to_string(),
            },
            Tool {
                id: "t5".to_string(),
                tool_type: "router".to_string(),
                diameter: 3.175,
                name: "1/8\" End Mill".to_string(),
                flutes: Some(2),
                status: "in-stock-preferred".to_string(),
            },
        ],
        rack_config: (1..=8)
            .map(|i| RackSlot {
                slot_number: i,
                tool_id: if i <= 5 { Some(format!("t{i}")) } else { None },
                disabled: false,
            })
            .collect(),
        output_config: OutputConfig {
            operations: vec![
                OperationTemplate {
                    id: 1,
                    name: "Program Header".to_string(),
                    template: "G21 ; Millimeters\nG90 ; Absolute positioning\nG94 ; Feed per minute".to_string(),
                },
                OperationTemplate {
                    id: 2,
                    name: "Tool Change".to_string(),
                    template: "M6 T{TOOL_NUMBER} ; Change to tool\nM3 S{SPINDLE_SPEED} ; Start spindle".to_string(),
                },
                OperationTemplate {
                    id: 3,
                    name: "Drill Operation".to_string(),
                    template: "G0 Z{SAFE_Z}\nG0 X{X_POS} Y{Y_POS}\nG1 Z{DRILL_DEPTH} F{FEED_RATE}\nG0 Z{SAFE_Z}".to_string(),
                },
                OperationTemplate {
                    id: 4,
                    name: "Route Operation".to_string(),
                    template: "G0 Z{SAFE_Z}\nG0 X{X_START} Y{Y_START}\nG1 Z{CUT_DEPTH} F{PLUNGE_RATE}\nG1 X{X_END} Y{Y_END} F{FEED_RATE}".to_string(),
                },
                OperationTemplate {
                    id: 10,
                    name: "Program Footer".to_string(),
                    template: "M5 ; Stop spindle\nG0 Z{SAFE_Z}\nG0 X0 Y0\nM30 ; Program end".to_string(),
                },
            ],
        },
        work_origin: WorkOrigin {
            x: 0.0,
            y: 0.0,
            coordinate_system: "G54".to_string(),
        },
        rack_options: RackOptions {
            management: "use-existing".to_string(),
            optimize_for_fewer_changes: true,
            allow_mid_job_reload: false,
        },
    }
}

pub fn mock_pcb_data() -> PCBData {
    PCBData {
        file_name: "sample_board.gbr".to_string(),
        drill_holes: vec![
            DrillHole { x: 10.0, y: 10.0, diameter: 0.8, hole_type: "PTH".to_string() },
            DrillHole { x: 20.0, y: 20.0, diameter: 1.0, hole_type: "PTH".to_string() },
            DrillHole { x: 30.0, y: 20.0, diameter: 1.5, hole_type: "NPTH".to_string() },
            DrillHole { x: 40.0, y: 30.0, diameter: 1.0, hole_type: "PTH".to_string() },
            DrillHole { x: 50.0, y: 40.0, diameter: 0.8, hole_type: "NPTH".to_string() },
        ],
        routes: vec![Route {
            points: vec![
                Point { x: 5.0, y: 5.0 },
                Point { x: 95.0, y: 5.0 },
                Point { x: 95.0, y: 75.0 },
                Point { x: 5.0, y: 75.0 },
                Point { x: 5.0, y: 5.0 },
            ],
            width: 0.2,
        }],
        board_outline: vec![
            Point { x: 0.0, y: 0.0 },
            Point { x: 100.0, y: 0.0 },
            Point { x: 100.0, y: 80.0 },
            Point { x: 0.0, y: 80.0 },
        ],
    }
}

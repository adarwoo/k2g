use super::model::{AppConfig, DrillHole, PCBData};

pub fn generate_gcode(config: &AppConfig, pcb_data: &PCBData) -> String {
    let ops = &config.output_config.operations;
    if ops.len() < 5 {
        return String::new();
    }

    let mut code = String::new();
    code.push_str(&ops[0].template);
    code.push_str("\n\n");

    if config.job_type == "drill-locating"
        || config.job_type == "drill-pth"
        || config.job_type == "drill-npth-route"
    {
        let holes: Vec<&DrillHole> = if config.job_type == "drill-pth" {
            pcb_data.drill_holes.iter().filter(|h| h.hole_type == "PTH").collect()
        } else {
            pcb_data.drill_holes.iter().collect()
        };

        let mut grouped: Vec<(f64, Vec<&DrillHole>)> = Vec::new();
        for hole in holes {
            if let Some((_, group)) = grouped
                .iter_mut()
                .find(|(diameter, _)| (*diameter - hole.diameter).abs() < f64::EPSILON)
            {
                group.push(hole);
            } else {
                grouped.push((hole.diameter, vec![hole]));
            }
        }

        for (idx, (diameter, hole_group)) in grouped.iter().enumerate() {
            let tool = config
                .tool_stock
                .iter()
                .find(|t| t.tool_type == "drill" && t.diameter >= *diameter);

            if let Some(selected_tool) = tool {
                let tool_change = ops[1]
                    .template
                    .replace("{TOOL_NUMBER}", &(idx + 1).to_string())
                    .replace("{SPINDLE_SPEED}", "10000");
                code.push_str(&format!(
                    "; Tool: {} ({:.3}mm)\n{}\n\n",
                    selected_tool.name, selected_tool.diameter, tool_change
                ));

                for hole in hole_group {
                    let drill = ops[2]
                        .template
                        .replace("{SAFE_Z}", "5.0")
                        .replace("{X_POS}", &format!("{:.3}", hole.x))
                        .replace("{Y_POS}", &format!("{:.3}", hole.y))
                        .replace("{DRILL_DEPTH}", "-2.0")
                        .replace("{FEED_RATE}", "100");
                    code.push_str(&drill);
                    code.push('\n');
                }
                code.push('\n');
            }
        }
    }

    if config.job_type == "drill-npth-route" || config.job_type == "route-board" {
        if let Some(route_tool) = config.tool_stock.iter().find(|t| t.tool_type == "router") {
            if !pcb_data.routes.is_empty() {
                let tool_change = ops[1]
                    .template
                    .replace("{TOOL_NUMBER}", "99")
                    .replace("{SPINDLE_SPEED}", "12000");
                code.push_str(&format!("; Tool: {}\n{}\n\n", route_tool.name, tool_change));

                for route in &pcb_data.routes {
                    for segment in route.points.windows(2) {
                        let start = &segment[0];
                        let end = &segment[1];
                        let route_op = ops[3]
                            .template
                            .replace("{SAFE_Z}", "5.0")
                            .replace("{X_START}", &format!("{:.3}", start.x))
                            .replace("{Y_START}", &format!("{:.3}", start.y))
                            .replace("{X_END}", &format!("{:.3}", end.x))
                            .replace("{Y_END}", &format!("{:.3}", end.y))
                            .replace("{CUT_DEPTH}", "-0.2")
                            .replace("{PLUNGE_RATE}", "50")
                            .replace("{FEED_RATE}", "300");
                        code.push_str(&route_op);
                        code.push('\n');
                    }
                }
                code.push('\n');
            }
        }
    }

    code.push_str(&ops[4].template.replace("{SAFE_Z}", "5.0"));
    code.push('\n');
    code
}

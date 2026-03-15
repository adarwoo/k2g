use gcode_rhai_lab::{GcodeTemplateParser, Length, TemplateContext};

const TEMPLATE: &str = r#"(Created by k2g from '{pcb_filename}' - {now().format("%Y-%m-%d %H:%M:%S")})
(Reset all back to safe defaults)
G17 G54 G40 G49 G80 G90
G21
G10 P0
(Establish the Z-Safe)
G0 Z{z_safe_height.mm}
{if has_positioning_pins {"G56"} else {"G54"}}
"#;

fn main() {
    let parser = GcodeTemplateParser::new();
    let context = TemplateContext {
        pcb_filename: "pulsegen.kicad_pcb".to_string(),
        has_positioning_pins: true,
        z_safe_height: Length::from_mm(-40.0),
        ..TemplateContext::default()
    };

    match parser.render(TEMPLATE, &context) {
        Ok(gcode) => {
            println!("{gcode}");
        }
        Err(err) => {
            eprintln!("Failed to render template: {err}");
            std::process::exit(1);
        }
    }
}

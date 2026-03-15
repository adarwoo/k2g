# gcode-rhai-lab

Prototype crate for rendering GCode templates where each `{...}` block is evaluated as a Rhai expression.

## What It Demonstrates

- Inline expression evaluation inside text templates
- Prefilled script variables (for example `pcb_filename`, `z_safe_height`, `has_positioning_pins`)
- Prefilled helper functions (for example `now()`, `mm_to_nm()`, `clamp()`)

## Run Tests

```bash
cargo test --manifest-path gcoder_rust/gcode-rhai-lab/Cargo.toml
```

## Run Demo

```bash
cargo run --manifest-path gcoder_rust/gcode-rhai-lab/Cargo.toml
```

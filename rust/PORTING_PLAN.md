# GCoder Python -> Rust Porting Plan

This repository currently runs a Rust UI + KiCad IPC connectivity check.
The legacy implementation in `kicad2gcode/k2g` remains the functional reference.

## Current scaffold status

- Done: operation flags scaffold (`src/port/operations.rs`)
- Done: inventory and feature model scaffold (`src/port/model.rs`, `src/port/inventory.rs`)
- Done: migration work-item map (`src/port/pipeline.rs`)
- Pending: KiCad IPC board extraction adapter
- Pending: tooling/rack selection and machining pipeline
- Pending: GCode profile rendering and output

## Recommended migration order

1. **Board adapter first**
   - Implement a Rust adapter that reads board holes/vias/oblongs from KiCad IPC
   - Populate `Inventory` in nanometers (no unit conversion loss)

2. **Tooling + rack**
   - Port stock normalization logic from `k2g/cutting_tools.py`
   - Port rack merge/sort behavior from `k2g/rack.py`

3. **Machining planner**
   - Port operation generation from `k2g/machining.py`
   - Keep operation model independent of output profile

4. **GCode profile emitter**
   - Port profile-specific output in a dedicated module
   - Keep command generation testable with string snapshots

## Parity checkpoints

- First compile-time milestone: map one open board into `Inventory`
- Functional milestone: NPTH drill-only output
- Full milestone: PTH + NPTH + outline with rack decisions

## Notes

- Keep Python behavior as source-of-truth while porting.
- Port in vertical slices to preserve testability.
- Prefer exact numeric representation in nm for geometry and tool matching.

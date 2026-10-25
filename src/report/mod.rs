//! The setup sheet: one PDF, written beside the exported programs, describing the board and
//! every machining step well enough to set the job up and run it without k2g open.
//!
//! Three stages, each its own module:
//!
//! 1. [`model`] — what the sheet says, built from the same plans the app's views show.
//! 2. [`layout`] — where it goes on the page: wrapping, tables, page breaks.
//! 3. [`pdf`] — drawing the laid-out pages, with a bundled font.
//!
//! This module gathers the sources from the live context ([`collect`]), turns the model into
//! blocks ([`blocks`]), and is what the Export dialog calls ([`setup_sheet_pdf`]).

pub mod layout;
pub mod model;
pub mod pdf;

use std::collections::BTreeMap;

use uuid::Uuid;

use crate::runtime::machining_plan::cached_plan;
use crate::runtime::tooling::{engrave_target, plan_tooling, read_steps};
use crate::runtime::AppCtx;
use layout::Block;
use model::{EngraveFacts, HeldFacts, RackSection, SetupSheet, Sources};

/// The setup sheet for the job on screen, as PDF bytes.
///
/// `program_files` is what each step's program is being written as, by step index — the
/// names the Export dialog is about to use, so the sheet names the files that are actually
/// on the stick.
pub fn setup_sheet_pdf(ctx: &AppCtx, program_files: &BTreeMap<usize, String>) -> Result<Vec<u8>, String> {
    let sheet = setup_sheet(ctx, program_files)?;
    let fonts = pdf::Fonts::load()?;
    let pages = layout::layout(&fonts, &blocks(&sheet));
    let chrome = pdf::Chrome {
        header: format!("{} · {}", sheet.board_name, sheet.profile_name),
        generated: sheet.generated.clone(),
        producer: format!("Setup sheet · k2g {}", sheet.version),
        title: format!("Setup sheet — {}", sheet.board_name),
    };
    pdf::write(&fonts, &pages, &chrome)
}

/// The sheet's content, from the live context.
pub fn setup_sheet(ctx: &AppCtx, program_files: &BTreeMap<usize, String>) -> Result<SetupSheet, String> {
    let profile = ctx
        .selected_process_profile()
        .ok_or("No machining profile is selected.")?;
    let profile_id = Uuid::parse_str(&profile.id).map_err(|_| "The machining profile has no valid id.")?;
    let steps = read_steps(profile_id);
    let tooling = plan_tooling(ctx, ctx.stitched_board_data.as_ref());
    let plan = cached_plan(ctx);
    let tool_feeds = ctx.tool_feed_map();

    let engraving = (0..steps.len())
        .map(|index| {
            let raw = steps.get(index)?;
            if !raw.isolates_traces() {
                return None;
            }
            // `engrave_target` falls back to the first engraving step when asked about one
            // that does not engrave; only take its answer for the step that does.
            let target = engrave_target(ctx, index).filter(|t| t.is_selected_step)?;
            let held = ctx.isolation.matching(&target.spec).map(|isolation| {
                let result = &isolation.result;
                HeldFacts {
                    contours: result.contours.len(),
                    length_nm: result.contours.iter().map(|c| path_length_nm(&c.path, c.closed)).sum(),
                    narrowed: result.contours.iter().filter(|c| c.width_nm < target.spec.width_nm).count(),
                    uncut_nm: result.uncut.iter().map(|u| u.length_nm).sum(),
                    islands_removed: isolation.clearing.removed,
                    milled_pieces: isolation.mill_clearing.removed,
                    copper_layers: isolation.copper_layer_count,
                }
            });
            Some(EngraveFacts {
                back: target.spec.layer_id == pcb::BACK_COPPER,
                tool: target.tool_label.clone(),
                requested: target.requested,
                achieved: target.achieved,
                held,
            })
        })
        .collect();
    let limits = steps.iter().map(|raw| ctx.step_machine_limits(raw)).collect();

    let sources = Sources {
        board: ctx.board.as_ref(),
        stitched: ctx.stitched_board_data.as_ref(),
        profile_name: profile.name.clone(),
        generated: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        unit: ctx.unit_system,
        steps: &steps,
        tooling: &tooling,
        plan: &plan,
        programs: &ctx.programs,
        program_files,
        engraving,
        limits,
        machines: &ctx.machines,
        fixtures: &ctx.fixtures,
        toolsets: &ctx.toolsets,
        tools: &ctx.tools,
        tool_feeds: &tool_feeds,
    };
    Ok(model::build(&sources))
}

/// A polyline's length, nm, closing it when it is a loop.
fn path_length_nm(path: &[(i64, i64)], closed: bool) -> f64 {
    let segment = |a: (i64, i64), b: (i64, i64)| ((b.0 - a.0) as f64).hypot((b.1 - a.1) as f64);
    let open: f64 = path.windows(2).map(|w| segment(w[0], w[1])).sum();
    match (closed, path.first(), path.last()) {
        (true, Some(&first), Some(&last)) => open + segment(last, first),
        _ => open,
    }
}

/// The sheet as blocks to lay out: the job, the board, then one page per step.
pub fn blocks(sheet: &SetupSheet) -> Vec<Block> {
    let mut out = vec![
        Block::Title(format!("Setup sheet — {}", sheet.board_name)),
        Block::KeyValues(vec![
            ("Board".into(), sheet.board_name.clone()),
            ("Machining profile".into(), sheet.profile_name.clone()),
            ("Steps".into(), sheet.steps.len().to_string()),
            ("Made".into(), format!("{} by k2g {}", sheet.generated, sheet.version)),
        ]),
        Block::Heading("Steps".into()),
        Block::Table {
            header: ["#", "Step", "Face", "Machine", "Work", "Program"].map(String::from).to_vec(),
            rows: sheet
                .steps
                .iter()
                .map(|step| {
                    vec![
                        step.number.to_string(),
                        step.name.clone(),
                        step.face.clone(),
                        step.machine.clone(),
                        step.work.clone(),
                        if step.program_file.is_empty() { "—".into() } else { step.program_file.clone() },
                    ]
                })
                .collect(),
        },
        Block::Heading("Board".into()),
        Block::KeyValues(sheet.board.clone()),
    ];
    if !sheet.holes.is_empty() {
        out.push(Block::Subheading("Holes".into()));
        out.push(Block::Table { header: ["Kind", "Size", "Count"].map(String::from).to_vec(), rows: sheet.holes.clone() });
    }
    for engraving in &sheet.engraving {
        out.push(Block::Subheading(format!("Isolation — {}", engraving.title)));
        out.push(Block::KeyValues(engraving.facts.clone()));
    }

    for step in &sheet.steps {
        out.push(Block::PageBreak);
        out.push(Block::Title(format!("Step {} — {}", step.number, step.name)));
        out.push(Block::KeyValues(vec![
            ("Side".into(), step.face.clone()),
            ("Type".into(), format!("{}\n{}", step.work, step.machine)),
            ("Program".into(), step.program.clone()),
        ]));
        out.push(Block::Heading("Operations".into()));
        out.push(Block::Bullets(step.operations.clone()));
        out.push(Block::Heading("Setup".into()));
        out.push(Block::KeyValues(step.setup.clone()));
        out.push(Block::Heading("Rack".into()));
        out.extend(rack_blocks(&step.rack));
        if !step.tools.is_empty() {
            out.push(Block::Heading("Tools, in the order they run".into()));
            out.push(Block::Table {
                header: ["Slot", "Tool", "Size", "Feed", "Plunge", "Spindle", "Depth", "Ops"].map(String::from).to_vec(),
                rows: step
                    .tools
                    .iter()
                    .map(|t| {
                        vec![
                            t.slot.clone(),
                            t.tool.clone(),
                            t.diameter.clone(),
                            t.feed.clone(),
                            t.plunge.clone(),
                            t.spindle.clone(),
                            t.depth.clone(),
                            t.ops.to_string(),
                        ]
                    })
                    .collect(),
            });
        }
        if !step.notes.is_empty() {
            out.push(Block::Heading("Notes".into()));
            out.push(Block::Bullets(step.notes.clone()));
        }
    }
    out
}

/// The rack section's blocks: the changer's slots and what to do to them, or the order the
/// tools go in by hand.
fn rack_blocks(rack: &RackSection) -> Vec<Block> {
    match rack {
        RackSection::Changer { lines, unplaced, clipped, manual_changes } => {
            let mut out = vec![Block::Paragraph(match manual_changes {
                0 => "Nothing to change: every tool this step needs is already in the rack.".to_string(),
                1 => "1 manual change before this step.".to_string(),
                n => format!("{n} manual changes before this step."),
            })];
            out.push(Block::Table {
                header: ["Slot", "Tool", "Before this step"].map(String::from).to_vec(),
                rows: lines.iter().map(|l| vec![l.slot.clone(), l.tool.clone(), l.action.clone()]).collect(),
            });
            if !unplaced.is_empty() {
                out.push(Block::Paragraph("The rack cannot hold these as well; change them by hand when the program asks:".into()));
                out.push(Block::Bullets(unplaced.clone()));
            }
            if *clipped > 0 {
                out.push(Block::Paragraph(format!(
                    "{clipped} toolset slot{} beyond this machine's changer cannot be loaded here.",
                    if *clipped == 1 { "" } else { "s" },
                )));
            }
            out
        }
        RackSection::Manual { sequence } => vec![
            Block::Paragraph(format!(
                "No tool changer: {} manual change{}. The program stops before each tool and names it.",
                sequence.len(),
                if sequence.len() == 1 { "" } else { "s" },
            )),
            Block::Bullets(sequence.iter().enumerate().map(|(n, tool)| format!("{}. {tool}", n + 1)).collect()),
        ],
        RackSection::Failed(reasons) => {
            let mut out = vec![Block::Paragraph("This step has no tooling solution:".into())];
            out.push(Block::Bullets(reasons.clone()));
            out
        }
        RackSection::Nothing => vec![Block::Paragraph("This step loads no tools.".into())],
    }
}

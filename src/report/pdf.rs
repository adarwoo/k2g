//! Draws laid-out pages ([`super::layout`]) into a PDF with krilla.
//!
//! **Text is set from the font's own metrics, not shaped.** Each character is looked up in
//! the font's character map and advanced by its own width, through [`FontFace`], which is also
//! what [`Measure`] measures with. The layout therefore wraps against exactly the widths that
//! are drawn. That is enough for a sheet of Latin text, numbers and symbols; it would not be
//! for a script that needs shaping, and the sheet has none.

use std::sync::Arc;

use krilla::color::rgb;
use krilla::geom::{PathBuilder, Point, Rect};
use krilla::metadata::Metadata;
use krilla::page::PageSettings;
use krilla::paint::{Fill, Stroke};
use krilla::surface::Surface;
use krilla::text::{Font, GlyphId, KrillaGlyph};
use krilla::Document;
use skrifa::instance::{LocationRef, Size};
use skrifa::{FontRef, MetadataProvider};

use super::layout::{chrome, Item, Measure, Page, PAGE_HEIGHT, PAGE_WIDTH};

/// DejaVu Sans, bundled so the binary needs nothing beside it. Chosen over Noto Sans because
/// it has ⌀ (U+2300), which every drill size on the sheet is written with, and Noto Sans does
/// not. Licence: `assets/fonts/DejaVu-LICENSE.txt`.
static REGULAR: &[u8] = include_bytes!("../../assets/fonts/DejaVuSans.ttf");
static BOLD: &[u8] = include_bytes!("../../assets/fonts/DejaVuSans-Bold.ttf");

/// One font, as both krilla (to embed and draw) and skrifa (to measure) see it.
struct FontFace {
    pdf: Font,
    metrics: FontRef<'static>,
    units_per_em: f32,
}

impl FontFace {
    fn new(data: &'static [u8]) -> Result<Self, String> {
        let shared: Arc<dyn AsRef<[u8]> + Send + Sync> = Arc::new(data);
        let pdf = Font::new(shared.into(), 0).ok_or("the bundled font could not be loaded")?;
        let metrics = FontRef::new(data).map_err(|e| format!("the bundled font could not be read: {e}"))?;
        let units_per_em = metrics.metrics(Size::unscaled(), LocationRef::default()).units_per_em as f32;
        Ok(Self { pdf, metrics, units_per_em })
    }

    /// The glyphs for `text`, each advanced by its own width in ems. A character the font
    /// does not have becomes glyph 0, the font's "missing" box, rather than vanishing.
    fn glyphs(&self, text: &str) -> Vec<KrillaGlyph> {
        let charmap = self.metrics.charmap();
        let advances = self.metrics.glyph_metrics(Size::unscaled(), LocationRef::default());
        text.char_indices()
            .map(|(at, ch)| {
                let id = charmap.map(ch).unwrap_or_default();
                let advance = advances.advance_width(id).unwrap_or(0.0) / self.units_per_em;
                KrillaGlyph::new(GlyphId::new(id.to_u32()), advance, 0.0, 0.0, 0.0, at..at + ch.len_utf8(), None)
            })
            .collect()
    }
}

/// The sheet's two fonts, and the measure the layout runs on.
pub struct Fonts {
    regular: FontFace,
    bold: FontFace,
}

impl Fonts {
    pub fn load() -> Result<Self, String> {
        Ok(Self { regular: FontFace::new(REGULAR)?, bold: FontFace::new(BOLD)? })
    }

    fn face(&self, bold: bool) -> &FontFace {
        if bold { &self.bold } else { &self.regular }
    }
}

impl Measure for Fonts {
    fn width(&self, text: &str, size: f32, bold: bool) -> f32 {
        self.face(bold).glyphs(text).iter().map(|g| g.x_advance).sum::<f32>() * size
    }
}

/// What every page carries besides its body.
pub struct Chrome {
    /// Left of the header: the board and the profile.
    pub header: String,
    /// Right of the header: when the sheet was made.
    pub generated: String,
    /// Left of the footer: what made it.
    pub producer: String,
    /// The document title in the PDF's own metadata.
    pub title: String,
}

/// The pages as a PDF file.
pub fn write(fonts: &Fonts, pages: &[Page], chrome: &Chrome) -> Result<Vec<u8>, String> {
    let mut document = Document::new();
    document.set_metadata(Metadata::new().title(chrome.title.clone()).creator(chrome.producer.clone()));
    let count = pages.len();
    for (index, page) in pages.iter().enumerate() {
        let settings = PageSettings::from_wh(PAGE_WIDTH, PAGE_HEIGHT).ok_or("invalid page size")?;
        let mut pdf_page = document.start_page_with(settings);
        let mut surface = pdf_page.surface();

        // The running header and footer, in grey, so the body reads first.
        let grey = rgb::Color::new(110, 110, 110);
        text(&mut surface, fonts, chrome::LEFT, chrome::HEADER_BASELINE, chrome::SIZE, true, &chrome.header, grey);
        let right = chrome::RIGHT - fonts.width(&chrome.generated, chrome::SIZE, false);
        text(&mut surface, fonts, right, chrome::HEADER_BASELINE, chrome::SIZE, false, &chrome.generated, grey);
        rule(&mut surface, chrome::LEFT, chrome::RIGHT, chrome::HEADER_BASELINE + 6.0, 0.5);
        text(&mut surface, fonts, chrome::LEFT, chrome::FOOTER_BASELINE, chrome::SIZE, false, &chrome.producer, grey);
        let page_label = format!("Page {} of {count}", index + 1);
        let right = chrome::RIGHT - fonts.width(&page_label, chrome::SIZE, false);
        text(&mut surface, fonts, right, chrome::FOOTER_BASELINE, chrome::SIZE, false, &page_label, grey);

        let ink = rgb::Color::new(25, 25, 25);
        for item in &page.items {
            match item {
                Item::Text { x, y, size, bold, text: line } => {
                    text(&mut surface, fonts, *x, *y, *size, *bold, line, ink);
                }
                Item::Rule { x1, x2, y } => rule(&mut surface, *x1, *x2, *y, 0.4),
                Item::Shade { x, y, w, h } => {
                    let mut path = PathBuilder::new();
                    if let Some(rect) = Rect::from_xywh(*x, *y, *w, *h) {
                        path.push_rect(rect);
                    }
                    if let Some(path) = path.finish() {
                        surface.set_stroke(None);
                        surface.set_fill(Some(Fill { paint: rgb::Color::new(232, 232, 232).into(), ..Fill::default() }));
                        surface.draw_path(&path);
                    }
                }
            }
        }
        surface.finish();
        pdf_page.finish();
    }
    document.finish().map_err(|e| format!("the PDF could not be written: {e:?}"))
}

/// One line of text with its baseline at `y`.
#[allow(clippy::too_many_arguments)]
fn text(surface: &mut Surface, fonts: &Fonts, x: f32, y: f32, size: f32, bold: bool, line: &str, colour: rgb::Color) {
    if line.is_empty() {
        return;
    }
    let face = fonts.face(bold);
    surface.set_stroke(None);
    surface.set_fill(Some(Fill { paint: colour.into(), ..Fill::default() }));
    surface.draw_glyphs(Point::from_xy(x, y), &face.glyphs(line), face.pdf.clone(), line, size, false);
}

/// A thin grey horizontal line.
fn rule(surface: &mut Surface, x1: f32, x2: f32, y: f32, width: f32) {
    let mut path = PathBuilder::new();
    path.move_to(x1, y);
    path.line_to(x2, y);
    if let Some(path) = path.finish() {
        surface.set_fill(None);
        surface.set_stroke(Some(Stroke { paint: rgb::Color::new(170, 170, 170).into(), width, ..Stroke::default() }));
        surface.draw_path(&path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::layout::{layout, Block};

    /// The bundled fonts have every symbol the sheet writes. A missing one prints as an empty
    /// box, and ⌀ in particular is on every drill size.
    #[test]
    fn the_bundled_fonts_have_every_symbol_the_sheet_uses() {
        let fonts = Fonts::load().expect("the bundled fonts load");
        for face in [&fonts.regular, &fonts.bold] {
            let charmap = face.metrics.charmap();
            for ch in "⌀µ·×–—…°±→≤≥•".chars() {
                assert!(charmap.map(ch).is_some(), "U+{:04X} is missing", ch as u32);
            }
        }
    }

    /// Measured widths are the widths drawn: wider text measures wider, and bold is wider than
    /// regular, which a wrapping layout depends on.
    #[test]
    fn text_is_measured_from_the_font() {
        let fonts = Fonts::load().unwrap();
        let short = fonts.width("T1", 10.0, false);
        let long = fonts.width("T1 drill ⌀0.80 mm", 10.0, false);
        assert!(short > 0.0 && long > short);
        assert!(fonts.width("Board", 10.0, true) > fonts.width("Board", 10.0, false));
        assert!((fonts.width("ab", 20.0, false) - 2.0 * fonts.width("ab", 10.0, false)).abs() < 1e-3);
    }

    /// A sheet renders to a well-formed PDF, one page per laid-out page.
    #[test]
    fn a_sheet_renders_to_a_pdf() {
        let fonts = Fonts::load().unwrap();
        let pages = layout(
            &fonts,
            &[
                Block::Title("panel.kicad_pcb".into()),
                Block::KeyValues(vec![("Size".into(), "50 × 30 mm".into())]),
                Block::PageBreak,
                Block::Table { header: vec!["Slot".into(), "Tool".into()], rows: vec![vec!["T1".into(), "⌀0.8 drill".into()]] },
            ],
        );
        assert_eq!(pages.len(), 2);
        let chrome = Chrome {
            header: "panel · Two-sided".into(),
            generated: "2026-10-18 17:00".into(),
            producer: "k2g".into(),
            title: "Setup sheet".into(),
        };
        let bytes = write(&fonts, &pages, &chrome).expect("the PDF is written");
        assert!(bytes.starts_with(b"%PDF-"), "a PDF header");
        // The page tree's own count, which is what a viewer reads.
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("/Count 2"), "two pages in the page tree");
    }
}

//! Flow layout for the setup sheet: blocks of text and tables in, positioned pages out.
//!
//! Pure, with no PDF in it, so page breaks and wrapping can be tested on their own. The PDF
//! writer ([`super::pdf`]) only draws what this has placed. Text is measured through
//! [`Measure`], which the writer implements with the very font metrics it draws with — so a
//! line wrapped to fit a column cannot come out wider than it.
//!
//! Coordinates are PDF points from the page's top-left corner, y growing downwards (the
//! writer's surface is set up the same way). A text item's `y` is its **baseline**.

/// A4 portrait, in points.
pub const PAGE_WIDTH: f32 = 595.28;
pub const PAGE_HEIGHT: f32 = 841.89;

/// Left and right margin.
const MARGIN_X: f32 = 50.0;
/// Top of the body: below the running header.
const BODY_TOP: f32 = 78.0;
/// Bottom of the body: above the running footer.
const BODY_BOTTOM: f32 = PAGE_HEIGHT - 60.0;

/// Body text size, and the sizes everything else is stated against.
pub const BODY_SIZE: f32 = 9.5;
const TITLE_SIZE: f32 = 20.0;
const HEADING_SIZE: f32 = 14.0;
const SUBHEADING_SIZE: f32 = 11.0;
const TABLE_SIZE: f32 = 8.5;

/// Line height as a multiple of the font size.
const LEADING: f32 = 1.35;
/// Padding inside a table cell, each side.
const CELL_PAD: f32 = 3.5;
/// Gap between a key and its value in a key/value list.
const KEY_GAP: f32 = 10.0;

/// The width of text at a size, in points. Implemented by the PDF writer from the real font;
/// tests use a fixed-pitch stand-in.
pub trait Measure {
    fn width(&self, text: &str, size: f32, bold: bool) -> f32;
}

/// What the sheet is written as, before it is placed on pages.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Title(String),
    /// A section heading. Kept on the same page as what follows it.
    Heading(String),
    /// A heading inside a section.
    Subheading(String),
    Paragraph(String),
    /// Short points, one per line.
    Bullets(Vec<String>),
    /// A two-column list: the key in bold, the value wrapped beside it.
    KeyValues(Vec<(String, String)>),
    /// A table with a header row, which repeats at the top of each page it runs onto.
    Table { header: Vec<String>, rows: Vec<Vec<String>> },
    /// Starts the next block on a new page — each step's section does.
    PageBreak,
}

/// One thing placed on a page.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Text { x: f32, y: f32, size: f32, bold: bool, text: String },
    /// A thin horizontal rule.
    Rule { x1: f32, x2: f32, y: f32 },
    /// A shaded rectangle (a table's header row).
    Shade { x: f32, y: f32, w: f32, h: f32 },
}

/// One laid-out page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page {
    pub items: Vec<Item>,
}

/// The usable width of the body.
fn body_width() -> f32 {
    PAGE_WIDTH - 2.0 * MARGIN_X
}

/// Wraps `text` into lines no wider than `width`.
///
/// Greedy, word by word, which is all a sheet of short values needs. A word wider than the
/// whole line (a long file name, a tool label with no spaces) is broken between characters
/// rather than allowed to run out of its column. Explicit newlines are kept.
pub fn wrap(measure: &dyn Measure, text: &str, size: f32, bold: bool, width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if measure.width(&candidate, size, bold) <= width {
                line = candidate;
                continue;
            }
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            if measure.width(word, size, bold) <= width {
                line = word.to_string();
                continue;
            }
            // Too wide on its own: break it where it stops fitting.
            for ch in word.chars() {
                let mut next = line.clone();
                next.push(ch);
                if !line.is_empty() && measure.width(&next, size, bold) > width {
                    lines.push(std::mem::take(&mut line));
                    line.push(ch);
                } else {
                    line = next;
                }
            }
        }
        lines.push(line);
    }
    lines
}

/// Column widths for a table, summing to `available`.
///
/// Each column wants its widest cell. When they all fit, the last column takes the slack. When
/// they do not, columns no wider than an even share keep their natural width, and the rest
/// share what is left in proportion to what they wanted — so a short "Slot" column is never
/// squeezed to make room for a long tool name.
fn column_widths(measure: &dyn Measure, header: &[String], rows: &[Vec<String>], available: f32) -> Vec<f32> {
    let columns = header.len().max(rows.iter().map(Vec::len).max().unwrap_or(0));
    if columns == 0 {
        return Vec::new();
    }
    let natural: Vec<f32> = (0..columns)
        .map(|c| {
            let head = header.get(c).map_or(0.0, |h| measure.width(h, TABLE_SIZE, true));
            let cells = rows
                .iter()
                .filter_map(|row| row.get(c))
                .map(|cell| measure.width(cell, TABLE_SIZE, false))
                .fold(0.0, f32::max);
            head.max(cells) + 2.0 * CELL_PAD
        })
        .collect();
    let total: f32 = natural.iter().sum();
    if total <= available {
        let mut widths = natural;
        if let Some(last) = widths.last_mut() {
            *last += available - total;
        }
        return widths;
    }
    let share = available / columns as f32;
    let fixed: f32 = natural.iter().filter(|w| **w <= share).sum();
    let flexible: f32 = natural.iter().filter(|w| **w > share).sum();
    let room = (available - fixed).max(0.0);
    natural
        .iter()
        .map(|w| if *w <= share { *w } else { room * w / flexible })
        .collect()
}

/// Lays `blocks` out on as many pages as they need.
pub fn layout(measure: &dyn Measure, blocks: &[Block]) -> Vec<Page> {
    let mut flow = Flow { pages: vec![Page::default()], y: BODY_TOP, measure };
    for (index, block) in blocks.iter().enumerate() {
        flow.block(block, blocks.get(index + 1));
    }
    flow.pages
}

/// The layout's running state: the pages so far and where on the last one the next block goes.
struct Flow<'a> {
    pages: Vec<Page>,
    /// The top of the free space on the current page.
    y: f32,
    measure: &'a dyn Measure,
}

impl Flow<'_> {
    fn page(&mut self) -> &mut Page {
        self.pages.last_mut().expect("the flow always has a page")
    }

    fn new_page(&mut self) {
        self.pages.push(Page::default());
        self.y = BODY_TOP;
    }

    /// Starts a new page unless `height` still fits on this one. A block taller than a whole
    /// page is placed at the top of a fresh one and allowed to run past the bottom rather
    /// than looping forever.
    fn ensure(&mut self, height: f32) {
        if self.y + height > BODY_BOTTOM && self.y > BODY_TOP {
            self.new_page();
        }
    }

    fn text(&mut self, x: f32, baseline: f32, size: f32, bold: bool, text: String) {
        self.page().items.push(Item::Text { x, y: baseline, size, bold, text });
    }

    /// Wrapped lines at `x`, advancing the flow; breaks pages between lines when it must.
    fn lines(&mut self, x: f32, width: f32, size: f32, bold: bool, text: &str) {
        let line_height = size * LEADING;
        for line in wrap(self.measure, text, size, bold, width) {
            self.ensure(line_height);
            let baseline = self.y + size;
            self.text(x, baseline, size, bold, line);
            self.y += line_height;
        }
    }

    /// The height a block's first few lines need, so a heading is never left alone at the
    /// bottom of a page with its content on the next.
    fn lead_in(next: Option<&Block>) -> f32 {
        match next {
            Some(Block::Table { .. }) => 3.0 * TABLE_SIZE * LEADING + 4.0 * CELL_PAD,
            Some(Block::PageBreak) | None => 0.0,
            _ => 3.0 * BODY_SIZE * LEADING,
        }
    }

    fn block(&mut self, block: &Block, next: Option<&Block>) {
        let x = MARGIN_X;
        let width = body_width();
        match block {
            Block::Title(text) => {
                self.lines(x, width, TITLE_SIZE, true, text);
                self.y += 6.0;
            }
            Block::Heading(text) => {
                self.y += 8.0;
                self.ensure(HEADING_SIZE * LEADING + Self::lead_in(next));
                self.lines(x, width, HEADING_SIZE, true, text);
                let y = self.y + 1.0;
                self.page().items.push(Item::Rule { x1: x, x2: x + width, y });
                self.y += 6.0;
            }
            Block::Subheading(text) => {
                self.y += 6.0;
                self.ensure(SUBHEADING_SIZE * LEADING + Self::lead_in(next));
                self.lines(x, width, SUBHEADING_SIZE, true, text);
                self.y += 2.0;
            }
            Block::Paragraph(text) => {
                self.lines(x, width, BODY_SIZE, false, text);
                self.y += 4.0;
            }
            Block::Bullets(points) => {
                let indent = 12.0;
                for point in points {
                    let line_height = BODY_SIZE * LEADING;
                    self.ensure(line_height);
                    let baseline = self.y + BODY_SIZE;
                    self.text(x + 2.0, baseline, BODY_SIZE, false, "•".to_string());
                    self.lines(x + indent, width - indent, BODY_SIZE, false, point);
                }
                self.y += 4.0;
            }
            Block::KeyValues(pairs) => self.key_values(pairs),
            Block::Table { header, rows } => self.table(header, rows),
            Block::PageBreak => {
                if !self.page().items.is_empty() {
                    self.new_page();
                }
            }
        }
    }

    fn key_values(&mut self, pairs: &[(String, String)]) {
        let x = MARGIN_X;
        let width = body_width();
        // Keys line up in one column, as wide as the widest key but never more than a third
        // of the page, so a long key wraps rather than pushing every value off to the right.
        let key_width = pairs
            .iter()
            .map(|(key, _)| self.measure.width(key, BODY_SIZE, true))
            .fold(0.0, f32::max)
            .min(width / 3.0);
        let value_x = x + key_width + KEY_GAP;
        let value_width = width - key_width - KEY_GAP;
        let line_height = BODY_SIZE * LEADING;
        for (key, value) in pairs {
            let keys = wrap(self.measure, key, BODY_SIZE, true, key_width);
            let values = wrap(self.measure, value, BODY_SIZE, false, value_width);
            // A pair is kept together: a key on one page and its value on the next reads as
            // two half-entries.
            let rows = keys.len().max(values.len());
            self.ensure(rows as f32 * line_height);
            for row in 0..rows {
                let baseline = self.y + BODY_SIZE;
                if let Some(key) = keys.get(row) {
                    self.text(x, baseline, BODY_SIZE, true, key.clone());
                }
                if let Some(value) = values.get(row) {
                    self.text(value_x, baseline, BODY_SIZE, false, value.clone());
                }
                self.y += line_height;
            }
        }
        self.y += 4.0;
    }

    fn table(&mut self, header: &[String], rows: &[Vec<String>]) {
        let x = MARGIN_X;
        let widths = column_widths(self.measure, header, rows, body_width());
        let line_height = TABLE_SIZE * LEADING;
        let wrap_row = |measure: &dyn Measure, cells: &[String], bold: bool| -> Vec<Vec<String>> {
            widths
                .iter()
                .enumerate()
                .map(|(c, w)| {
                    let text = cells.get(c).map(String::as_str).unwrap_or("");
                    wrap(measure, text, TABLE_SIZE, bold, (w - 2.0 * CELL_PAD).max(1.0))
                })
                .collect()
        };
        let height_of = |cells: &[Vec<String>]| {
            cells.iter().map(Vec::len).max().unwrap_or(1).max(1) as f32 * line_height + 2.0 * CELL_PAD
        };

        let header_cells = wrap_row(self.measure, header, true);
        let header_height = height_of(&header_cells);
        let place_header = |flow: &mut Self| {
            let total: f32 = widths.iter().sum();
            let y = flow.y;
            flow.page().items.push(Item::Shade { x, y, w: total, h: header_height });
            flow.cells(x, &widths, &header_cells, true);
            flow.y += header_height;
        };

        // The header and the first row go together, for the reason a heading keeps with its
        // text.
        let first = rows.first().map(|row| height_of(&wrap_row(self.measure, row, false))).unwrap_or(0.0);
        self.ensure(header_height + first);
        place_header(self);

        for row in rows {
            let cells = wrap_row(self.measure, row, false);
            let height = height_of(&cells);
            // **A row is never split across pages**; when one will not fit, the table carries
            // on overleaf under its own header again.
            if self.y + height > BODY_BOTTOM {
                self.new_page();
                place_header(self);
            }
            self.cells(x, &widths, &cells, false);
            self.y += height;
            let total: f32 = widths.iter().sum();
            let y = self.y;
            self.page().items.push(Item::Rule { x1: x, x2: x + total, y });
        }
        self.y += 8.0;
    }

    /// One row's cells at the current y. Does not advance the flow.
    fn cells(&mut self, x: f32, widths: &[f32], cells: &[Vec<String>], bold: bool) {
        let line_height = TABLE_SIZE * LEADING;
        let mut cell_x = x;
        for (lines, width) in cells.iter().zip(widths) {
            for (n, line) in lines.iter().enumerate() {
                let baseline = self.y + CELL_PAD + TABLE_SIZE + n as f32 * line_height;
                self.text(cell_x + CELL_PAD, baseline, TABLE_SIZE, bold, line.clone());
            }
            cell_x += width;
        }
    }
}

/// Where the running header and footer go, for the writer.
pub mod chrome {
    use super::*;

    pub const HEADER_BASELINE: f32 = 40.0;
    pub const FOOTER_BASELINE: f32 = PAGE_HEIGHT - 32.0;
    pub const LEFT: f32 = MARGIN_X;
    pub const RIGHT: f32 = PAGE_WIDTH - MARGIN_X;
    pub const SIZE: f32 = 8.0;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every character is 5 points wide at every size: enough to reason about wrapping
    /// exactly, without a font.
    struct Fixed;
    impl Measure for Fixed {
        fn width(&self, text: &str, _size: f32, _bold: bool) -> f32 {
            text.chars().count() as f32 * 5.0
        }
    }

    fn texts(page: &Page) -> Vec<&str> {
        page.items
            .iter()
            .filter_map(|item| match item {
                Item::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Text wraps at word boundaries within its width, and a word wider than the whole line
    /// is broken rather than left to overrun its column.
    #[test]
    fn text_wraps_within_its_width() {
        assert_eq!(wrap(&Fixed, "one two three", BODY_SIZE, false, 40.0), vec!["one two", "three"]);
        let broken = wrap(&Fixed, "abcdefghij", BODY_SIZE, false, 20.0);
        assert_eq!(broken, vec!["abcd", "efgh", "ij"]);
        assert!(broken.iter().all(|line| Fixed.width(line, BODY_SIZE, false) <= 20.0));
        assert_eq!(wrap(&Fixed, "a\nb", BODY_SIZE, false, 100.0), vec!["a", "b"], "newlines are kept");
    }

    /// Short columns keep their width when a long one has to be squeezed, and the widths
    /// always fill the table exactly.
    #[test]
    fn a_long_column_is_squeezed_before_a_short_one() {
        let header = vec!["Slot".to_string(), "Tool".to_string()];
        let rows = vec![vec!["T1".to_string(), "x".repeat(200)]];
        let widths = column_widths(&Fixed, &header, &rows, 300.0);
        assert!((widths.iter().sum::<f32>() - 300.0).abs() < 0.01, "{widths:?}");
        assert!((widths[0] - (20.0 + 2.0 * CELL_PAD)).abs() < 0.01, "Slot keeps its width: {widths:?}");
    }

    /// A table that runs past the bottom of the page carries on overleaf under its header,
    /// and no row is ever split between two pages.
    #[test]
    fn a_long_table_repeats_its_header_and_never_splits_a_row() {
        let rows: Vec<Vec<String>> = (0..120).map(|n| vec![format!("T{n}"), format!("tool {n}")]).collect();
        let pages = layout(&Fixed, &[Block::Table { header: vec!["Slot".into(), "Tool".into()], rows }]);

        assert!(pages.len() > 1, "120 rows do not fit on one page");
        for page in &pages {
            assert_eq!(texts(page).first(), Some(&"Slot"), "each page starts with the header");
            for item in &page.items {
                if let Item::Text { y, .. } = item {
                    assert!(*y <= BODY_BOTTOM, "a line was placed past the bottom margin");
                }
            }
        }
        let rows_placed = pages.iter().flat_map(texts).filter(|t| t.starts_with("tool ")).count();
        assert_eq!(rows_placed, 120, "every row is placed exactly once");
    }

    /// Each step's section starts a page, but a page break on an empty page does not leave a
    /// blank one behind.
    #[test]
    fn a_page_break_starts_a_page_but_never_a_blank_one() {
        let blocks = vec![
            Block::PageBreak,
            Block::Paragraph("first".into()),
            Block::PageBreak,
            Block::Paragraph("second".into()),
        ];
        let pages = layout(&Fixed, &blocks);
        assert_eq!(pages.len(), 2);
        assert_eq!(texts(&pages[0]), vec!["first"]);
        assert_eq!(texts(&pages[1]), vec!["second"]);
    }

    /// A heading is never left alone at the foot of a page with its content overleaf.
    #[test]
    fn a_heading_keeps_with_what_follows() {
        let filler: Vec<String> = (0..200).map(|n| format!("line {n}")).collect();
        let mut blocks: Vec<Block> = Vec::new();
        // Fill the page until only a sliver is left, then a heading and its paragraph.
        let lines_per_page = ((BODY_BOTTOM - BODY_TOP) / (BODY_SIZE * LEADING)) as usize;
        blocks.push(Block::Paragraph(filler[..lines_per_page - 2].join("\n")));
        blocks.push(Block::Heading("Heading".into()));
        blocks.push(Block::Paragraph("body".into()));
        let pages = layout(&Fixed, &blocks);

        let page_of = |needle: &str| pages.iter().position(|p| texts(p).contains(&needle));
        assert_eq!(page_of("Heading"), page_of("body"), "the heading went with its text");
    }
}

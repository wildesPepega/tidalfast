//! A small Markdown renderer for egui, for the release notes in the update dialog. It shows what
//! GitHub shows: headings, paragraphs, bold, italic, strikethrough, inline code, code blocks, bullet,
//! numbered and task lists, quotes, links, rules and tables. Pictures are shown as their alt text and
//! raw HTML is left out.

use eframe::egui::{self, Color32, RichText};
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::ui::{self, accent, TEXT, TEXT_DIM};

// ---------------------------------------------------------------------------- the parsed text

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Inline {
    Text(String, Style),
    Link { runs: Vec<(String, Style)>, url: String },
    Break,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// `Some(done)` for a task list entry.
    pub task: Option<bool>,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Heading(u8, Vec<Inline>),
    Paragraph(Vec<Inline>),
    Code(String),
    /// `Some(first number)` for a numbered list.
    List(Option<u64>, Vec<Item>),
    Quote(Vec<Block>),
    Rule,
    Table { head: Vec<Vec<Inline>>, rows: Vec<Vec<Vec<Inline>>> },
}

/// Turn Markdown into blocks.
pub fn parse(markdown: &str) -> Vec<Block> {
    let options = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS;
    let mut blocks: Vec<Vec<Block>> = vec![Vec::new()];
    let mut inline: Vec<Inline> = Vec::new();
    let mut style = Style::default();
    let mut bold_depth = 0;
    let mut italic_depth = 0;
    let mut strike_depth = 0;
    let mut link: Option<(String, Vec<(String, Style)>)> = None;
    let mut code: Option<String> = None;
    let mut lists: Vec<(Option<u64>, Vec<Item>)> = Vec::new();
    let mut tasks: Vec<Option<bool>> = Vec::new();
    // tables
    let mut table: Option<(Vec<Vec<Inline>>, Vec<Vec<Vec<Inline>>>)> = None;
    let mut row: Vec<Vec<Inline>> = Vec::new();
    let mut in_head = false;

    // text that stands loose (inside a tight list item) becomes a paragraph before the next block
    fn flush(inline: &mut Vec<Inline>, blocks: &mut [Vec<Block>]) {
        if !inline.is_empty() {
            if let Some(top) = blocks.last_mut() {
                top.push(Block::Paragraph(std::mem::take(inline)));
            }
        }
    }

    for event in Parser::new_ext(markdown, options) {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph | Tag::TableCell => {}
                Tag::Heading { .. } => flush(&mut inline, &mut blocks),
                Tag::BlockQuote(_) => {
                    flush(&mut inline, &mut blocks);
                    blocks.push(Vec::new());
                }
                Tag::CodeBlock(_) => {
                    flush(&mut inline, &mut blocks);
                    code = Some(String::new());
                }
                Tag::List(start) => {
                    flush(&mut inline, &mut blocks);
                    lists.push((start, Vec::new()));
                }
                Tag::Item => {
                    blocks.push(Vec::new());
                    tasks.push(None);
                }
                Tag::Emphasis => italic_depth += 1,
                Tag::Strong => bold_depth += 1,
                Tag::Strikethrough => strike_depth += 1,
                Tag::Link { dest_url, .. } => link = Some((dest_url.to_string(), Vec::new())),
                Tag::Table(_) => {
                    flush(&mut inline, &mut blocks);
                    table = Some((Vec::new(), Vec::new()));
                }
                Tag::TableHead => {
                    in_head = true;
                    row.clear();
                }
                Tag::TableRow => row.clear(),
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => {
                    if let Some(top) = blocks.last_mut() {
                        top.push(Block::Paragraph(std::mem::take(&mut inline)));
                    }
                }
                TagEnd::Heading(level) => {
                    let n = match level {
                        HeadingLevel::H1 => 1,
                        HeadingLevel::H2 => 2,
                        HeadingLevel::H3 => 3,
                        HeadingLevel::H4 => 4,
                        HeadingLevel::H5 => 5,
                        HeadingLevel::H6 => 6,
                    };
                    if let Some(top) = blocks.last_mut() {
                        top.push(Block::Heading(n, std::mem::take(&mut inline)));
                    }
                }
                TagEnd::BlockQuote(_) => {
                    flush(&mut inline, &mut blocks);
                    let inner = blocks.pop().unwrap_or_default();
                    if let Some(top) = blocks.last_mut() {
                        top.push(Block::Quote(inner));
                    }
                }
                TagEnd::CodeBlock => {
                    if let (Some(text), Some(top)) = (code.take(), blocks.last_mut()) {
                        top.push(Block::Code(text.trim_end_matches('\n').to_string()));
                    }
                }
                TagEnd::Item => {
                    flush(&mut inline, &mut blocks);
                    let inner = blocks.pop().unwrap_or_default();
                    let task = tasks.pop().flatten();
                    if let Some(list) = lists.last_mut() {
                        list.1.push(Item { task, blocks: inner });
                    }
                }
                TagEnd::List(_) => {
                    if let (Some((start, items)), Some(top)) = (lists.pop(), blocks.last_mut()) {
                        top.push(Block::List(start, items));
                    }
                }
                TagEnd::Emphasis => italic_depth -= 1,
                TagEnd::Strong => bold_depth -= 1,
                TagEnd::Strikethrough => strike_depth -= 1,
                TagEnd::Link => {
                    if let Some((url, runs)) = link.take() {
                        inline.push(Inline::Link { runs, url });
                    }
                }
                TagEnd::TableCell => row.push(std::mem::take(&mut inline)),
                TagEnd::TableHead => {
                    in_head = false;
                    if let Some(t) = table.as_mut() {
                        t.0 = std::mem::take(&mut row);
                    }
                }
                TagEnd::TableRow => {
                    if !in_head {
                        if let Some(t) = table.as_mut() {
                            t.1.push(std::mem::take(&mut row));
                        }
                    }
                }
                TagEnd::Table => {
                    if let (Some((head, rows)), Some(top)) = (table.take(), blocks.last_mut()) {
                        top.push(Block::Table { head, rows });
                    }
                }
                _ => {}
            },
            Event::Text(text) => {
                if let Some(code) = code.as_mut() {
                    code.push_str(&text);
                } else {
                    style = Style { bold: bold_depth > 0, italic: italic_depth > 0, strike: strike_depth > 0, code: false };
                    match link.as_mut() {
                        Some((_, runs)) => runs.push((text.to_string(), style)),
                        None => inline.push(Inline::Text(text.to_string(), style)),
                    }
                }
            }
            Event::Code(text) => {
                let s = Style { bold: bold_depth > 0, italic: italic_depth > 0, strike: strike_depth > 0, code: true };
                match link.as_mut() {
                    Some((_, runs)) => runs.push((text.to_string(), s)),
                    None => inline.push(Inline::Text(text.to_string(), s)),
                }
            }
            Event::SoftBreak => inline.push(Inline::Text(" ".into(), style)),
            Event::HardBreak => inline.push(Inline::Break),
            Event::Rule => {
                flush(&mut inline, &mut blocks);
                if let Some(top) = blocks.last_mut() {
                    top.push(Block::Rule);
                }
            }
            Event::TaskListMarker(done) => {
                if let Some(task) = tasks.last_mut() {
                    *task = Some(done);
                }
            }
            _ => {}
        }
    }
    flush(&mut inline, &mut blocks);
    blocks.into_iter().next().unwrap_or_default()
}

// ---------------------------------------------------------------------------- drawing

/// Body text: a little dimmer than the headings.
fn body_color() -> Color32 {
    ui::lerp_color(TEXT, TEXT_DIM, 0.28)
}

fn run(text: &str, style: Style, size: f32, base: Color32) -> RichText {
    let mut t = RichText::new(text).size(size).color(if style.bold { TEXT } else { base });
    if style.bold {
        t = t.strong();
    }
    if style.italic {
        t = t.italics();
    }
    if style.strike {
        t = t.strikethrough();
    }
    if style.code {
        t = t.monospace().size(size - 0.5).background_color(ui::with_alpha(Color32::WHITE, 20)).color(TEXT);
    }
    t
}

/// Characters the app's font does not have, replaced by look-alikes it does.
fn printable(text: &str) -> String {
    text.replace('→', "›").replace('←', "‹").replace('↑', "^").replace('↓', "v")
}

/// The app's font has no bold weight, so bold text is drawn twice, a hair apart.
fn bold_label(ui: &mut egui::Ui, text: &RichText) -> egui::Response {
    let galley = egui::WidgetText::from(text.clone()).into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Body);
    let (rect, resp) = ui.allocate_exact_size(galley.size() + egui::vec2(0.6, 0.0), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let color = ui.visuals().text_color();
        ui.painter().galley(rect.min, galley.clone(), color);
        ui.painter().galley(rect.min + egui::vec2(0.6, 0.0), galley, color);
    }
    resp
}

/// `wrap`: the text may wrap over several lines (paragraphs); table cells stay on one line.
fn inlines(ui: &mut egui::Ui, items: &[Inline], size: f32, base: Color32, wrap: bool) {
    let layout = |ui: &mut egui::Ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for item in items {
            match item {
                Inline::Text(text, style) => {
                    if style.bold && !style.code {
                        bold_label(ui, &run(&printable(text), *style, size, base));
                    } else {
                        ui.label(run(&printable(text), *style, size, base));
                    }
                }
                Inline::Link { runs, url } => {
                    for (text, style) in runs {
                        let label = run(&printable(text), *style, size, accent()).color(accent()).underline();
                        let resp = ui.add(egui::Link::new(label));
                        if resp.clicked() && (url.starts_with("https://") || url.starts_with("http://")) {
                            crate::app::open_url(url);
                        }
                        resp.on_hover_text(url);
                    }
                }
                Inline::Break => ui.end_row(),
            }
        }
    };
    if wrap {
        ui.horizontal_wrapped(layout);
    } else {
        ui.horizontal(layout);
    }
}

fn blocks(ui: &mut egui::Ui, items: &[Block], id: egui::Id) {
    for (i, block) in items.iter().enumerate() {
        let id = id.with(i);
        match block {
            Block::Heading(level, content) => {
                let size = match level {
                    1 => 20.0,
                    2 => 17.0,
                    3 => 15.0,
                    _ => 13.5,
                };
                if i > 0 {
                    ui.add_space(8.0);
                }
                let bold: Vec<Inline> = content
                    .iter()
                    .map(|c| match c {
                        Inline::Text(t, s) => Inline::Text(t.clone(), Style { bold: true, ..*s }),
                        other => other.clone(),
                    })
                    .collect();
                inlines(ui, &bold, size, TEXT, true);
                if *level <= 2 {
                    // GitHub draws a thin line under the big headings
                    ui.add_space(2.0);
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
                    ui.painter().rect_filled(rect, 0.0, ui::with_alpha(TEXT, 30));
                }
                ui.add_space(4.0);
            }
            Block::Paragraph(content) => {
                inlines(ui, content, 13.0, body_color(), true);
                if i + 1 < items.len() {
                    ui.add_space(6.0);
                }
            }
            Block::Code(text) => {
                egui::Frame::new()
                    .fill(ui::with_alpha(Color32::BLACK, 70))
                    .corner_radius(6.0)
                    .inner_margin(egui::Margin::symmetric(10, 8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        egui::ScrollArea::horizontal().id_salt(id).show(ui, |ui| {
                            ui.add(egui::Label::new(RichText::new(printable(text)).monospace().size(12.0).color(TEXT)).extend());
                        });
                    });
                ui.add_space(6.0);
            }
            Block::List(start, entries) => {
                ui.spacing_mut().item_spacing.y = 3.0;
                for (n, entry) in entries.iter().enumerate() {
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        ui.add_space(6.0);
                        let marker = match (entry.task, start) {
                            (Some(true), _) => egui_phosphor::regular::CHECK_SQUARE.to_string(),
                            (Some(false), _) => egui_phosphor::regular::SQUARE.to_string(),
                            (None, Some(first)) => format!("{}.", first + n as u64),
                            (None, None) => "•".to_string(),
                        };
                        let color = if entry.task == Some(true) { accent() } else { TEXT_DIM };
                        let size = if marker == "•" { 17.0 } else { 13.0 };
                        ui.add_sized([18.0, 18.0], egui::Label::new(RichText::new(marker).size(size).color(color)));
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            blocks(ui, &entry.blocks, id.with(n));
                        });
                    });
                }
                ui.add_space(4.0);
            }
            Block::Quote(inner) => {
                let resp = ui.horizontal_top(|ui| {
                    ui.add_space(12.0);
                    ui.vertical(|ui| blocks(ui, inner, id));
                });
                let rect = resp.response.rect;
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(rect.min, egui::vec2(3.0, rect.height())),
                    1.5,
                    ui::with_alpha(accent(), 140),
                );
            }
            Block::Rule => {
                ui.add_space(4.0);
                let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 0.0, ui::with_alpha(TEXT, 40));
                ui.add_space(6.0);
            }
            Block::Table { head, rows } => {
                egui::Grid::new(id).striped(true).spacing([14.0, 5.0]).show(ui, |ui| {
                    for cell in head {
                        let bold: Vec<Inline> = cell
                            .iter()
                            .map(|c| match c {
                                Inline::Text(t, s) => Inline::Text(t.clone(), Style { bold: true, ..*s }),
                                other => other.clone(),
                            })
                            .collect();
                        inlines(ui, &bold, 12.5, TEXT, false);
                    }
                    ui.end_row();
                    for row in rows {
                        for cell in row {
                            inlines(ui, cell, 12.5, body_color(), false);
                        }
                        ui.end_row();
                    }
                });
                ui.add_space(6.0);
            }
        }
    }
}

/// Draw Markdown into the space that is left.
pub fn show(ui: &mut egui::Ui, markdown: &str) {
    let parsed = parse(markdown);
    blocks(ui, &parsed, egui::Id::new("markdown"));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(b: &Block) -> String {
        match b {
            Block::Heading(_, c) | Block::Paragraph(c) => c
                .iter()
                .map(|i| match i {
                    Inline::Text(t, _) => t.clone(),
                    Inline::Link { runs, .. } => runs.iter().map(|(t, _)| t.as_str()).collect(),
                    Inline::Break => "\n".into(),
                })
                .collect(),
            _ => String::new(),
        }
    }

    #[test]
    fn headings_paragraphs_and_styles() {
        let b = parse("## New\n\nSome **bold**, *slanted*, ~~gone~~ and `code`.");
        assert!(matches!(&b[0], Block::Heading(2, _)));
        assert_eq!(text(&b[0]), "New");
        let Block::Paragraph(p) = &b[1] else { panic!("{b:?}") };
        assert_eq!(text(&b[1]), "Some bold, slanted, gone and code.");
        assert!(p.iter().any(|i| matches!(i, Inline::Text(t, s) if t == "bold" && s.bold)));
        assert!(p.iter().any(|i| matches!(i, Inline::Text(t, s) if t == "slanted" && s.italic)));
        assert!(p.iter().any(|i| matches!(i, Inline::Text(t, s) if t == "gone" && s.strike)));
        assert!(p.iter().any(|i| matches!(i, Inline::Text(t, s) if t == "code" && s.code)));
    }

    #[test]
    fn lists_nest_and_task_items_are_marked() {
        let b = parse("- one\n- two\n  - inner\n\n1. first\n2. second\n\n- [x] done\n- [ ] open");
        let Block::List(None, items) = &b[0] else { panic!("{b:?}") };
        assert_eq!(items.len(), 2);
        assert!(items[1].blocks.iter().any(|x| matches!(x, Block::List(None, inner) if inner.len() == 1)));
        assert!(matches!(&b[1], Block::List(Some(1), items) if items.len() == 2));
        let Block::List(None, tasks) = &b[2] else { panic!("{b:?}") };
        assert_eq!(tasks[0].task, Some(true));
        assert_eq!(tasks[1].task, Some(false));
    }

    #[test]
    fn links_code_blocks_quotes_rules_and_tables() {
        let b = parse("See [the page](https://example.com/x).\n\n```rust\nlet a = 1;\n```\n\n> quoted\n\n---\n\n| a | b |\n|---|---|\n| 1 | 2 |\n");
        let Block::Paragraph(p) = &b[0] else { panic!("{b:?}") };
        assert!(p.iter().any(|i| matches!(i, Inline::Link { url, runs } if url == "https://example.com/x" && runs[0].0 == "the page")));
        assert_eq!(b[1], Block::Code("let a = 1;".into()));
        assert!(matches!(&b[2], Block::Quote(inner) if inner.len() == 1));
        assert_eq!(b[3], Block::Rule);
        let Block::Table { head, rows } = &b[4] else { panic!("{b:?}") };
        assert_eq!(head.len(), 2);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].len(), 2);
    }

    #[test]
    fn plain_text_and_empty_input_are_fine() {
        assert!(parse("").is_empty());
        let b = parse("just text");
        assert_eq!(text(&b[0]), "just text");
    }
}

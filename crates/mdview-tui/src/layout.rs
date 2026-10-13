//! Turns a parsed document into terminal lines for a given width.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use image::RgbaImage;
use mdview_core::{Align, Block, Document, Inline, ListItem, Table};
use mdview_render::{Palette, Registry};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::graphics::{half_blocks, hash_of, Graphics};
use crate::theme::Theme;

/// Pixels per terminal cell assumed when sizing images. Terminals inside a
/// multiplexer do not report their real cell size.
pub const DEFAULT_CELL: (u32, u32) = (10, 20);
/// Diagrams are rasterized at this many pixels per logical pixel so they stay
/// sharp on high-density screens.
const DENSITY: f32 = 2.0;
/// Shown on every code block; a click anywhere on the block copies it.
const COPY_BUTTON: &str = "[ copy ] ";

/// Clickable range of a line.
#[derive(Debug, Clone)]
pub struct LinkHit {
    pub start: u16,
    pub end: u16,
    pub link: usize,
}

/// Marks a line as one row of an image drawn by the terminal.
#[derive(Debug, Clone, Copy)]
pub struct ImageCell {
    pub slot: usize,
    pub row: u16,
    pub col: u16,
}

#[derive(Debug, Clone, Default)]
pub struct RLine {
    pub spans: Vec<Span<'static>>,
    pub links: Vec<LinkHit>,
    pub image: Option<ImageCell>,
    /// Index into [`Rendered::codes`] when the line belongs to a code block.
    pub code: Option<usize>,
}

impl RLine {
    fn from_spans(spans: Vec<Span<'static>>) -> Self {
        RLine {
            spans,
            ..RLine::default()
        }
    }

    pub fn width(&self) -> u16 {
        self.spans.iter().map(|s| s.content.width() as u16).sum()
    }

    pub fn text(&self) -> String {
        self.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// Puts `spans` in front of the line, shifting everything positioned.
    fn prefix(&mut self, spans: &[Span<'static>]) {
        let shift: u16 = spans.iter().map(|s| s.content.width() as u16).sum();
        for hit in &mut self.links {
            hit.start += shift;
            hit.end += shift;
        }
        if let Some(cell) = &mut self.image {
            cell.col += shift;
        }
        self.spans.splice(0..0, spans.iter().cloned());
    }

    fn pad_to(&mut self, width: u16, style: Style) {
        let current = self.width();
        if current < width {
            self.spans
                .push(Span::styled(" ".repeat((width - current) as usize), style));
        }
    }
}

pub struct ImageSlot {
    pub key: u64,
    pub image: Arc<RgbaImage>,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Default)]
pub struct Rendered {
    pub lines: Vec<RLine>,
    pub images: Vec<ImageSlot>,
    pub links: Vec<String>,
    /// Heading id to line number.
    pub anchors: HashMap<String, usize>,
    /// Level, title and line of each heading.
    pub toc: Vec<(u8, String, usize)>,
    /// Source of every code block, for copying.
    pub codes: Vec<String>,
}

/// Long-lived rendering resources: engines, syntax definitions and the
/// cache of drawn pictures, kept across reloads and resizes.
pub struct Assets {
    registry: Registry,
    syntaxes: SyntaxSet,
    themes: ThemeSet,
    pictures: HashMap<u64, Result<Arc<RgbaImage>, String>>,
}

impl Default for Assets {
    fn default() -> Self {
        Assets {
            registry: Registry::default(),
            syntaxes: SyntaxSet::load_defaults_newlines(),
            themes: ThemeSet::load_defaults(),
            pictures: HashMap::new(),
        }
    }
}

impl Assets {
    fn picture(
        &mut self,
        key: u64,
        draw: impl FnOnce(&Registry) -> anyhow::Result<RgbaImage>,
    ) -> Result<Arc<RgbaImage>, String> {
        if let Some(cached) = self.pictures.get(&key) {
            return cached.clone();
        }
        let drawn = draw(&self.registry)
            .map(Arc::new)
            .map_err(|e| format!("{e:#}"));
        self.pictures.insert(key, drawn.clone());
        drawn
    }
}

pub struct LayoutOptions<'a> {
    pub width: u16,
    pub theme: &'a Theme,
    pub graphics: Graphics,
    pub cell: (u32, u32),
    /// Directory relative links and images resolve against.
    pub base_dir: &'a Path,
}

pub fn layout(doc: &Document, options: &LayoutOptions, assets: &mut Assets) -> Rendered {
    let mut layouter = Layouter {
        options,
        assets,
        out: Rendered::default(),
    };
    let mut lines = Vec::new();
    if let Some(frontmatter) = &doc.frontmatter {
        lines.extend(layouter.code_block("yaml", frontmatter, options.width, Some("front matter")));
        lines.push(RLine::default());
    }
    layouter.top_level(&doc.blocks, &mut lines);
    if !doc.footnotes.is_empty() {
        lines.push(RLine::default());
        lines.push(layouter.rule(options.width));
        for note in &doc.footnotes {
            let label = format!("[{}] ", note.label);
            let indent = label.width() as u16;
            let mut body =
                layouter.blocks(&note.blocks, options.width.saturating_sub(indent), true);
            let marker = [Span::styled(label, Style::new().fg(options.theme.link))];
            let blank = [Span::raw(" ".repeat(indent as usize))];
            for (i, line) in body.iter_mut().enumerate() {
                line.prefix(if i == 0 { &marker } else { &blank });
            }
            lines.extend(body);
        }
    }
    layouter.out.lines = lines;
    layouter.out
}

#[derive(Clone)]
struct Piece {
    text: String,
    style: Style,
    link: Option<usize>,
}

enum Item {
    Piece(Piece),
    Break,
}

struct Layouter<'a> {
    options: &'a LayoutOptions<'a>,
    assets: &'a mut Assets,
    out: Rendered,
}

impl Layouter<'_> {
    fn theme(&self) -> &Theme {
        self.options.theme
    }

    fn palette(&self) -> Palette {
        Palette {
            dark: self.theme().dark,
            foreground: self.theme().text,
        }
    }

    /// Top-level blocks, recording where each heading lands.
    fn top_level(&mut self, blocks: &[Block], lines: &mut Vec<RLine>) {
        for (i, block) in blocks.iter().enumerate() {
            if i > 0 {
                lines.push(RLine::default());
            }
            if let Block::Heading { level, id, content } = block {
                self.out.anchors.insert(id.clone(), lines.len());
                self.out
                    .toc
                    .push((*level, mdview_core::plain_text(content), lines.len()));
            }
            lines.extend(self.block(block, self.options.width));
        }
    }

    fn blocks(&mut self, blocks: &[Block], width: u16, compact: bool) -> Vec<RLine> {
        let mut lines = Vec::new();
        for (i, block) in blocks.iter().enumerate() {
            if i > 0 && !(compact && matches!(block, Block::List { .. })) {
                lines.push(RLine::default());
            }
            lines.extend(self.block(block, width));
        }
        lines
    }

    fn block(&mut self, block: &Block, width: u16) -> Vec<RLine> {
        let width = width.max(8);
        match block {
            Block::Heading { level, content, .. } => self.heading(*level, content, width),
            Block::Paragraph(inlines) => self.paragraph(inlines, width),
            Block::CodeBlock { lang, code } => {
                if self.assets.registry.find(lang).is_some() {
                    self.diagram(lang, code, width)
                } else if matches!(lang.as_str(), "math" | "latex" | "tex" | "katex") {
                    self.display_math(code, width)
                } else {
                    self.code_block(lang, code, width, None)
                }
            }
            Block::Math(tex) => self.display_math(tex, width),
            Block::Quote { alert, blocks } => {
                let (color, title) = match alert {
                    Some(alert) => {
                        let (color, icon) = self.theme().alert(*alert);
                        (color, Some(format!("{icon} {}", alert.label())))
                    }
                    None => (self.theme().border, None),
                };
                let mut lines = Vec::new();
                if let Some(title) = title {
                    lines.push(RLine::from_spans(vec![Span::styled(
                        title,
                        Style::new().fg(color).add_modifier(Modifier::BOLD),
                    )]));
                }
                let mut body = self.blocks(blocks, width.saturating_sub(2), false);
                if alert.is_none() {
                    let quote = self.theme().quote;
                    for line in &mut body {
                        for span in &mut line.spans {
                            if span.style.fg.is_none() {
                                span.style = span.style.fg(quote);
                            }
                        }
                    }
                }
                lines.extend(body);
                let bar = [Span::styled("▌ ", Style::new().fg(color))];
                for line in &mut lines {
                    line.prefix(&bar);
                }
                lines
            }
            Block::List { start, items } => self.list(*start, items, width),
            Block::Table(table) => self.table(table, width),
            Block::DefinitionList(defs) => {
                let mut lines = Vec::new();
                for def in defs {
                    let mut items = Vec::new();
                    self.inline_items(
                        &def.term,
                        Style::new().add_modifier(Modifier::BOLD),
                        None,
                        &mut items,
                    );
                    lines.extend(wrap(items, width));
                    for detail in &def.details {
                        let mut body = self.blocks(detail, width.saturating_sub(4), true);
                        let first = [Span::styled("  : ", Style::new().fg(self.theme().dim))];
                        let rest = [Span::raw("    ")];
                        for (i, line) in body.iter_mut().enumerate() {
                            line.prefix(if i == 0 { &first } else { &rest });
                        }
                        lines.extend(body);
                    }
                }
                lines
            }
            Block::Rule => vec![self.rule(width)],
            Block::Html(html) => self.html(html, width),
        }
    }

    fn rule(&self, width: u16) -> RLine {
        RLine::from_spans(vec![Span::styled(
            "─".repeat(width as usize),
            Style::new().fg(self.theme().border),
        )])
    }

    fn heading(&mut self, level: u8, content: &[Inline], width: u16) -> Vec<RLine> {
        let color = self.theme().headings[(level.clamp(1, 6) - 1) as usize];
        // Levels 1 and 2 get a rule below; deeper ones are told apart by
        // colour, and by italics from level 4 down.
        let mut style = Style::new().fg(color).add_modifier(Modifier::BOLD);
        if level >= 4 {
            style = style.add_modifier(Modifier::ITALIC);
        }
        let mut items = Vec::new();
        self.inline_items(content, style, None, &mut items);
        let mut lines = wrap(items, width);
        let underline = match level {
            1 => Some("━"),
            2 => Some("─"),
            _ => None,
        };
        if let Some(glyph) = underline {
            lines.push(RLine::from_spans(vec![Span::styled(
                glyph.repeat(width as usize),
                Style::new().fg(color).add_modifier(Modifier::DIM),
            )]));
        }
        lines
    }

    fn paragraph(&mut self, inlines: &[Inline], width: u16) -> Vec<RLine> {
        // A paragraph that is only pictures (optionally linked, like badges
        // and screenshots) is laid out as block images.
        let pictures: Vec<(&str, &str)> = inlines
            .iter()
            .filter_map(|inline| match inline {
                Inline::Image { url, alt, .. } => Some((url.as_str(), alt.as_str())),
                Inline::Link { content, .. } => match content.as_slice() {
                    [Inline::Image { url, alt, .. }] => Some((url.as_str(), alt.as_str())),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        let only_pictures = !pictures.is_empty()
            && inlines.iter().all(|inline| match inline {
                Inline::Image { .. } | Inline::SoftBreak | Inline::HardBreak => true,
                Inline::Link { content, .. } => {
                    matches!(content.as_slice(), [Inline::Image { .. }])
                }
                Inline::Text(t) => t.trim().is_empty(),
                _ => false,
            });
        if only_pictures {
            let local: Vec<_> = pictures
                .iter()
                .filter_map(|(url, alt)| self.local_path(url).map(|p| (p, *alt)))
                .collect();
            if local.len() == pictures.len() {
                let mut lines = Vec::new();
                for (path, alt) in local {
                    lines.extend(self.image_file(&path, alt, width));
                }
                return lines;
            }
        }
        let mut items = Vec::new();
        self.inline_items(inlines, Style::new(), None, &mut items);
        wrap(items, width)
    }

    /// Resolves a link target to a file on disk, if it is one.
    fn local_path(&self, url: &str) -> Option<PathBuf> {
        resolve_local(self.options.base_dir, url).filter(|p| p.is_file())
    }

    fn image_file(&mut self, path: &Path, alt: &str, width: u16) -> Vec<RLine> {
        // The modification time is part of the key so an edited image is
        // read again.
        let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        let key = hash_of(("file", path, modified));
        let owned = path.to_path_buf();
        let is_svg = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("svg"));
        let density = if is_svg { DENSITY } else { 1.0 };
        match self
            .assets
            .picture(key, |_| mdview_render::load_image(&owned, density))
        {
            Ok(image) => {
                let mut lines = self.picture_lines(key, image, density, width, false);
                if !alt.is_empty() && self.options.graphics == Graphics::Text {
                    lines.push(RLine::from_spans(vec![Span::styled(
                        alt.to_string(),
                        Style::new()
                            .fg(self.theme().dim)
                            .add_modifier(Modifier::ITALIC),
                    )]));
                }
                lines
            }
            Err(error) => vec![self.error_line(&format!("image: {error}"))],
        }
    }

    fn error_line(&self, message: &str) -> RLine {
        RLine::from_spans(vec![Span::styled(
            format!("⚠ {}", message.lines().next().unwrap_or(message)),
            Style::new().fg(self.theme().error),
        )])
    }

    /// Lays a picture out as terminal rows, drawn by the terminal or as half
    /// blocks depending on the graphics mode.
    fn picture_lines(
        &mut self,
        key: u64,
        image: Arc<RgbaImage>,
        density: f32,
        width: u16,
        centered: bool,
    ) -> Vec<RLine> {
        let (cell_w, cell_h) = self.options.cell;
        let logical_width = image.width() as f32 / density;
        // Half blocks are far coarser than pixels, so text-mode pictures get
        // twice the room to stay recognisable.
        let zoom = if self.options.graphics == Graphics::Text {
            2.0
        } else {
            1.0
        };
        let cols = ((logical_width * zoom / cell_w as f32).ceil() as u16).clamp(1, width);
        let aspect = image.height() as f32 / image.width().max(1) as f32;
        let rows = ((cols as f32 * cell_w as f32 * aspect / cell_h as f32).ceil() as u16).max(1);
        let indent = if centered { (width - cols) / 2 } else { 0 };
        let pad = [Span::raw(" ".repeat(indent as usize))];
        match self.options.graphics {
            Graphics::Kitty => {
                let slot = self.out.images.len();
                self.out.images.push(ImageSlot {
                    key,
                    image,
                    cols,
                    rows,
                });
                (0..rows)
                    .map(|row| RLine {
                        image: Some(ImageCell {
                            slot,
                            row,
                            col: indent,
                        }),
                        ..RLine::default()
                    })
                    .collect()
            }
            Graphics::Text => half_blocks(&image, cols, rows, self.theme().background)
                .into_iter()
                .map(|spans| {
                    let mut line = RLine::from_spans(spans);
                    line.prefix(&pad);
                    line
                })
                .collect(),
        }
    }

    fn diagram(&mut self, lang: &str, source: &str, width: u16) -> Vec<RLine> {
        let palette = self.palette();
        if self.options.graphics == Graphics::Text {
            let text = self
                .assets
                .registry
                .find(lang)
                .and_then(|engine| engine.to_text(source, width as usize));
            match text {
                Some(Ok(text)) => {
                    let style = Style::new().fg(self.theme().diagram);
                    let widest = text.lines().map(|l| l.width() as u16).max().unwrap_or(0);
                    let indent = " ".repeat((width.saturating_sub(widest) / 2) as usize);
                    return text
                        .lines()
                        .map(|line| {
                            RLine::from_spans(vec![Span::styled(
                                truncate(&format!("{indent}{line}"), width as usize),
                                style,
                            )])
                        })
                        .collect();
                }
                Some(Err(error)) => {
                    let mut lines = vec![self.error_line(&format!("{lang}: {error}"))];
                    lines.extend(self.code_block(lang, source, width, None));
                    return lines;
                }
                None => {}
            }
        }
        let key = hash_of(("diagram", lang, source, palette));
        let drawn = self.assets.picture(key, |registry| {
            let engine = registry
                .find(lang)
                .ok_or_else(|| anyhow::anyhow!("no engine for {lang}"))?;
            mdview_render::rasterize_svg(&engine.to_svg(source, palette)?, DENSITY)
        });
        match drawn {
            Ok(image) => {
                let mut lines = self.picture_lines(key, image, DENSITY, width, true);
                if self.options.graphics == Graphics::Text {
                    lines.push(RLine::from_spans(vec![Span::styled(
                        format!("{lang} diagram · press o to see it sharp in the browser"),
                        Style::new()
                            .fg(self.theme().dim)
                            .add_modifier(Modifier::ITALIC),
                    )]));
                }
                lines
            }
            Err(error) => {
                let mut lines = vec![self.error_line(&format!("{lang}: {error}"))];
                lines.extend(self.code_block(lang, source, width, None));
                lines
            }
        }
    }

    fn display_math(&mut self, tex: &str, width: u16) -> Vec<RLine> {
        if self.options.graphics == Graphics::Kitty {
            let palette = self.palette();
            let em = self.options.cell.1 as f32 * DENSITY;
            let key = hash_of(("math", tex, palette, self.options.cell));
            let drawn = self.assets.picture(key, |_| {
                mdview_render::math_to_image(tex, true, palette, em)
            });
            if let Ok(image) = drawn {
                return self.picture_lines(key, image, DENSITY, width, true);
            }
        }
        let style = Style::new().fg(self.theme().math);
        mdview_render::math_to_unicode(tex)
            .lines()
            .flat_map(|line| {
                let items = vec![Item::Piece(Piece {
                    text: line.to_string(),
                    style,
                    link: None,
                })];
                wrap(items, width)
            })
            .map(|mut line| {
                let indent = (width.saturating_sub(line.width()) / 2) as usize;
                line.prefix(&[Span::raw(" ".repeat(indent))]);
                line
            })
            .collect()
    }

    fn code_block(
        &mut self,
        lang: &str,
        code: &str,
        width: u16,
        label: Option<&str>,
    ) -> Vec<RLine> {
        let theme = self.theme().clone();
        let background = Style::new().bg(theme.code_bg);
        let inner = width.saturating_sub(2).max(1) as usize;
        let mut lines = Vec::new();
        // Header: the language on the left, a copy button on the right.
        let label = truncate(
            &format!(" {}", label.unwrap_or(lang)),
            (width as usize).saturating_sub(COPY_BUTTON.width()),
        );
        let mut header = RLine::from_spans(vec![Span::styled(
            label,
            background.fg(theme.dim).add_modifier(Modifier::ITALIC),
        )]);
        header.pad_to(width.saturating_sub(COPY_BUTTON.width() as u16), background);
        header
            .spans
            .push(Span::styled(COPY_BUTTON, background.fg(theme.link)));
        header.pad_to(width, background);
        lines.push(header);
        let syntax = self
            .assets
            .syntaxes
            .find_syntax_by_token(lang)
            .unwrap_or_else(|| self.assets.syntaxes.find_syntax_plain_text());
        let mut highlighter = syntect::easy::HighlightLines::new(
            syntax,
            &self.assets.themes.themes[theme.syntax_theme],
        );
        for source_line in code.split('\n') {
            let source_line = source_line.replace('\t', "    ");
            let with_newline = format!("{source_line}\n");
            let ranges = highlighter
                .highlight_line(&with_newline, &self.assets.syntaxes)
                .unwrap_or_default();
            let mut row: Vec<Span<'static>> = vec![Span::styled(" ", background)];
            let mut used = 0;
            let flush = |row: &mut Vec<Span<'static>>, lines: &mut Vec<RLine>| {
                let mut line = RLine::from_spans(std::mem::take(row));
                line.pad_to(width, background);
                lines.push(line);
                row.push(Span::styled(" ", background));
            };
            for (style, text) in ranges {
                let color = Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b);
                let mut chunk = String::new();
                for c in text.chars().filter(|c| *c != '\n' && *c != '\r') {
                    let w = c.width().unwrap_or(0);
                    if used + w > inner {
                        row.push(Span::styled(
                            std::mem::take(&mut chunk),
                            background.fg(color),
                        ));
                        flush(&mut row, &mut lines);
                        used = 0;
                    }
                    chunk.push(c);
                    used += w;
                }
                if !chunk.is_empty() {
                    row.push(Span::styled(chunk, background.fg(color)));
                }
            }
            flush(&mut row, &mut lines);
        }
        let index = self.out.codes.len();
        self.out.codes.push(code.to_string());
        for line in &mut lines {
            line.code = Some(index);
        }
        lines
    }

    fn list(&mut self, start: Option<u64>, items: &[ListItem], width: u16) -> Vec<RLine> {
        let theme = self.theme().clone();
        let loose = items.iter().any(|item| {
            item.blocks
                .iter()
                .filter(|b| !matches!(b, Block::List { .. }))
                .count()
                > 1
        });
        let number_width = start
            .map(|s| (s + items.len() as u64).to_string().len())
            .unwrap_or(0);
        let mut lines = Vec::new();
        for (i, item) in items.iter().enumerate() {
            if loose && i > 0 {
                lines.push(RLine::default());
            }
            let marker = match (item.checked, start) {
                (Some(true), _) => Span::styled("☑ ", Style::new().fg(theme.headings[4])),
                (Some(false), _) => Span::styled("☐ ", Style::new().fg(theme.dim)),
                (None, Some(first)) => Span::styled(
                    format!("{:>number_width$}. ", first + i as u64),
                    Style::new().fg(theme.marker),
                ),
                (None, None) => Span::styled("• ", Style::new().fg(theme.marker)),
            };
            let indent = marker.content.width() as u16;
            let mut body = self.blocks(&item.blocks, width.saturating_sub(indent), true);
            if body.is_empty() {
                body.push(RLine::default());
            }
            if item.checked == Some(true) {
                for span in body.iter_mut().flat_map(|l| l.spans.iter_mut()) {
                    if span.style.fg.is_none() {
                        span.style = span.style.fg(theme.dim);
                    }
                }
            }
            let first = [marker];
            let rest = [Span::raw(" ".repeat(indent as usize))];
            for (n, line) in body.iter_mut().enumerate() {
                line.prefix(if n == 0 { &first } else { &rest });
            }
            lines.extend(body);
        }
        lines
    }

    fn table(&mut self, table: &Table, width: u16) -> Vec<RLine> {
        let columns = table
            .rows
            .iter()
            .map(Vec::len)
            .chain([table.head.len()])
            .max()
            .unwrap_or(0);
        if columns == 0 {
            return Vec::new();
        }
        let border = Style::new().fg(self.theme().border);
        let head_style = Style::new()
            .fg(self.theme().headings[2])
            .add_modifier(Modifier::BOLD);

        // Natural width of every column, then squeezed to fit.
        let mut cells: Vec<Vec<Vec<Item>>> = Vec::new();
        for (r, row) in std::iter::once(&table.head).chain(&table.rows).enumerate() {
            let mut out_row = Vec::new();
            for c in 0..columns {
                let mut items = Vec::new();
                if let Some(cell) = row.get(c) {
                    let style = if r == 0 { head_style } else { Style::new() };
                    self.inline_items(cell, style, None, &mut items);
                }
                out_row.push(items);
            }
            cells.push(out_row);
        }
        let mut widths = vec![1usize; columns];
        for row in &cells {
            for (c, items) in row.iter().enumerate() {
                widths[c] = widths[c].max(items_width(items));
            }
        }
        let available = (width as usize)
            .saturating_sub(columns * 3 + 1)
            .max(columns * 3);
        while widths.iter().sum::<usize>() > available {
            let widest = (0..columns).max_by_key(|c| widths[*c]).unwrap();
            if widths[widest] <= 3 {
                break;
            }
            widths[widest] -= 1;
        }

        let edge = |left: &str, mid: &str, right: &str| {
            let mut text = String::from(left);
            for (c, w) in widths.iter().enumerate() {
                text.push_str(&"─".repeat(w + 2));
                text.push_str(if c + 1 == columns { right } else { mid });
            }
            RLine::from_spans(vec![Span::styled(text, border)])
        };
        let rows: Vec<Vec<Vec<RLine>>> = cells
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .enumerate()
                    .map(|(c, items)| wrap(items, widths[c] as u16))
                    .collect()
            })
            .collect();
        let height_of = |row: &Vec<Vec<RLine>>| row.iter().map(Vec::len).max().unwrap_or(1).max(1);
        // Once a cell wraps, rows are ruled off so it stays clear which
        // lines belong together.
        let ruled = rows.iter().skip(1).any(|row| height_of(row) > 1);
        let last = rows.len() - 1;
        let mut lines = vec![edge("┌", "┬", "┐")];
        for (r, wrapped) in rows.iter().enumerate() {
            let height = height_of(wrapped);
            for n in 0..height {
                let mut line = RLine::from_spans(vec![Span::styled("│", border)]);
                for (c, column) in wrapped.iter().enumerate() {
                    let cell = column.get(n).cloned().unwrap_or_default();
                    let free = widths[c].saturating_sub(cell.width() as usize);
                    let left = match table.aligns.get(c) {
                        Some(Align::Right) => free,
                        Some(Align::Center) => free / 2,
                        _ => 0,
                    };
                    let offset = line.width() + 1 + left as u16;
                    line.spans.push(Span::raw(" ".repeat(left + 1)));
                    line.links.extend(cell.links.iter().map(|hit| LinkHit {
                        start: hit.start + offset,
                        end: hit.end + offset,
                        link: hit.link,
                    }));
                    line.spans.extend(cell.spans);
                    line.spans.push(Span::raw(" ".repeat(free - left + 1)));
                    line.spans.push(Span::styled("│", border));
                }
                lines.push(line);
            }
            if r < last && (r == 0 || ruled) {
                lines.push(edge("├", "┼", "┤"));
            }
        }
        lines.push(edge("└", "┴", "┘"));
        lines
    }

    /// Raw HTML: pictures are shown, the remaining markup is reduced to text.
    fn html(&mut self, html: &str, width: u16) -> Vec<RLine> {
        let mut lines = Vec::new();
        for tag in html_tags(html, "img") {
            let alt = html_attr(&tag, "alt").unwrap_or_default();
            if let Some(path) = html_attr(&tag, "src").and_then(|src| self.local_path(&src)) {
                lines.extend(self.image_file(&path, &alt, width));
            } else if let Some(src) = html_attr(&tag, "src") {
                let link = self.link(&src);
                let label = if alt.is_empty() { src } else { alt };
                lines.extend(wrap(
                    vec![Item::Piece(Piece {
                        text: format!("🖼 {label}"),
                        style: Style::new().fg(self.theme().link),
                        link: Some(link),
                    })],
                    width,
                ));
            }
        }
        let text = strip_html(html);
        for paragraph in text.split("\n\n").filter(|p| !p.trim().is_empty()) {
            if !lines.is_empty() {
                lines.push(RLine::default());
            }
            lines.extend(wrap(
                vec![Item::Piece(Piece {
                    text: paragraph.to_string(),
                    style: Style::new(),
                    link: None,
                })],
                width,
            ));
        }
        lines
    }

    fn link(&mut self, url: &str) -> usize {
        self.out.links.push(url.to_string());
        self.out.links.len() - 1
    }

    fn inline_items(
        &mut self,
        inlines: &[Inline],
        style: Style,
        link: Option<usize>,
        out: &mut Vec<Item>,
    ) {
        let theme = self.theme().clone();
        let piece = |out: &mut Vec<Item>, text: String, style: Style, link: Option<usize>| {
            out.push(Item::Piece(Piece { text, style, link }))
        };
        for inline in inlines {
            match inline {
                Inline::Text(text) => piece(out, text.clone(), style, link),
                Inline::Code(code) => piece(
                    out,
                    // Non-breaking spaces keep the wrapper from collapsing
                    // the spacing inside code.
                    format!("\u{a0}{}\u{a0}", code.replace(' ', "\u{a0}")),
                    style.fg(theme.code).bg(theme.code_bg),
                    link,
                ),
                Inline::Math(tex) => piece(
                    out,
                    mdview_render::math_to_unicode(tex),
                    style.fg(theme.math).add_modifier(Modifier::ITALIC),
                    link,
                ),
                Inline::Emph(content) => {
                    self.inline_items(content, style.add_modifier(Modifier::ITALIC), link, out)
                }
                Inline::Strong(content) => {
                    self.inline_items(content, style.add_modifier(Modifier::BOLD), link, out)
                }
                Inline::Strike(content) => self.inline_items(
                    content,
                    style.add_modifier(Modifier::CROSSED_OUT),
                    link,
                    out,
                ),
                Inline::Sup(content) => {
                    let text = mdview_core::plain_text(content);
                    piece(
                        out,
                        mdview_render::math_to_unicode(&format!("^{{{text}}}")),
                        style,
                        link,
                    )
                }
                Inline::Sub(content) => {
                    let text = mdview_core::plain_text(content);
                    piece(
                        out,
                        mdview_render::math_to_unicode(&format!("_{{{text}}}")),
                        style,
                        link,
                    )
                }
                Inline::Link { url, content, .. } => {
                    let id = self.link(url);
                    self.inline_items(
                        content,
                        style.fg(theme.link).add_modifier(Modifier::UNDERLINED),
                        Some(id),
                        out,
                    );
                }
                Inline::Image { url, alt, .. } => {
                    let id = link.unwrap_or_else(|| self.link(url));
                    let label = if alt.is_empty() { url } else { alt };
                    piece(out, format!("🖼 {label}"), style.fg(theme.link), Some(id));
                }
                Inline::FootnoteRef(label) => {
                    piece(out, format!("[{label}]"), style.fg(theme.link), link)
                }
                Inline::Html(html) => {
                    let lower = html.to_ascii_lowercase();
                    if lower.starts_with("<br") {
                        out.push(Item::Break);
                    }
                }
                Inline::SoftBreak => piece(out, " ".into(), style, link),
                Inline::HardBreak => out.push(Item::Break),
            }
        }
    }
}

/// Resolves a relative link against the document directory. `None` for URLs
/// with a scheme and for pure anchors.
pub fn resolve_local(base_dir: &Path, url: &str) -> Option<PathBuf> {
    let target = url.split(['#', '?']).next().unwrap_or("");
    if target.is_empty() || target.contains("://") || target.starts_with("mailto:") {
        return None;
    }
    let target = target.strip_prefix("file:").unwrap_or(target);
    let decoded = percent_decode(target);
    let path = Path::new(&decoded);
    Some(if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir.join(path)
    })
}

pub fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = (bytes[i] == b'%')
            .then(|| text.get(i + 1..i + 3))
            .flatten()
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match hex {
            Some(byte) => {
                out.push(byte);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn truncate(text: &str, width: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > width {
            break;
        }
        out.push(c);
        used += w;
    }
    out
}

fn items_width(items: &[Item]) -> usize {
    let mut widest = 0;
    let mut current = 0;
    for item in items {
        match item {
            Item::Piece(piece) => current += piece.text.width(),
            Item::Break => {
                widest = widest.max(current);
                current = 0;
            }
        }
    }
    widest.max(current)
}

struct LineBuilder {
    line: RLine,
    width: usize,
}

impl LineBuilder {
    fn new() -> Self {
        LineBuilder {
            line: RLine::default(),
            width: 0,
        }
    }

    fn push(&mut self, text: &str, style: Style, link: Option<usize>) {
        let start = self.width as u16;
        self.width += text.width();
        let end = self.width as u16;
        if let Some(link) = link {
            match self.line.links.last_mut() {
                Some(hit) if hit.link == link && hit.end == start => hit.end = end,
                _ => self.line.links.push(LinkHit { start, end, link }),
            }
        }
        match self.line.spans.last_mut() {
            Some(span) if span.style == style => span.content.to_mut().push_str(text),
            _ => self.line.spans.push(Span::styled(text.to_string(), style)),
        }
    }

    fn take(&mut self) -> RLine {
        self.width = 0;
        std::mem::take(&mut self.line)
    }
}

/// Word-wraps styled text to `width` columns.
fn wrap(items: Vec<Item>, width: u16) -> Vec<RLine> {
    let width = width.max(1) as usize;
    let mut lines = Vec::new();
    let mut current = LineBuilder::new();
    let mut space: Option<(Style, Option<usize>)> = None;
    for item in items {
        let piece = match item {
            Item::Break => {
                lines.push(current.take());
                space = None;
                continue;
            }
            Item::Piece(piece) => piece,
        };
        let mut rest = piece.text.as_str();
        while !rest.is_empty() {
            let is_space = |c: char| c.is_whitespace() && c != '\u{a0}';
            let leading = rest.chars().next().is_some_and(is_space);
            let end = rest
                .char_indices()
                .find(|(_, c)| is_space(*c) != leading)
                .map_or(rest.len(), |(i, _)| i);
            let (token, tail) = rest.split_at(end);
            rest = tail;
            if leading {
                if current.width > 0 {
                    space = Some((piece.style, piece.link));
                }
                continue;
            }
            let word = token.replace('\u{a0}', " ");
            let word_width = word.width();
            let gap = usize::from(space.is_some());
            if current.width > 0 && current.width + gap + word_width > width {
                lines.push(current.take());
                space = None;
            }
            if let Some((style, link)) = space.take() {
                // A space inside one link stays part of it; between two
                // different runs it is plain.
                let (style, link) = if link == piece.link {
                    (style, link)
                } else {
                    (Style::new(), None)
                };
                current.push(" ", style, link);
            }
            if word_width <= width {
                current.push(&word, piece.style, piece.link);
                continue;
            }
            // A single word wider than the line is split by characters.
            let mut chunk = String::new();
            for c in word.chars() {
                let w = c.width().unwrap_or(0);
                if current.width + chunk.width() + w > width {
                    current.push(&chunk, piece.style, piece.link);
                    chunk.clear();
                    lines.push(current.take());
                }
                chunk.push(c);
            }
            current.push(&chunk, piece.style, piece.link);
        }
    }
    if current.width > 0 || lines.is_empty() {
        lines.push(current.take());
    }
    lines
}

/// Every `<name ...>` tag in `html`.
fn html_tags(html: &str, name: &str) -> Vec<String> {
    let lower = html.to_ascii_lowercase();
    let needle = format!("<{name}");
    let mut tags = Vec::new();
    let mut from = 0;
    while let Some(found) = lower[from..].find(&needle) {
        let start = from + found;
        let end = lower[start..]
            .find('>')
            .map_or(html.len(), |e| start + e + 1);
        tags.push(html[start..end].to_string());
        from = end;
    }
    tags
}

fn html_attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(found) = lower[from..].find(name) {
        let at = from + found;
        from = at + name.len();
        let boundary = lower[..at].ends_with(|c: char| c.is_whitespace());
        let rest = tag[from..].trim_start();
        if !boundary || !rest.starts_with('=') {
            continue;
        }
        let value = rest[1..].trim_start();
        return Some(match value.chars().next() {
            Some(quote @ ('"' | '\'')) => value[1..].split(quote).next().unwrap_or("").to_string(),
            _ => value
                .split(|c: char| c.is_whitespace() || c == '>')
                .next()
                .unwrap_or("")
                .to_string(),
        });
    }
    None
}

/// Drops tags and comments, keeping paragraph breaks.
fn strip_html(html: &str) -> String {
    let mut text = String::new();
    let mut rest = html;
    while let Some(open) = rest.find('<') {
        text.push_str(&rest[..open]);
        let tail = &rest[open..];
        let close = if tail.starts_with("<!--") {
            tail.find("-->").map(|i| i + 3)
        } else {
            tail.find('>').map(|i| i + 1)
        };
        let Some(close) = close else {
            rest = "";
            break;
        };
        let tag = tail[..close].to_ascii_lowercase();
        let name = tag.trim_start_matches(['<', '/']);
        if [
            "p", "div", "br", "li", "tr", "h1", "h2", "h3", "h4", "details", "summary",
        ]
        .iter()
        .any(|block| {
            name.strip_prefix(block)
                .is_some_and(|after| after.starts_with([' ', '>', '/']))
        }) {
            text.push_str("\n\n");
        }
        rest = &tail[close..];
    }
    text.push_str(rest);
    text.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(source: &str, width: u16, graphics: Graphics) -> Rendered {
        let theme = Theme::dark();
        let options = LayoutOptions {
            width,
            theme: &theme,
            graphics,
            cell: DEFAULT_CELL,
            base_dir: Path::new("."),
        };
        layout(
            &mdview_core::parse(source),
            &options,
            &mut Assets::default(),
        )
    }

    fn texts(rendered: &Rendered) -> Vec<String> {
        rendered.lines.iter().map(RLine::text).collect()
    }

    #[test]
    fn paragraphs_wrap_at_word_boundaries_within_the_width() {
        let rendered = render(
            "one two three four five six seven eight nine ten",
            12,
            Graphics::Text,
        );
        let lines = texts(&rendered);
        assert_eq!(
            lines,
            [
                "one two",
                "three four",
                "five six",
                "seven eight",
                "nine ten"
            ]
        );
        assert!(rendered.lines.iter().all(|l| l.width() <= 12));
    }

    #[test]
    fn links_record_their_columns_and_target() {
        let rendered = render("see [the docs](docs/a.md) now", 40, Graphics::Text);
        let hit = &rendered.lines[0].links[0];
        assert_eq!((hit.start, hit.end), (4, 12));
        assert_eq!(rendered.links[hit.link], "docs/a.md");
    }

    #[test]
    fn headings_are_indexed_by_line() {
        let rendered = render("# Title\n\ntext\n\n## Part two\n\nmore", 40, Graphics::Text);
        assert_eq!(rendered.anchors["title"], 0);
        let line = rendered.anchors["part-two"];
        assert_eq!(rendered.lines[line].text(), "Part two");
        assert_eq!(rendered.toc.len(), 2);
        let deep = render("### Third level\n", 40, Graphics::Text);
        assert_eq!(texts(&deep), ["Third level"]);
    }

    #[test]
    fn tables_fit_the_width_and_keep_every_cell() {
        let source = "| Name | Description |\n|---|---|\n| alpha | a rather long description that must wrap |\n";
        let rendered = render(source, 30, Graphics::Text);
        assert!(
            rendered.lines.iter().all(|l| l.width() <= 30),
            "{:#?}",
            texts(&rendered)
        );
        let all = texts(&rendered).join("\n");
        for word in ["Name", "alpha", "rather", "wrap"] {
            assert!(all.contains(word), "missing {word} in\n{all}");
        }
        assert!(all.starts_with('┌') && all.trim_end().ends_with('┘'));
        // A wrapped cell rules off every row; a table of single lines only
        // rules off its header.
        let wrapped = "| a | b |\n|---|---|\n| one two three four five six | x |\n| y | z |\n";
        let ruled = |source| {
            let lines = texts(&render(source, 20, Graphics::Text));
            lines.iter().filter(|l| l.starts_with('├')).count()
        };
        assert_eq!(ruled(wrapped), 2);
        assert_eq!(ruled("| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n"), 1);
    }

    #[test]
    fn lists_nest_with_markers_and_task_boxes() {
        let rendered = render(
            "- one\n  - inner\n- [x] done\n- [ ] todo\n\n3. three\n4. four\n",
            40,
            Graphics::Text,
        );
        assert_eq!(
            texts(&rendered),
            [
                "• one",
                "  • inner",
                "☑ done",
                "☐ todo",
                "",
                "3. three",
                "4. four"
            ]
        );
    }

    #[test]
    fn mermaid_is_drawn_as_text_or_reserved_for_an_image() {
        let source = "```mermaid\nflowchart LR\n  A[Start] --> B[End]\n```\n";
        let text = render(source, 60, Graphics::Text);
        let all = texts(&text).join("\n");
        assert!(all.contains("Start") && all.contains("End") && !all.contains("flowchart"));
        assert!(text.images.is_empty());

        let kitty = render(source, 60, Graphics::Kitty);
        assert_eq!(kitty.images.len(), 1);
        let slot = &kitty.images[0];
        assert_eq!(
            kitty.lines.iter().filter(|l| l.image.is_some()).count(),
            slot.rows as usize
        );
        assert!(slot.cols <= 60 && slot.rows >= 1);
    }

    #[test]
    fn code_blocks_fill_their_width_and_wrap_long_lines() {
        let rendered = render(
            "```rust\nfn main() { println!(\"a very long line of code here\"); }\n```\n",
            24,
            Graphics::Text,
        );
        assert!(rendered.lines.len() >= 3);
        assert!(rendered.lines[0].text().contains("rust"));
        assert!(rendered.lines[0].text().contains("[ copy ]"));
        assert!(rendered.lines.iter().all(|l| l.code == Some(0)));
        assert_eq!(rendered.codes.len(), 1);
        assert!(rendered.codes[0].starts_with("fn main()"));
        assert!(
            rendered.lines.iter().all(|l| l.width() == 24),
            "{:#?}",
            texts(&rendered)
        );
    }

    #[test]
    fn html_is_reduced_to_its_text() {
        let rendered = render(
            "<div align=\"center\">\n<b>Hello</b> &amp; welcome<!-- hidden -->\n</div>\n",
            40,
            Graphics::Text,
        );
        assert_eq!(texts(&rendered).join(" ").trim(), "Hello & welcome");
        assert_eq!(
            html_attr("<img alt='x y' src=a.png>", "src").as_deref(),
            Some("a.png")
        );
        assert_eq!(
            html_attr("<img alt='x y' src=a.png>", "alt").as_deref(),
            Some("x y")
        );
    }

    #[test]
    fn relative_links_resolve_against_the_document() {
        let base = Path::new("/docs");
        assert_eq!(
            resolve_local(base, "a%20b.md#top"),
            Some(PathBuf::from("/docs/a b.md"))
        );
        assert_eq!(resolve_local(base, "https://example.com/x.md"), None);
        assert_eq!(resolve_local(base, "#anchor"), None);
    }
}

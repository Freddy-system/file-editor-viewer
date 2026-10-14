use std::collections::HashMap;
use std::iter::Peekable;

use pulldown_cmark::{
    Alignment, BlockQuoteKind, CodeBlockKind, Event, Options, Parser, Tag, TagEnd,
};

use crate::model::*;

/// Every Markdown extension fev understands. The HTML frontend parses with
/// the same options so both renderers agree on what the source means.
pub fn parser_options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_SMART_PUNCTUATION
        | Options::ENABLE_HEADING_ATTRIBUTES
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
        | Options::ENABLE_PLUSES_DELIMITED_METADATA_BLOCKS
        | Options::ENABLE_MATH
        | Options::ENABLE_GFM
        | Options::ENABLE_DEFINITION_LIST
        | Options::ENABLE_SUPERSCRIPT
        | Options::ENABLE_SUBSCRIPT
        | Options::ENABLE_WIKILINKS
}

pub fn parse(source: &str) -> Document {
    let mut builder = Builder {
        events: Parser::new_ext(source, parser_options()).peekable(),
        doc: Document::default(),
        slugs: HashMap::new(),
        pending_task: None,
    };
    let blocks = builder.blocks(None);
    builder.doc.blocks = blocks;
    builder.doc
}

/// A piece of text after intraword script markers are recognised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Script<'a> {
    Plain(&'a str),
    Sub(&'a str),
    Sup(&'a str),
}

/// Splits Pandoc-style `H~2~O` and `x^2^` out of running text. CommonMark's
/// delimiter rules only see these markers at word boundaries.
pub fn split_scripts(text: &str) -> Vec<Script<'_>> {
    let mut parts = Vec::new();
    let mut plain_from = 0;
    let mut at = 0;
    while let Some(found) = text[at..].find(['~', '^']) {
        let open = at + found;
        let marker = text.as_bytes()[open];
        let inner_from = open + 1;
        let inner_len = text[inner_from..]
            .find(|c: char| !(c.is_alphanumeric() || "+-=()".contains(c)))
            .unwrap_or(text.len() - inner_from);
        let close = inner_from + inner_len;
        if inner_len == 0 || text.as_bytes().get(close) != Some(&marker) {
            at = inner_from;
            continue;
        }
        if open > plain_from {
            parts.push(Script::Plain(&text[plain_from..open]));
        }
        let inner = &text[inner_from..close];
        parts.push(if marker == b'~' {
            Script::Sub(inner)
        } else {
            Script::Sup(inner)
        });
        at = close + 1;
        plain_from = at;
    }
    if plain_from < text.len() {
        parts.push(Script::Plain(&text[plain_from..]));
    }
    parts
}

/// GitHub-compatible heading anchor.
pub fn slugify(text: &str) -> String {
    let mut slug = String::new();
    for c in text.trim().chars() {
        if c.is_alphanumeric() || c == '_' || c == '-' {
            slug.extend(c.to_lowercase());
        } else if c.is_whitespace() {
            slug.push('-');
        }
    }
    slug
}

struct Builder<'a> {
    events: Peekable<Parser<'a>>,
    doc: Document,
    slugs: HashMap<String, usize>,
    pending_task: Option<bool>,
}

fn is_inline_start(event: &Event) -> bool {
    match event {
        Event::Text(_)
        | Event::Code(_)
        | Event::InlineMath(_)
        | Event::DisplayMath(_)
        | Event::InlineHtml(_)
        | Event::FootnoteReference(_)
        | Event::SoftBreak
        | Event::HardBreak
        | Event::TaskListMarker(_) => true,
        Event::Start(tag) => matches!(
            tag,
            Tag::Emphasis
                | Tag::Strong
                | Tag::Strikethrough
                | Tag::Superscript
                | Tag::Subscript
                | Tag::Link { .. }
                | Tag::Image { .. }
        ),
        _ => false,
    }
}

impl<'a> Builder<'a> {
    /// Reads blocks until the matching end tag (or end of input for `None`).
    fn blocks(&mut self, until: Option<TagEnd>) -> Vec<Block> {
        let mut out = Vec::new();
        loop {
            let Some(event) = self.events.peek() else {
                break;
            };
            if let Event::End(end) = event {
                let done = Some(*end) == until;
                self.events.next();
                if done {
                    break;
                }
                continue;
            }
            // Tight list items carry their text without a paragraph wrapper.
            if is_inline_start(event) {
                let inlines = self.inlines(None);
                push_paragraph(&mut out, inlines);
                continue;
            }
            let event = self.events.next().unwrap();
            match event {
                Event::Start(tag) => self.block(tag, &mut out),
                Event::Rule => out.push(Block::Rule),
                Event::Html(html) => push_html(&mut out, &html),
                _ => {}
            }
        }
        out
    }

    fn block(&mut self, tag: Tag<'a>, out: &mut Vec<Block>) {
        match tag {
            Tag::Paragraph => {
                let inlines = self.inlines(Some(TagEnd::Paragraph));
                push_paragraph(out, inlines);
            }
            Tag::Heading { level, id, .. } => {
                let content = self.inlines(Some(TagEnd::Heading(level)));
                let base = match id {
                    Some(id) => id.to_string(),
                    None => slugify(&plain_text(&content)),
                };
                let seen = self.slugs.entry(base.clone()).or_insert(0);
                let id = if *seen == 0 {
                    base
                } else {
                    format!("{base}-{seen}")
                };
                *seen += 1;
                out.push(Block::Heading {
                    level: level as u8,
                    id,
                    content,
                });
            }
            Tag::BlockQuote(kind) => {
                let blocks = self.blocks(Some(TagEnd::BlockQuote(kind)));
                let alert = kind.map(|k| match k {
                    BlockQuoteKind::Note => Alert::Note,
                    BlockQuoteKind::Tip => Alert::Tip,
                    BlockQuoteKind::Important => Alert::Important,
                    BlockQuoteKind::Warning => Alert::Warning,
                    BlockQuoteKind::Caution => Alert::Caution,
                });
                out.push(Block::Quote { alert, blocks });
            }
            Tag::CodeBlock(kind) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => info
                        .split(|c: char| c.is_whitespace() || c == ',' || c == '{')
                        .next()
                        .unwrap_or("")
                        .to_ascii_lowercase(),
                    CodeBlockKind::Indented => String::new(),
                };
                let mut code = self.raw_text(TagEnd::CodeBlock);
                if code.ends_with('\n') {
                    code.pop();
                }
                out.push(Block::CodeBlock { lang, code });
            }
            Tag::HtmlBlock => {
                let html = self.raw_text(TagEnd::HtmlBlock);
                push_html(out, &html);
            }
            Tag::List(start) => {
                let mut items = Vec::new();
                loop {
                    match self.events.next() {
                        Some(Event::Start(Tag::Item)) => {
                            self.pending_task = None;
                            let blocks = self.blocks(Some(TagEnd::Item));
                            items.push(ListItem {
                                checked: self.pending_task.take(),
                                blocks,
                            });
                        }
                        Some(Event::End(TagEnd::List(_))) | None => break,
                        _ => {}
                    }
                }
                out.push(Block::List { start, items });
            }
            Tag::FootnoteDefinition(label) => {
                let blocks = self.blocks(Some(TagEnd::FootnoteDefinition));
                self.doc.footnotes.push(Footnote {
                    label: label.to_string(),
                    blocks,
                });
            }
            Tag::Table(aligns) => out.push(Block::Table(self.table(aligns))),
            Tag::DefinitionList => out.push(Block::DefinitionList(self.definitions())),
            Tag::MetadataBlock(kind) => {
                let text = self.raw_text(TagEnd::MetadataBlock(kind));
                self.doc.frontmatter = Some(text.trim_end().to_string());
            }
            // Anything else at block level is a container we do not model:
            // keep its children.
            _ => {}
        }
    }

    fn table(&mut self, aligns: Vec<Alignment>) -> Table {
        let mut table = Table {
            aligns: aligns
                .into_iter()
                .map(|a| match a {
                    Alignment::None => Align::None,
                    Alignment::Left => Align::Left,
                    Alignment::Center => Align::Center,
                    Alignment::Right => Align::Right,
                })
                .collect(),
            ..Table::default()
        };
        let mut row: Vec<Vec<Inline>> = Vec::new();
        loop {
            match self.events.next() {
                Some(Event::Start(Tag::TableCell)) => {
                    row.push(self.inlines(Some(TagEnd::TableCell)));
                }
                Some(Event::End(TagEnd::TableHead)) => table.head = std::mem::take(&mut row),
                Some(Event::End(TagEnd::TableRow)) => table.rows.push(std::mem::take(&mut row)),
                Some(Event::End(TagEnd::Table)) | None => break,
                _ => {}
            }
        }
        table
    }

    fn definitions(&mut self) -> Vec<Definition> {
        let mut defs: Vec<Definition> = Vec::new();
        loop {
            match self.events.next() {
                Some(Event::Start(Tag::DefinitionListTitle)) => {
                    let term = self.inlines(Some(TagEnd::DefinitionListTitle));
                    defs.push(Definition {
                        term,
                        details: Vec::new(),
                    });
                }
                Some(Event::Start(Tag::DefinitionListDefinition)) => {
                    let blocks = self.blocks(Some(TagEnd::DefinitionListDefinition));
                    if let Some(def) = defs.last_mut() {
                        def.details.push(blocks);
                    }
                }
                Some(Event::End(TagEnd::DefinitionList)) | None => break,
                _ => {}
            }
        }
        defs
    }

    /// Collects the text of a leaf block verbatim.
    fn raw_text(&mut self, until: TagEnd) -> String {
        let mut text = String::new();
        for event in self.events.by_ref() {
            match event {
                Event::End(end) if end == until => break,
                Event::Text(t) | Event::Html(t) | Event::InlineHtml(t) | Event::Code(t) => {
                    text.push_str(&t)
                }
                Event::SoftBreak | Event::HardBreak => text.push('\n'),
                _ => {}
            }
        }
        text
    }

    /// Reads inline content. With `until` it consumes through that end tag;
    /// without it, it stops before the first block-level event.
    fn inlines(&mut self, until: Option<TagEnd>) -> Vec<Inline> {
        let mut out = Vec::new();
        loop {
            match self.events.peek() {
                None => break,
                Some(Event::End(end)) => {
                    if until.is_none() {
                        break;
                    }
                    let done = Some(*end) == until;
                    self.events.next();
                    if done {
                        break;
                    }
                    continue;
                }
                Some(event) if until.is_none() && !is_inline_start(event) => break,
                _ => {}
            }
            match self.events.next().unwrap() {
                Event::Text(t) => match out.last_mut() {
                    Some(Inline::Text(prev)) => prev.push_str(&t),
                    _ => out.push(Inline::Text(t.to_string())),
                },
                Event::Code(t) => out.push(Inline::Code(t.to_string())),
                Event::InlineMath(t) => out.push(Inline::Math(t.to_string())),
                // Split into a block by `push_paragraph`.
                Event::DisplayMath(t) => out.push(Inline::Math(format!("\u{0}{t}"))),
                Event::InlineHtml(t) | Event::Html(t) => out.push(Inline::Html(t.to_string())),
                Event::FootnoteReference(t) => out.push(Inline::FootnoteRef(t.to_string())),
                Event::SoftBreak => out.push(Inline::SoftBreak),
                Event::HardBreak => out.push(Inline::HardBreak),
                Event::TaskListMarker(checked) => self.pending_task = Some(checked),
                Event::Start(tag) => match tag {
                    Tag::Emphasis => out.push(Inline::Emph(self.inlines(Some(TagEnd::Emphasis)))),
                    Tag::Strong => out.push(Inline::Strong(self.inlines(Some(TagEnd::Strong)))),
                    Tag::Strikethrough => {
                        out.push(Inline::Strike(self.inlines(Some(TagEnd::Strikethrough))))
                    }
                    Tag::Superscript => {
                        out.push(Inline::Sup(self.inlines(Some(TagEnd::Superscript))))
                    }
                    Tag::Subscript => out.push(Inline::Sub(self.inlines(Some(TagEnd::Subscript)))),
                    Tag::Link {
                        dest_url, title, ..
                    } => {
                        let content = self.inlines(Some(TagEnd::Link));
                        out.push(Inline::Link {
                            url: dest_url.to_string(),
                            title: title.to_string(),
                            content,
                        });
                    }
                    Tag::Image {
                        dest_url, title, ..
                    } => {
                        let alt = plain_text(&self.inlines(Some(TagEnd::Image)));
                        out.push(Inline::Image {
                            url: dest_url.to_string(),
                            title: title.to_string(),
                            alt,
                        });
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        expand_scripts(out)
    }
}

fn expand_scripts(inlines: Vec<Inline>) -> Vec<Inline> {
    let mut out = Vec::with_capacity(inlines.len());
    for inline in inlines {
        let Inline::Text(text) = &inline else {
            out.push(inline);
            continue;
        };
        let parts = split_scripts(text);
        if let [Script::Plain(_)] = parts.as_slice() {
            out.push(inline);
            continue;
        }
        for part in parts {
            out.push(match part {
                Script::Plain(t) => Inline::Text(t.to_string()),
                Script::Sub(t) => Inline::Sub(vec![Inline::Text(t.to_string())]),
                Script::Sup(t) => Inline::Sup(vec![Inline::Text(t.to_string())]),
            });
        }
    }
    out
}

/// Pushes a paragraph, lifting display math out into its own block.
fn push_paragraph(out: &mut Vec<Block>, inlines: Vec<Inline>) {
    let mut run: Vec<Inline> = Vec::new();
    let flush = |run: &mut Vec<Inline>, out: &mut Vec<Block>| {
        while matches!(run.last(), Some(Inline::SoftBreak | Inline::HardBreak)) {
            run.pop();
        }
        while matches!(run.first(), Some(Inline::SoftBreak | Inline::HardBreak)) {
            run.remove(0);
        }
        if !run.is_empty() {
            out.push(Block::Paragraph(std::mem::take(run)));
        }
    };
    for inline in inlines {
        match inline {
            Inline::Math(tex) if tex.starts_with('\u{0}') => {
                flush(&mut run, out);
                out.push(Block::Math(tex[1..].trim().to_string()));
            }
            other => run.push(other),
        }
    }
    flush(&mut run, out);
}

fn push_html(out: &mut Vec<Block>, html: &str) {
    if html.trim().is_empty() {
        return;
    }
    match out.last_mut() {
        Some(Block::Html(prev)) => prev.push_str(html),
        _ => out.push(Block::Html(html.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headings_get_unique_github_slugs() {
        let doc = parse("# Hello World\n\n## Hello World\n\n### Custom {#mine}\n");
        let ids: Vec<_> = doc.toc().into_iter().map(|e| e.id).collect();
        assert_eq!(ids, ["hello-world", "hello-world-1", "mine"]);
    }

    #[test]
    fn front_matter_is_separated_from_the_body() {
        let doc = parse("---\ntitle: x\n---\n\nbody\n");
        assert_eq!(doc.frontmatter.as_deref(), Some("title: x"));
        assert!(matches!(doc.blocks.as_slice(), [Block::Paragraph(_)]));
    }

    #[test]
    fn task_lists_keep_their_state_in_tight_and_loose_lists() {
        let doc = parse("- [x] done\n- [ ] todo\n- plain\n\n1. [x] loose\n\n2. other\n");
        let states = |b: &Block| match b {
            Block::List { items, .. } => items.iter().map(|i| i.checked).collect::<Vec<_>>(),
            _ => panic!("expected a list"),
        };
        assert_eq!(states(&doc.blocks[0]), [Some(true), Some(false), None]);
        assert_eq!(states(&doc.blocks[1]), [Some(true), None]);
        assert!(matches!(&doc.blocks[1], Block::List { start: Some(1), .. }));
    }

    #[test]
    fn display_math_becomes_its_own_block() {
        let doc = parse("before\n$$\nx^2\n$$\nafter $y$ end\n");
        assert!(matches!(&doc.blocks[0], Block::Paragraph(_)));
        assert!(matches!(&doc.blocks[1], Block::Math(tex) if tex == "x^2"));
        match &doc.blocks[2] {
            Block::Paragraph(inlines) => {
                assert!(inlines
                    .iter()
                    .any(|i| matches!(i, Inline::Math(t) if t == "y")))
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn intraword_scripts_are_recognised_but_lookalikes_are_not() {
        use Script::*;
        assert_eq!(
            split_scripts("H~2~O and x^2^"),
            [Plain("H"), Sub("2"), Plain("O and x"), Sup("2")]
        );
        for plain in ["~5 min or ~10", "~/a/~b", "a ^ b ^ c", "2^10", "~~"] {
            assert_eq!(split_scripts(plain), [Plain(plain)], "{plain}");
        }
        let doc = parse("CO~2~ `H~2~O`\n");
        match &doc.blocks[0] {
            Block::Paragraph(inlines) => {
                assert!(matches!(&inlines[1], Inline::Sub(_)));
                assert!(matches!(inlines.last(), Some(Inline::Code(code)) if code == "H~2~O"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn code_fence_language_drops_attributes() {
        let doc = parse("```Mermaid {theme=dark}\ngraph TD; A-->B\n```\n");
        assert!(matches!(
            &doc.blocks[0],
            Block::CodeBlock { lang, code } if lang == "mermaid" && code == "graph TD; A-->B"
        ));
    }

    #[test]
    fn tables_alerts_footnotes_and_definitions_are_modelled() {
        let doc = parse(
            "| a | b |\n|:--|--:|\n| 1 | 2 |\n\n> [!WARNING]\n> careful\n\ntext[^n]\n\n[^n]: note\n\nTerm\n: Meaning\n",
        );
        match &doc.blocks[0] {
            Block::Table(t) => {
                assert_eq!(t.aligns, [Align::Left, Align::Right]);
                assert_eq!((t.head.len(), t.rows.len()), (2, 1));
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(matches!(
            &doc.blocks[1],
            Block::Quote { alert: Some(Alert::Warning), blocks } if blocks.len() == 1
        ));
        assert_eq!(doc.footnotes.len(), 1);
        assert!(matches!(
            doc.blocks.last(),
            Some(Block::DefinitionList(defs)) if defs.len() == 1 && defs[0].details.len() == 1
        ));
    }
}

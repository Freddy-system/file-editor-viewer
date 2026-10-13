#[derive(Debug, Clone, Default)]
pub struct Document {
    /// Raw YAML/TOML front matter, without its fences.
    pub frontmatter: Option<String>,
    pub blocks: Vec<Block>,
    /// Footnote definitions in source order.
    pub footnotes: Vec<Footnote>,
}

#[derive(Debug, Clone)]
pub struct Footnote {
    pub label: String,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone)]
pub enum Block {
    Heading {
        level: u8,
        id: String,
        content: Vec<Inline>,
    },
    Paragraph(Vec<Inline>),
    /// Fenced or indented code. Diagram fences (`mermaid`, `dot`, ...) arrive
    /// here too; frontends decide by `lang` whether to draw them.
    CodeBlock {
        lang: String,
        code: String,
    },
    /// Display math (`$$ ... $$`), LaTeX source.
    Math(String),
    Quote {
        alert: Option<Alert>,
        blocks: Vec<Block>,
    },
    List {
        /// First number for ordered lists, `None` for bullet lists.
        start: Option<u64>,
        items: Vec<ListItem>,
    },
    Table(Table),
    DefinitionList(Vec<Definition>),
    Rule,
    Html(String),
}

#[derive(Debug, Clone)]
pub struct ListItem {
    /// `Some` for task list items.
    pub checked: Option<bool>,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, Default)]
pub struct Table {
    pub aligns: Vec<Align>,
    pub head: Vec<Vec<Inline>>,
    pub rows: Vec<Vec<Vec<Inline>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    None,
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone)]
pub struct Definition {
    pub term: Vec<Inline>,
    pub details: Vec<Vec<Block>>,
}

/// GitHub-style alert (`> [!NOTE]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alert {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

impl Alert {
    pub fn label(self) -> &'static str {
        match self {
            Alert::Note => "Note",
            Alert::Tip => "Tip",
            Alert::Important => "Important",
            Alert::Warning => "Warning",
            Alert::Caution => "Caution",
        }
    }
}

#[derive(Debug, Clone)]
pub enum Inline {
    Text(String),
    Code(String),
    Emph(Vec<Inline>),
    Strong(Vec<Inline>),
    Strike(Vec<Inline>),
    Sup(Vec<Inline>),
    Sub(Vec<Inline>),
    Link {
        url: String,
        title: String,
        content: Vec<Inline>,
    },
    Image {
        url: String,
        title: String,
        alt: String,
    },
    /// Inline math (`$ ... $`), LaTeX source.
    Math(String),
    FootnoteRef(String),
    Html(String),
    SoftBreak,
    HardBreak,
}

/// One entry of the table of contents.
#[derive(Debug, Clone)]
pub struct TocEntry {
    pub level: u8,
    pub id: String,
    pub title: String,
}

impl Document {
    pub fn toc(&self) -> Vec<TocEntry> {
        self.blocks
            .iter()
            .filter_map(|b| match b {
                Block::Heading { level, id, content } => Some(TocEntry {
                    level: *level,
                    id: id.clone(),
                    title: plain_text(content),
                }),
                _ => None,
            })
            .collect()
    }

    /// Title of the document: its first heading, if any.
    pub fn title(&self) -> Option<String> {
        self.toc().into_iter().next().map(|e| e.title)
    }
}

/// Flattens inline content to unstyled text.
pub fn plain_text(inlines: &[Inline]) -> String {
    let mut out = String::new();
    push_plain(inlines, &mut out);
    out
}

fn push_plain(inlines: &[Inline], out: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Text(t) | Inline::Code(t) | Inline::Math(t) => out.push_str(t),
            Inline::Emph(c)
            | Inline::Strong(c)
            | Inline::Strike(c)
            | Inline::Sup(c)
            | Inline::Sub(c) => push_plain(c, out),
            Inline::Link { content, .. } => push_plain(content, out),
            Inline::Image { alt, .. } => out.push_str(alt),
            Inline::SoftBreak | Inline::HardBreak => out.push(' '),
            Inline::FootnoteRef(_) | Inline::Html(_) => {}
        }
    }
}

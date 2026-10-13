use mdview_core::{parse, parser_options, split_scripts, Script};
use pulldown_cmark::{CodeBlockKind, CowStr, Event, Parser, Tag, TagEnd, TextMergeStream};

const CSS: &str = include_str!("../assets/page.css");
const JS: &str = include_str!("../assets/page.js");

const KATEX: &str = "https://cdn.jsdelivr.net/npm/katex@0.16.22/dist";
const HIGHLIGHT: &str = "https://cdn.jsdelivr.net/gh/highlightjs/cdn-release@11.11.1/build";
const MERMAID: &str = "https://cdn.jsdelivr.net/npm/mermaid@11.6.0/dist/mermaid.esm.min.mjs";
const VIZ: &str = "https://cdn.jsdelivr.net/npm/@viz-js/viz@3.11.0/+esm";

#[derive(Debug, Clone, Default)]
pub struct PageOptions {
    /// Shown in the browser tab when the document has no heading.
    pub fallback_title: String,
    /// URL the page polls to learn the source changed. `None` for exports.
    pub stamp_url: Option<String>,
    /// Sibling documents listed under the table of contents: label and href.
    pub files: Vec<(String, String)>,
}

pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

fn diagram_kind(lang: &str) -> Option<&'static str> {
    match lang {
        "mermaid" | "mmd" => Some("mermaid"),
        "dot" | "graphviz" | "gv" => Some("graphviz"),
        _ => None,
    }
}

/// Renders the Markdown body. Diagrams and math are left as marked-up source
/// for the page script to draw.
fn render_body(source: &str, heading_ids: &[String]) -> String {
    let mut events: Vec<Event> = Vec::new();
    // Merged so script markers and their content arrive in one text event.
    let mut parser = TextMergeStream::new(Parser::new_ext(source, parser_options()));
    let mut heading = 0;
    let mut in_code = false;
    while let Some(event) = parser.next() {
        match event {
            Event::Text(text) if !in_code => {
                let parts = split_scripts(&text);
                if let [Script::Plain(_)] = parts.as_slice() {
                    events.push(Event::Text(text));
                    continue;
                }
                for part in parts {
                    events.push(match part {
                        Script::Plain(t) => Event::Text(CowStr::from(t.to_string())),
                        Script::Sub(t) => {
                            Event::InlineHtml(CowStr::from(format!("<sub>{}</sub>", escape(t))))
                        }
                        Script::Sup(t) => {
                            Event::InlineHtml(CowStr::from(format!("<sup>{}</sup>", escape(t))))
                        }
                    });
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code = false;
                events.push(Event::End(TagEnd::CodeBlock));
            }
            Event::Start(Tag::MetadataBlock(kind)) => {
                // Front matter is shown by the page shell.
                for inner in parser.by_ref() {
                    if inner == Event::End(TagEnd::MetadataBlock(kind)) {
                        break;
                    }
                }
            }
            Event::Start(Tag::Heading {
                level,
                id,
                classes,
                attrs,
            }) => {
                // Same ids as the terminal viewer, so anchors agree.
                let id = heading_ids
                    .get(heading)
                    .map(|id| CowStr::from(id.clone()))
                    .or(id);
                heading += 1;
                events.push(Event::Start(Tag::Heading {
                    level,
                    id,
                    classes,
                    attrs,
                }));
            }
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info))) => {
                let lang = info
                    .split(|c: char| c.is_whitespace() || c == ',' || c == '{')
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let special = diagram_kind(&lang)
                    .map(|kind| format!("<div class=\"diagram\" data-kind=\"{kind}\">"));
                let special = special.or_else(|| {
                    matches!(lang.as_str(), "math" | "latex" | "tex" | "katex")
                        .then(|| "<div class=\"math math-display\">".to_string())
                });
                let Some(open) = special else {
                    in_code = true;
                    events.push(Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(
                        CowStr::from(lang),
                    ))));
                    continue;
                };
                let mut code = String::new();
                for inner in parser.by_ref() {
                    match inner {
                        Event::Text(text) => code.push_str(&text),
                        Event::End(TagEnd::CodeBlock) => break,
                        _ => {}
                    }
                }
                events.push(Event::Html(CowStr::from(format!(
                    "{open}{}</div>\n",
                    escape(code.trim_end())
                ))));
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                in_code = true;
                events.push(Event::Start(Tag::CodeBlock(kind)));
            }
            Event::Start(Tag::Table(aligns)) => {
                events.push(Event::Html(CowStr::from("<div class=\"table-wrap\">")));
                events.push(Event::Start(Tag::Table(aligns)));
            }
            Event::End(TagEnd::Table) => {
                events.push(Event::End(TagEnd::Table));
                events.push(Event::Html(CowStr::from("</div>")));
            }
            other => events.push(other),
        }
    }
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, events.into_iter());
    html
}

/// A complete HTML page for `source`.
pub fn render_page(source: &str, options: &PageOptions) -> String {
    let doc = parse(source);
    let toc = doc.toc();
    let ids: Vec<String> = toc.iter().map(|e| e.id.clone()).collect();
    let title = doc
        .title()
        .unwrap_or_else(|| options.fallback_title.clone());

    let mut nav = String::new();
    if !toc.is_empty() {
        nav.push_str("<h2>On this page</h2>");
        for entry in &toc {
            nav.push_str(&format!(
                "<a class=\"l{}\" href=\"#{}\">{}</a>",
                entry.level,
                escape(&entry.id),
                escape(&entry.title)
            ));
        }
    }
    if !options.files.is_empty() {
        nav.push_str("<div class=\"files\"><h2>Documents</h2>");
        for (label, href) in &options.files {
            nav.push_str(&format!(
                "<a href=\"{}\">{}</a>",
                escape(href),
                escape(label)
            ));
        }
        nav.push_str("</div>");
    }

    let mut body = String::new();
    if let Some(frontmatter) = &doc.frontmatter {
        body.push_str(&format!(
            "<details class=\"frontmatter\"><summary>Front matter</summary><pre><code class=\"language-yaml\">{}</code></pre></details>",
            escape(frontmatter)
        ));
    }
    body.push_str(&render_body(source, &ids));

    let stamp = options
        .stamp_url
        .as_ref()
        .map(|url| format!(" data-stamp=\"{}\"", escape(url)))
        .unwrap_or_default();
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title}</title>
<link rel="stylesheet" href="{KATEX}/katex.min.css">
<link rel="stylesheet" href="{HIGHLIGHT}/styles/github.min.css" media="(prefers-color-scheme: light)">
<link rel="stylesheet" href="{HIGHLIGHT}/styles/github-dark.min.css" media="(prefers-color-scheme: dark)">
<style>{CSS}</style>
<script defer src="{KATEX}/katex.min.js"></script>
<script defer src="{HIGHLIGHT}/highlight.min.js"></script>
<script>const MDVIEW_CDN = {{ mermaid: "{MERMAID}", viz: "{VIZ}" }};</script>
<script type="module">{JS}</script>
</head>
<body{stamp}>
<div class="layout">
<nav class="toc">{nav}</nav>
<main>{body}</main>
</div>
</body>
</html>
"#,
        title = escape(&title),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(source: &str) -> String {
        render_page(source, &PageOptions::default())
    }

    #[test]
    fn diagrams_and_math_are_handed_to_the_page_script() {
        let html = page("```mermaid\ngraph TD; A-->B\n```\n\n```dot\ndigraph { a -> b }\n```\n\n$$x<1$$\n\ninline $a^2$\n");
        assert!(
            html.contains("<div class=\"diagram\" data-kind=\"mermaid\">graph TD; A--&gt;B</div>")
        );
        assert!(html
            .contains("<div class=\"diagram\" data-kind=\"graphviz\">digraph { a -&gt; b }</div>"));
        assert!(html.contains("<span class=\"math math-display\">x&lt;1</span>"));
        assert!(html.contains("<span class=\"math math-inline\">a^2</span>"));
    }

    #[test]
    fn headings_carry_the_same_ids_as_the_table_of_contents() {
        let html = page("# Intro\n\n## Intro\n");
        assert!(html.contains("<h1 id=\"intro\">") && html.contains("<h2 id=\"intro-1\">"));
        assert!(html.contains("href=\"#intro-1\""));
        assert!(html.contains("<title>Intro</title>"));
    }

    #[test]
    fn github_extensions_reach_the_html() {
        let html = page("---\na: 1\n---\n\n> [!TIP]\n> hi\n\n- [x] done\n\n| a |\n|---|\n| b |\n\n~~gone~~ text[^1]\n\n[^1]: note\n\n```rust\nfn main() {}\n```\n");
        for needle in [
            "class=\"frontmatter\"",
            "markdown-alert-tip",
            "type=\"checkbox\"",
            "<table>",
            "<del>gone</del>",
            "footnote-definition",
            "class=\"language-rust\"",
        ] {
            assert!(html.contains(needle), "missing {needle}");
        }
        assert!(!html.contains("<p>a: 1</p>"));
        let scripts = page("H~2~O x^2^\n\n```\nH~2~O\n```\n");
        assert!(scripts.contains("H<sub>2</sub>O x<sup>2</sup>"));
        assert!(scripts.contains("<code>H~2~O"));
    }

    #[test]
    fn script_content_in_markdown_is_escaped_inside_diagrams() {
        let html = page("```mermaid\n</div><script>alert(1)</script>\n```\n");
        assert!(!html.contains("<script>alert(1)</script>"));
    }
}

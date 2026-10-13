use std::io::Write;
use std::process::{Command, Stdio};

use anyhow::{anyhow, bail, Context};

use crate::{DiagramEngine, Palette};

/// Mermaid, drawn in-process.
pub struct MermaidEngine;

impl DiagramEngine for MermaidEngine {
    fn languages(&self) -> &[&str] {
        &["mermaid", "mmd"]
    }

    fn to_svg(&self, source: &str, palette: Palette) -> anyhow::Result<String> {
        let mut options = mermaid_rs_renderer::RenderOptions::default();
        if palette.dark {
            options.theme = mermaid_rs_renderer::Theme::dark();
        }
        // The renderer can panic on syntax it does not know; a broken diagram
        // must not take the viewer down.
        std::panic::catch_unwind(|| mermaid_rs_renderer::render_with_options(source, options))
            .map_err(|_| anyhow!("the Mermaid renderer failed on this diagram"))?
    }

    fn to_text(&self, source: &str, max_width: usize) -> Option<anyhow::Result<String>> {
        let rendered =
            std::panic::catch_unwind(|| mermaid_text::render_with_width(source, Some(max_width)))
                .map_err(|_| anyhow!("the Mermaid text renderer failed on this diagram"))
                .and_then(|r| r.map_err(|e| anyhow!("{e}")));
        Some(rendered)
    }
}

/// Graphviz DOT, drawn in-process.
pub struct DotEngine;

impl DiagramEngine for DotEngine {
    fn languages(&self) -> &[&str] {
        &["dot", "graphviz", "gv"]
    }

    fn to_svg(&self, source: &str, palette: Palette) -> anyhow::Result<String> {
        use layout::backends::svg::SVGWriter;
        use layout::gv::{DotParser, GraphBuilder};

        let source = source.to_string();
        std::panic::catch_unwind(move || {
            let graph = DotParser::new(&source)
                .process()
                .map_err(|e| anyhow!("invalid DOT: {e}"))?;
            let mut builder = GraphBuilder::new();
            builder.visit_graph(&graph);
            let mut visual = builder.get();
            let mut svg = SVGWriter::new();
            visual.do_it(false, false, false, &mut svg);
            Ok(svg.finalize())
        })
        .map_err(|_| anyhow!("the Graphviz renderer failed on this graph"))?
        .map(|svg| recolor_dot(svg, palette))
    }

    /// Text mode goes through Mermaid's text renderer: positions differ from
    /// Graphviz's own layout, but the graph is readable in any terminal.
    fn to_text(&self, source: &str, max_width: usize) -> Option<anyhow::Result<String>> {
        let flowchart = dot_to_mermaid(source)?;
        MermaidEngine.to_text(&flowchart, max_width)
    }
}

/// Rewrites a DOT graph as a Mermaid flowchart: its nodes, edges and their
/// labels, without styling. `None` when the source does not parse.
fn dot_to_mermaid(source: &str) -> Option<String> {
    use layout::gv::parser::ast::{ArrowKind, Graph, Stmt};

    struct Flow {
        ids: Vec<String>,
        labels: Vec<Option<String>>,
        edges: Vec<String>,
    }

    impl Flow {
        fn node(&mut self, name: &str) -> usize {
            match self.ids.iter().position(|id| id == name) {
                Some(index) => index,
                None => {
                    self.ids.push(name.to_string());
                    self.labels.push(None);
                    self.ids.len() - 1
                }
            }
        }

        fn visit(&mut self, graph: &Graph) {
            for statement in &graph.list.list {
                match statement {
                    Stmt::Node(node) => {
                        let index = self.node(&node.id.name);
                        if let Some((_, label)) = node.list.iter().find(|(k, _)| k == "label") {
                            self.labels[index] = Some(label.clone());
                        }
                    }
                    Stmt::Edge(edge) => {
                        let label = edge
                            .list
                            .iter()
                            .find(|(k, _)| k == "label")
                            .map(|(_, label)| format!("|{}|", clean(label)))
                            .unwrap_or_default();
                        let mut from = self.node(&edge.from.name);
                        for (to, kind) in &edge.to {
                            let to = self.node(&to.name);
                            let arrow = match kind {
                                ArrowKind::Arrow => "-->",
                                ArrowKind::Line => "---",
                            };
                            self.edges.push(format!("    n{from} {arrow}{label} n{to}"));
                            from = to;
                        }
                    }
                    Stmt::SubGraph(inner) => self.visit(inner),
                    Stmt::Attribute(_) => {}
                }
            }
        }
    }

    fn clean(label: &str) -> String {
        label
            .replace("\\n", " ")
            .replace(['"', '|', '\n'], " ")
            .trim()
            .to_string()
    }

    let owned = source.to_string();
    let graph = std::panic::catch_unwind(move || layout::gv::DotParser::new(&owned).process())
        .ok()?
        .ok()?;
    let mut flow = Flow {
        ids: Vec::new(),
        labels: Vec::new(),
        edges: Vec::new(),
    };
    flow.visit(&graph);
    if flow.ids.is_empty() {
        return None;
    }
    let compact: String = source.chars().filter(|c| !c.is_whitespace()).collect();
    let direction = if compact.contains("rankdir=LR") || compact.contains("rankdir=\"LR\"") {
        "LR"
    } else {
        "TD"
    };
    let mut out = format!("flowchart {direction}\n");
    for (index, id) in flow.ids.iter().enumerate() {
        let label = clean(flow.labels[index].as_deref().unwrap_or(id));
        out.push_str(&format!("    n{index}[\"{label}\"]\n"));
    }
    out.push_str(&flow.edges.join("\n"));
    Some(out)
}

/// The layout engine draws black on white. On a dark background the default
/// colours are swapped for the palette's; colours the graph sets itself stay.
fn recolor_dot(svg: String, palette: Palette) -> String {
    if !palette.dark {
        return svg;
    }
    let [r, g, b] = palette.foreground;
    let ink = format!("#{r:02x}{g:02x}{b:02x}");
    svg.replace("#000000ff", &format!("{ink}ff"))
        .replace("#ffffffff", "#ffffff14")
        .replace("font-family:", &format!("fill: {ink}; font-family:"))
}

/// A diagram tool run as a subprocess: source on stdin, SVG on stdout.
pub struct ExternalEngine {
    languages: &'static [&'static str],
    program: &'static str,
    args: &'static [&'static str],
}

const EXTERNAL_TOOLS: &[ExternalEngine] = &[
    ExternalEngine {
        languages: &["plantuml", "puml"],
        program: "plantuml",
        args: &["-tsvg", "-pipe"],
    },
    ExternalEngine {
        languages: &["d2"],
        program: "d2",
        args: &["-", "-"],
    },
];

impl ExternalEngine {
    /// The known external tools that are installed.
    pub fn available() -> Vec<ExternalEngine> {
        EXTERNAL_TOOLS
            .iter()
            .filter(|tool| on_path(tool.program))
            .map(|tool| ExternalEngine {
                languages: tool.languages,
                program: tool.program,
                args: tool.args,
            })
            .collect()
    }
}

fn on_path(program: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths)
        .any(|dir| dir.join(program).is_file() || dir.join(format!("{program}.exe")).is_file())
}

impl DiagramEngine for ExternalEngine {
    fn languages(&self) -> &[&str] {
        self.languages
    }

    fn to_svg(&self, source: &str, _palette: Palette) -> anyhow::Result<String> {
        let mut child = Command::new(self.program)
            .args(self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("could not start {}", self.program))?;
        let mut stdin = child.stdin.take().expect("stdin was piped");
        let source = source.to_string();
        // Written from a thread so a tool that fills its stdout pipe before
        // draining stdin cannot deadlock us.
        let writer = std::thread::spawn(move || stdin.write_all(source.as_bytes()));
        let output = child.wait_with_output()?;
        let _ = writer.join();
        if !output.status.success() {
            bail!(
                "{} failed: {}",
                self.program,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(String::from_utf8(output.stdout)?)
    }
}

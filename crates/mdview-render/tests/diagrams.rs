use mdview_render::{rasterize_svg, Palette, Registry};

const DARK: Palette = Palette {
    dark: true,
    foreground: [220, 220, 220],
};

#[test]
fn mermaid_renders_to_pixels_and_to_text() {
    let registry = Registry::default();
    let engine = registry.find("mermaid").expect("mermaid is built in");
    let source = "flowchart LR\n  A[Start] --> B{Choice}\n  B -->|yes| C[Done]\n";

    let svg = engine.to_svg(source, DARK).unwrap();
    let image = rasterize_svg(&svg, 2.0).unwrap();
    assert!(image.width() > 200 && image.height() > 40);
    assert!(image.pixels().any(|p| p[3] > 0));

    let text = engine.to_text(source, 80).unwrap().unwrap();
    assert!(text.contains("Start") && text.contains("Done"));
}

#[test]
fn graphviz_renders_to_pixels() {
    let registry = Registry::default();
    let engine = registry.find("dot").expect("dot is built in");
    let svg = engine.to_svg("digraph { a -> b; b -> c; }", DARK).unwrap();
    let image = rasterize_svg(&svg, 1.0).unwrap();
    assert!(image.width() > 20 && image.height() > 20);

    // Terminals without images get the graph as a text drawing.
    let source =
        "digraph { rankdir=LR; a [label=\"Start here\"]; a -> b -> c; b -> a [label=\"retry\"]; }";
    let text = engine.to_text(source, 100).unwrap().unwrap();
    for part in ["Start here", "b", "c", "retry"] {
        assert!(text.contains(part), "missing {part} in\n{text}");
    }
    assert!(
        text.lines().count() < 12,
        "left-to-right graph should be shallow:\n{text}"
    );
    assert!(!text.contains('"'), "labels must not keep quotes:\n{text}");
    assert!(engine.to_text("digraph {", 80).is_none());

    // Default black ink would vanish on a dark background.
    assert!(!svg.contains("#000000ff") && svg.contains("#dcdcdcff"));
    let light = Palette {
        dark: false,
        ..DARK
    };
    assert!(engine
        .to_svg("digraph { a -> b }", light)
        .unwrap()
        .contains("#000000ff"));
}

#[test]
fn broken_diagrams_are_errors_not_crashes() {
    let registry = Registry::default();
    assert!(registry
        .find("dot")
        .unwrap()
        .to_svg("digraph {", DARK)
        .is_err());
    assert!(registry.find("nope").is_none());
}

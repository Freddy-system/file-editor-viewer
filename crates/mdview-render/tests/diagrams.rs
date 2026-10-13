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
    assert!(engine.to_text("digraph { a -> b }", 80).is_none());

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

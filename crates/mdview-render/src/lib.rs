//! Diagram, math and image engines.
//!
//! Each diagram language is a [`DiagramEngine`] registered in a [`Registry`];
//! supporting a new one means adding an engine, nothing else changes.

mod engines;
mod math;
mod raster;

use std::sync::Arc;

pub use engines::{DotEngine, ExternalEngine, MermaidEngine};
pub use image::RgbaImage;
pub use math::{math_to_image, math_to_unicode};
pub use raster::{load_image, rasterize_svg};

/// Colours a frontend wants its graphics drawn with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Palette {
    pub dark: bool,
    pub foreground: [u8; 3],
}

pub trait DiagramEngine: Send + Sync {
    /// Code fence languages this engine draws.
    fn languages(&self) -> &[&str];

    fn to_svg(&self, source: &str, palette: Palette) -> anyhow::Result<String>;

    /// Unicode drawing for terminals without image support. `None` when the
    /// engine has no text mode.
    fn to_text(&self, _source: &str, _max_width: usize) -> Option<anyhow::Result<String>> {
        None
    }
}

#[derive(Clone)]
pub struct Registry {
    engines: Vec<Arc<dyn DiagramEngine>>,
}

impl Default for Registry {
    /// Built-in engines, plus the external tools found on `PATH`.
    fn default() -> Self {
        let mut registry = Registry {
            engines: Vec::new(),
        };
        registry.register(MermaidEngine);
        registry.register(DotEngine);
        for engine in ExternalEngine::available() {
            registry.register(engine);
        }
        registry
    }
}

impl Registry {
    pub fn register(&mut self, engine: impl DiagramEngine + 'static) {
        self.engines.push(Arc::new(engine));
    }

    pub fn find(&self, language: &str) -> Option<&dyn DiagramEngine> {
        self.engines
            .iter()
            .find(|e| e.languages().contains(&language))
            .map(|e| e.as_ref())
    }
}

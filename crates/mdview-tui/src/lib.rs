//! Terminal frontend: the viewer that runs in a herdr pane or any terminal.

mod app;
mod graphics;
mod layout;
mod theme;

use std::path::PathBuf;

pub use graphics::Graphics;

pub struct Options {
    /// Directory the file picker and the browser preview are rooted at.
    pub root: PathBuf,
    /// Document to open first. Without one the viewer opens the directory's
    /// README, or the file picker.
    pub file: Option<PathBuf>,
    /// Heading of `file` to start at.
    pub anchor: Option<String>,
}

/// Runs the viewer until the user quits.
pub fn run(options: Options) -> anyhow::Result<()> {
    app::run(options)
}

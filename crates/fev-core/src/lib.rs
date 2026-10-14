//! Markdown parsing and the document model shared by every fev frontend.
//!
//! The terminal viewer, the browser preview and any future host (VS Code,
//! desktop shell) all start from [`parse`], so a Markdown feature added here
//! reaches every frontend at once.

mod model;
mod parser;

pub use model::*;
pub use parser::{parse, parser_options, slugify, split_scripts, Script};

/// File extensions treated as Markdown documents.
pub const MARKDOWN_EXTENSIONS: &[&str] = &["md", "markdown", "mdx", "mdown", "mkd"];

pub fn is_markdown_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| MARKDOWN_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

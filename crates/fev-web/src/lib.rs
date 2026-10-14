//! HTML frontend: turns Markdown into a self-contained page and serves it
//! with live reload. The page is what a VS Code webview or a desktop shell
//! embeds, so those hosts add no rendering code of their own.

mod page;
mod server;

pub use page::{render_page, PageOptions};
pub use server::Server;

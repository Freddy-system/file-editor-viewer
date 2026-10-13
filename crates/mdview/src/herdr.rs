//! Glue between herdr and the viewer.
//!
//! herdr starts plugin commands in the plugin directory and describes the
//! invocation in `HERDR_PLUGIN_CONTEXT_JSON`. An action cannot draw, so
//! `open` works out what to preview and asks herdr for a pane running `pane`.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context};
use clap::{Subcommand, ValueEnum};
use serde_json::Value;

const TARGET_ENV: &str = "MDVIEW_TARGET";
const ROOT_ENV: &str = "MDVIEW_ROOT";
const ANCHOR_ENV: &str = "MDVIEW_ANCHOR";

#[derive(Subcommand)]
pub enum HerdrCmd {
    /// Action: preview the clicked link, the selected path, or the focused
    /// pane's directory.
    Open {
        #[arg(long, value_enum, default_value_t = Placement::Split)]
        placement: Placement,
    },
    /// Pane entry point: run the viewer on what `open` chose.
    Pane,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Placement {
    Split,
    Tab,
    Overlay,
    Zoomed,
}

pub fn run(command: HerdrCmd) -> anyhow::Result<()> {
    let context = context();
    match command {
        HerdrCmd::Open { placement } => open(&context, placement),
        HerdrCmd::Pane => {
            let root = std::env::var_os(ROOT_ENV)
                .map(PathBuf::from)
                .or_else(|| pane_directory(&context));
            let target = std::env::var_os(TARGET_ENV)
                .map(PathBuf::from)
                .or_else(|| root.clone())
                .unwrap_or_else(|| PathBuf::from("."));
            crate::view(&target, root, std::env::var(ANCHOR_ENV).ok())
        }
    }
}

fn context() -> Value {
    std::env::var("HERDR_PLUGIN_CONTEXT_JSON")
        .ok()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or(Value::Null)
}

fn text<'a>(context: &'a Value, key: &str) -> Option<&'a str> {
    context
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

/// Directory of the pane the user was in when they invoked the plugin.
fn pane_directory(context: &Value) -> Option<PathBuf> {
    ["focused_pane_cwd", "worktree_path", "workspace_cwd"]
        .iter()
        .filter_map(|key| text(context, key))
        .map(PathBuf::from)
        .find(|dir| dir.is_dir())
}

/// Turns a clicked link or selected text into an existing path.
fn resolve(reference: &str, base: &Path) -> Option<PathBuf> {
    let reference = reference.trim().trim_matches(['"', '\'', '`', '<', '>']);
    let reference = reference.split('#').next().unwrap_or("");
    // file://host/path, where the host is normally empty.
    let reference = match reference.strip_prefix("file://") {
        Some(rest) => &rest[rest.find('/')?..],
        None => reference,
    };
    if reference.is_empty() || reference.contains("://") || reference.contains('\n') {
        return None;
    }
    let decoded = percent_decode(reference);
    let expanded = match decoded.strip_prefix("~/") {
        Some(rest) => PathBuf::from(std::env::var_os("HOME")?).join(rest),
        None => PathBuf::from(&decoded),
    };
    let path = if expanded.is_absolute() {
        expanded
    } else {
        base.join(expanded)
    };
    if path.exists() {
        return Some(path);
    }
    // Compilers and agents print paths as `docs/x.md:12` or `docs/x.md:12:3`.
    let trimmed = decoded.trim_end_matches(|c: char| c.is_ascii_digit() || c == ':');
    (trimmed.len() < decoded.len())
        .then(|| resolve(trimmed, base))
        .flatten()
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = (bytes[i] == b'%')
            .then(|| text.get(i + 1..i + 3))
            .flatten()
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match hex {
            Some(byte) => {
                out.push(byte);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn open(context: &Value, placement: Placement) -> anyhow::Result<()> {
    let base = pane_directory(context).unwrap_or_else(|| PathBuf::from("."));
    let clicked = std::env::var("HERDR_PLUGIN_CLICKED_URL").ok();
    let references = [
        clicked.as_deref(),
        text(context, "clicked_url"),
        text(context, "selected_text"),
        text(context, "selection"),
    ];
    let found = references.into_iter().flatten().find_map(|reference| {
        let anchor = reference.split_once('#').map(|(_, anchor)| anchor.trim());
        resolve(reference, &base).map(|path| (path, anchor.unwrap_or("")))
    });
    let (target, anchor) = found.unwrap_or_else(|| (base.clone(), ""));

    let herdr = std::env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".into());
    let plugin = std::env::var("HERDR_PLUGIN_ID").context("not running as a herdr plugin")?;
    let mut command = Command::new(herdr);
    command.args([
        "plugin",
        "pane",
        "open",
        "--plugin",
        &plugin,
        "--entrypoint",
        "preview",
    ]);
    command.args([
        "--placement",
        match placement {
            Placement::Split => "split",
            Placement::Tab => "tab",
            Placement::Overlay => "overlay",
            Placement::Zoomed => "zoomed",
        },
    ]);
    if matches!(placement, Placement::Split) {
        command.args(["--direction", "right"]);
        if let Some(pane) = text(context, "focused_pane_id") {
            command.args(["--target-pane", pane]);
        }
    }
    command.arg("--focus");
    command
        .arg("--env")
        .arg(format!("{TARGET_ENV}={}", target.display()));
    command
        .arg("--env")
        .arg(format!("{ROOT_ENV}={}", base.display()));
    if !anchor.is_empty() {
        command.arg("--env").arg(format!("{ANCHOR_ENV}={anchor}"));
    }
    let output = command.output().context("cannot run herdr")?;
    if !output.status.success() {
        bail!(
            "herdr refused to open the preview pane: {}{}",
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_resolve_to_existing_paths_only() {
        let dir = std::env::temp_dir().join(format!("mdview-herdr-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        let file = dir.join("docs/my notes.md");
        std::fs::write(&file, "# x").unwrap();

        assert_eq!(resolve("docs/my notes.md", &dir), Some(file.clone()));
        assert_eq!(
            resolve(" `docs/my%20notes.md#intro` ", &dir),
            Some(file.clone())
        );
        assert_eq!(resolve("docs/my notes.md:12:3", &dir), Some(file.clone()));
        let url = format!("file://{}", file.display()).replace(' ', "%20");
        assert_eq!(resolve(&url, Path::new("/")), Some(file.clone()));
        assert_eq!(resolve("docs/missing.md", &dir), None);
        assert_eq!(resolve("https://example.com/readme.md", &dir), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_focused_pane_directory_wins_over_the_workspace() {
        let here = std::env::temp_dir();
        let context = serde_json::json!({
            "workspace_cwd": "/",
            "focused_pane_cwd": here.to_string_lossy(),
        });
        assert_eq!(pane_directory(&context), Some(here));
        assert_eq!(pane_directory(&Value::Null), None);
    }
}

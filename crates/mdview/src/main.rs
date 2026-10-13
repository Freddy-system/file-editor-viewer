mod herdr;

use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use clap::{Parser, Subcommand};

/// Markdown previewer with diagrams, math and live reload.
#[derive(Parser)]
#[command(name = "mdview", version, args_conflicts_with_subcommands = true)]
struct Cli {
    /// Markdown file or directory to view in the terminal.
    path: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Serve a live-reloading browser preview.
    Serve {
        /// Markdown file or directory.
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Port to listen on; 0 picks a free one.
        #[arg(long, default_value_t = 0)]
        port: u16,
        /// Do not open the browser.
        #[arg(long)]
        no_open: bool,
    },
    /// Preview in a herdr pane beside this one; outside herdr, view here.
    Open {
        /// Markdown file or directory.
        #[arg(default_value = ".")]
        path: PathBuf,
        #[arg(long, value_enum, default_value_t = herdr::Placement::Split)]
        placement: herdr::Placement,
    },
    /// Write a Markdown file as a standalone HTML page.
    Export {
        file: PathBuf,
        /// Output file; defaults to the input with an .html extension.
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Entry points used by the herdr plugin manifest.
    #[command(subcommand)]
    Herdr(herdr::HerdrCmd),
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        None => view(&cli.path.unwrap_or_else(|| PathBuf::from(".")), None, None),
        Some(Cmd::Serve {
            path,
            port,
            no_open,
        }) => serve(&path, port, !no_open),
        Some(Cmd::Open { path, placement }) => open(&path, placement),
        Some(Cmd::Export { file, output }) => export(&file, output),
        Some(Cmd::Herdr(command)) => herdr::run(command),
    }
}

/// Opens the terminal viewer on a file, or on a directory's documents.
fn view(target: &Path, root: Option<PathBuf>, anchor: Option<String>) -> anyhow::Result<()> {
    if !target.exists() {
        bail!("{} does not exist", target.display());
    }
    let options = if target.is_dir() {
        mdview_tui::Options {
            root: target.to_path_buf(),
            file: None,
            anchor: None,
        }
    } else {
        let parent = match target.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
            _ => PathBuf::from("."),
        };
        mdview_tui::Options {
            root: root.filter(|dir| dir.is_dir()).unwrap_or(parent),
            file: Some(target.to_path_buf()),
            anchor,
        }
    };
    mdview_tui::run(options)
}

fn open(path: &Path, placement: herdr::Placement) -> anyhow::Result<()> {
    if std::env::var_os("HERDR_ENV").is_none() {
        return view(path, None, None);
    }
    let target = path
        .canonicalize()
        .with_context(|| format!("{} does not exist", path.display()))?;
    let base = std::env::current_dir()?;
    let pane = std::env::var("HERDR_PANE_ID").ok();
    herdr::open_pane(
        herdr::PLUGIN_ID,
        &target,
        &base,
        "",
        placement,
        pane.as_deref(),
    )
}

fn serve(path: &Path, port: u16, open: bool) -> anyhow::Result<()> {
    let root = if path.is_dir() {
        path
    } else {
        match path.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => dir,
            _ => Path::new("."),
        }
    };
    let server = mdview_web::Server::start(root, port)?;
    let url = server.url_for(path);
    println!("Serving {} at {url}", root.display());
    println!("Press Ctrl-C to stop.");
    if open {
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else if cfg!(windows) {
            "explorer"
        } else {
            "xdg-open"
        };
        let _ = std::process::Command::new(opener).arg(&url).spawn();
    }
    server.wait();
    Ok(())
}

fn export(file: &Path, output: Option<PathBuf>) -> anyhow::Result<()> {
    let source =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let options = mdview_web::PageOptions {
        fallback_title: file
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        ..Default::default()
    };
    let output = output.unwrap_or_else(|| file.with_extension("html"));
    std::fs::write(&output, mdview_web::render_page(&source, &options))
        .with_context(|| format!("cannot write {}", output.display()))?;
    println!("{}", output.display());
    Ok(())
}

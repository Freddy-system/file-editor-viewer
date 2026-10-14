//! How pictures reach the screen: the Kitty graphics protocol where the
//! terminal has it, half-block characters everywhere else.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use base64::Engine as _;
use image::RgbaImage;
use ratatui::style::{Color, Style};
use ratatui::text::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Graphics {
    /// Real images through the Kitty graphics protocol.
    Kitty,
    /// Diagrams as box-drawing text, images as half blocks.
    Text,
}

impl Graphics {
    pub fn name(self) -> &'static str {
        match self {
            Graphics::Kitty => "kitty",
            Graphics::Text => "text",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "kitty" | "image" | "images" => Some(Graphics::Kitty),
            "text" | "blocks" | "off" => Some(Graphics::Text),
            _ => None,
        }
    }

    /// `FEV_GRAPHICS`, then the saved choice, then a guess from the terminal.
    pub fn resolve() -> Self {
        if let Some(mode) = std::env::var("FEV_GRAPHICS")
            .ok()
            .and_then(|v| Self::parse(&v))
        {
            return mode;
        }
        if let Some(mode) = std::fs::read_to_string(saved_choice_path())
            .ok()
            .and_then(|v| Self::parse(&v))
        {
            return mode;
        }
        Self::detect()
    }

    pub fn save(self) {
        let path = saved_choice_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, self.name());
    }

    fn detect() -> Self {
        let names = if std::env::var_os("HERDR_ENV").is_some() {
            // Inside herdr every pane sees herdr as its terminal; what decides
            // whether images show is the terminal the herdr client runs in.
            herdr_client_ancestors()
        } else {
            let mut names = Vec::new();
            for var in ["TERM", "TERM_PROGRAM"] {
                names.extend(std::env::var(var).ok());
            }
            for (var, name) in [
                ("KITTY_WINDOW_ID", "kitty"),
                ("GHOSTTY_RESOURCES_DIR", "ghostty"),
                ("KONSOLE_VERSION", "konsole"),
            ] {
                if std::env::var_os(var).is_some() {
                    names.push(name.to_string());
                }
            }
            names
        };
        let supported = names.iter().any(|name| {
            let name = name.to_ascii_lowercase();
            ["kitty", "ghostty", "wezterm", "konsole", "warp"]
                .iter()
                .any(|t| name.contains(t))
        });
        if supported {
            Graphics::Kitty
        } else {
            Graphics::Text
        }
    }
}

pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir);
    base.join("fev")
}

fn saved_choice_path() -> PathBuf {
    config_dir().join("graphics")
}

/// Names of the processes above each running herdr client.
fn herdr_client_ancestors() -> Vec<String> {
    let Ok(output) = Command::new("ps")
        .args(["-eo", "pid=,ppid=,args="])
        .output()
    else {
        return Vec::new();
    };
    let mut procs: HashMap<u32, (u32, String, bool)> = HashMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut parts = line.split_whitespace();
        let (Some(pid), Some(ppid), Some(program)) = (
            parts.next().and_then(|p| p.parse().ok()),
            parts.next().and_then(|p| p.parse().ok()),
            parts.next(),
        ) else {
            continue;
        };
        let name = program.rsplit('/').next().unwrap_or(program).to_string();
        let is_server = parts.any(|arg| arg == "server");
        procs.insert(pid, (ppid, name, is_server));
    }
    let mut names = Vec::new();
    for (ppid, name, is_server) in procs.values() {
        if name != "herdr" || *is_server {
            continue;
        }
        let mut parent = *ppid;
        for _ in 0..32 {
            let Some((next, name, _)) = procs.get(&parent) else {
                break;
            };
            names.push(name.clone());
            parent = *next;
        }
    }
    names
}

pub fn hash_of(value: impl Hash) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

/// Draws an image with upper-half blocks: one cell is two stacked pixels.
pub fn half_blocks(
    image: &RgbaImage,
    cols: u16,
    rows: u16,
    background: [u8; 3],
) -> Vec<Vec<Span<'static>>> {
    let scaled = image::imageops::resize(
        image,
        cols.max(1) as u32,
        rows.max(1) as u32 * 2,
        image::imageops::FilterType::Triangle,
    );
    let pixel = |x: u32, y: u32| {
        let p = scaled.get_pixel(x, y);
        let a = p[3] as u32;
        let mix = |c: u8, b: u8| ((c as u32 * a + b as u32 * (255 - a)) / 255) as u8;
        Color::Rgb(
            mix(p[0], background[0]),
            mix(p[1], background[1]),
            mix(p[2], background[2]),
        )
    };
    (0..rows as u32)
        .map(|row| {
            (0..cols as u32)
                .map(|col| {
                    Span::styled(
                        "▀",
                        Style::new()
                            .fg(pixel(col, row * 2))
                            .bg(pixel(col, row * 2 + 1)),
                    )
                })
                .collect()
        })
        .collect()
}

/// A visible part of an image, in screen cells.
#[derive(Clone)]
pub struct Placement {
    pub key: u64,
    pub image: Arc<RgbaImage>,
    pub x: u16,
    pub y: u16,
    pub cols: u16,
    /// Rows of the image on screen, starting at `first_row`.
    pub rows: u16,
    pub first_row: u16,
    pub total_rows: u16,
}

impl Placement {
    fn signature(&self) -> (u64, u16, u16, u16, u16, u16) {
        (
            self.key,
            self.x,
            self.y,
            self.cols,
            self.rows,
            self.first_row,
        )
    }
}

/// Keeps the terminal's image placements in step with what is on screen.
#[derive(Default)]
pub struct KittyScreen {
    ids: HashMap<u64, u32>,
    next_id: u32,
    shown: Option<u64>,
}

impl KittyScreen {
    /// Places exactly `placements`, uploading images the terminal lacks.
    pub fn show(&mut self, out: &mut impl Write, placements: &[Placement]) -> io::Result<()> {
        let signature = hash_of(
            placements
                .iter()
                .map(Placement::signature)
                .collect::<Vec<_>>(),
        );
        if self.shown == Some(signature) {
            return Ok(());
        }
        self.shown = Some(signature);
        write!(out, "\x1b7\x1b_Ga=d,d=a,q=2\x1b\\")?;
        for placement in placements {
            let id = match self.ids.get(&placement.key) {
                Some(id) => *id,
                None => {
                    self.next_id += 1;
                    let id = self.next_id;
                    transmit(out, id, &placement.image)?;
                    self.ids.insert(placement.key, id);
                    id
                }
            };
            let height = placement.image.height() as u64;
            let total = placement.total_rows.max(1) as u64;
            let top = height * placement.first_row as u64 / total;
            let bottom =
                (height * (placement.first_row + placement.rows) as u64 / total).min(height);
            write!(
                out,
                "\x1b[{};{}H\x1b_Ga=p,i={id},p=1,x=0,y={top},w={},h={},c={},r={},C=1,q=2\x1b\\",
                placement.y + 1,
                placement.x + 1,
                placement.image.width(),
                (bottom - top).max(1),
                placement.cols,
                placement.rows,
            )?;
        }
        write!(out, "\x1b8")?;
        out.flush()
    }

    /// Makes the next [`show`](Self::show) place everything again, for when
    /// the screen was cleared behind our back.
    pub fn forget_screen(&mut self) {
        self.shown = None;
    }

    /// Frees images that the current document no longer uses.
    pub fn retain(&mut self, out: &mut impl Write, keys: &[u64]) -> io::Result<()> {
        let stale: Vec<u64> = self
            .ids
            .keys()
            .filter(|k| !keys.contains(k))
            .copied()
            .collect();
        for key in stale {
            if let Some(id) = self.ids.remove(&key) {
                write!(out, "\x1b_Ga=d,d=I,i={id},q=2\x1b\\")?;
            }
        }
        self.shown = None;
        out.flush()
    }

    /// Removes everything, e.g. before leaving or when switching mode.
    pub fn clear(&mut self, out: &mut impl Write) -> io::Result<()> {
        self.retain(out, &[])?;
        write!(out, "\x1b_Ga=d,d=a,q=2\x1b\\")?;
        out.flush()
    }
}

fn transmit(out: &mut impl Write, id: u32, image: &RgbaImage) -> io::Result<()> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(image.as_raw());
    let mut chunks = encoded.as_bytes().chunks(4096).peekable();
    let mut first = true;
    while let Some(chunk) = chunks.next() {
        let more = u8::from(chunks.peek().is_some());
        if first {
            write!(
                out,
                "\x1b_Ga=t,f=32,i={id},s={},v={},q=2,m={more};",
                image.width(),
                image.height()
            )?;
            first = false;
        } else {
            write!(out, "\x1b_Gm={more};")?;
        }
        out.write_all(chunk)?;
        write!(out, "\x1b\\")?;
    }
    Ok(())
}

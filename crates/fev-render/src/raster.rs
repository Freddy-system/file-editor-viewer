use std::path::Path;
use std::sync::{Arc, OnceLock};

use anyhow::{anyhow, Context};
use image::RgbaImage;
use resvg::{tiny_skia, usvg};

/// Widest raster we produce; larger drawings are scaled down to it.
const MAX_WIDTH: f32 = 3200.0;
const MAX_HEIGHT: f32 = 12000.0;

fn fonts() -> Arc<usvg::fontdb::Database> {
    static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut db = usvg::fontdb::Database::new();
            db.load_system_fonts();
            Arc::new(db)
        })
        .clone()
}

/// Rasterizes an SVG at `scale` device pixels per SVG unit.
pub fn rasterize_svg(svg: &str, scale: f32) -> anyhow::Result<RgbaImage> {
    let options = usvg::Options {
        fontdb: fonts(),
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_str(svg, &options).context("invalid SVG")?;
    let size = tree.size();
    let scale = scale
        .min(MAX_WIDTH / size.width())
        .min(MAX_HEIGHT / size.height());
    let width = (size.width() * scale).ceil().max(1.0) as u32;
    let height = (size.height() * scale).ceil().max(1.0) as u32;
    let mut pixmap =
        tiny_skia::Pixmap::new(width, height).ok_or_else(|| anyhow!("SVG has no drawable area"))?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    // tiny-skia stores premultiplied alpha; the rest of fev wants straight.
    let mut data = pixmap.take();
    for px in data.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a != 0 && a != 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    RgbaImage::from_raw(width, height, data).ok_or_else(|| anyhow!("raster size mismatch"))
}

/// Loads a raster or SVG image from disk. SVGs are rasterized at `scale`.
pub fn load_image(path: &Path, scale: f32) -> anyhow::Result<RgbaImage> {
    let bytes = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    let is_svg = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("svg"));
    if is_svg {
        return rasterize_svg(&String::from_utf8_lossy(&bytes), scale);
    }
    Ok(image::load_from_memory(&bytes)
        .with_context(|| format!("cannot decode {}", path.display()))?
        .to_rgba8())
}

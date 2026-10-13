use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use anyhow::{anyhow, Context};
use mdview_core::is_markdown_path;
use tiny_http::{Header, Response, StatusCode};

use crate::page::{escape, render_page, PageOptions};

/// Most documents listed in the sidebar and index.
const MAX_LISTED: usize = 300;

/// Local preview server for one directory. Stops when dropped.
pub struct Server {
    http: Arc<tiny_http::Server>,
    root: PathBuf,
    port: u16,
}

impl Server {
    /// Serves `root` on the loopback interface. Port 0 picks a free one.
    pub fn start(root: &Path, port: u16) -> anyhow::Result<Server> {
        let root = root
            .canonicalize()
            .with_context(|| format!("cannot open {}", root.display()))?;
        let http = tiny_http::Server::http(("127.0.0.1", port))
            .map_err(|e| anyhow!("cannot start the preview server: {e}"))?;
        let port = http
            .server_addr()
            .to_ip()
            .map(|addr| addr.port())
            .ok_or_else(|| anyhow!("preview server has no TCP address"))?;
        let http = Arc::new(http);
        let worker = http.clone();
        let served = root.clone();
        std::thread::spawn(move || {
            for request in worker.incoming_requests() {
                let response = respond(&served, request.url());
                let _ = request.respond(response);
            }
        });
        Ok(Server { http, root, port })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Address of the page for `file`, or of the index when it is outside
    /// the served directory.
    pub fn url_for(&self, file: &Path) -> String {
        let base = format!("http://127.0.0.1:{}/", self.port);
        let Ok(file) = file.canonicalize() else {
            return base;
        };
        match file.strip_prefix(&self.root) {
            Ok(relative) => format!("{base}{}", encode_path(relative)),
            Err(_) => base,
        }
    }

    /// Blocks the calling thread for as long as the server runs.
    pub fn wait(&self) {
        loop {
            std::thread::park();
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.http.unblock();
    }
}

fn encode_path(path: &Path) -> String {
    path.components()
        .map(|part| {
            let mut out = String::new();
            for byte in part.as_os_str().to_string_lossy().bytes() {
                if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
                    out.push(byte as char);
                } else {
                    out.push_str(&format!("%{byte:02X}"));
                }
            }
            out
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn decode(text: &str) -> String {
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

type Reply = Response<std::io::Cursor<Vec<u8>>>;

fn reply(status: u16, content_type: &str, body: Vec<u8>) -> Reply {
    let mut response = Response::from_data(body).with_status_code(StatusCode(status));
    for (name, value) in [
        ("Content-Type", content_type),
        ("Cache-Control", "no-store"),
    ] {
        if let Ok(header) = Header::from_bytes(name.as_bytes(), value.as_bytes()) {
            response.add_header(header);
        }
    }
    response
}

fn html(body: String) -> Reply {
    reply(200, "text/html; charset=utf-8", body.into_bytes())
}

fn not_found() -> Reply {
    reply(404, "text/plain; charset=utf-8", b"Not found".to_vec())
}

/// Maps a request path to a file inside `root`, refusing anything that
/// escapes it.
fn resolve(root: &Path, url_path: &str) -> Option<PathBuf> {
    let relative = decode(url_path.trim_start_matches('/'));
    let path = root.join(relative).canonicalize().ok()?;
    path.starts_with(root).then_some(path)
}

fn stamp(path: &Path) -> String {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos().to_string())
        .unwrap_or_default()
}

fn markdown_files(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.')
                || matches!(name.as_ref(), "node_modules" | "target" | "vendor")
            {
                continue;
            }
            if path.is_dir() {
                pending.push(path);
            } else if is_markdown_path(&path) {
                found.push(path);
            }
            if found.len() >= MAX_LISTED {
                found.sort();
                return found;
            }
        }
    }
    found.sort();
    found
}

fn file_links(root: &Path) -> Vec<(String, String)> {
    markdown_files(root)
        .iter()
        .filter_map(|path| path.strip_prefix(root).ok())
        .map(|relative| {
            (
                relative.to_string_lossy().into_owned(),
                format!("/{}", encode_path(relative)),
            )
        })
        .collect()
}

fn respond(root: &Path, url: &str) -> Reply {
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    if path == "/__stamp" {
        let target = query.strip_prefix("path=").unwrap_or("");
        return match resolve(root, target) {
            Some(file) => reply(200, "text/plain", stamp(&file).into_bytes()),
            None => not_found(),
        };
    }
    let Some(file) = resolve(root, path) else {
        return not_found();
    };
    if file.is_dir() {
        let readme = ["README.md", "readme.md", "index.md"]
            .iter()
            .map(|name| file.join(name))
            .find(|candidate| candidate.is_file());
        return match readme {
            Some(readme) => document(root, &readme),
            None => index(root),
        };
    }
    if is_markdown_path(&file) {
        return document(root, &file);
    }
    match std::fs::read(&file) {
        Ok(bytes) => reply(200, content_type(&file), bytes),
        Err(_) => not_found(),
    }
}

fn document(root: &Path, file: &Path) -> Reply {
    let Ok(source) = std::fs::read_to_string(file) else {
        return not_found();
    };
    let relative = file.strip_prefix(root).unwrap_or(file);
    let options = PageOptions {
        fallback_title: relative.to_string_lossy().into_owned(),
        stamp_url: Some(format!("/__stamp?path={}", encode_path(relative))),
        files: file_links(root),
    };
    html(render_page(&source, &options))
}

fn index(root: &Path) -> Reply {
    let mut source = String::from("# Documents\n\n<div class=\"file-index\">\n");
    for (label, href) in file_links(root) {
        source.push_str(&format!(
            "<a href=\"{}\">{}</a>\n",
            escape(&href),
            escape(&label)
        ));
    }
    source.push_str("</div>\n");
    html(render_page(&source, &PageOptions::default()))
}

fn content_type(path: &Path) -> &'static str {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "pdf" => "application/pdf",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "html" | "htm" => "text/html; charset=utf-8",
        "txt" | "log" | "toml" | "yaml" | "yml" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(response: Reply) -> (u16, String) {
        let status = response.status_code().0;
        let mut text = String::new();
        std::io::Read::read_to_string(&mut response.into_reader(), &mut text).unwrap();
        (status, text)
    }

    fn fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mdview-web-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        std::fs::write(dir.join("README.md"), "# Home\n").unwrap();
        std::fs::write(dir.join("docs/my guide.md"), "# Guide\n").unwrap();
        std::fs::write(dir.join("docs/pic.png"), [1u8, 2, 3]).unwrap();
        dir.canonicalize().unwrap()
    }

    #[test]
    fn serves_documents_assets_and_the_directory_readme() {
        let root = fixture("serve");
        let (status, home) = body(respond(&root, "/"));
        assert_eq!(status, 200);
        assert!(home.contains("<h1 id=\"home\">Home</h1>"));
        assert!(home.contains("href=\"/docs/my%20guide.md\""));
        assert!(home.contains("data-stamp=\"/__stamp?path=README.md\""));

        let (status, guide) = body(respond(&root, "/docs/my%20guide.md"));
        assert_eq!(status, 200);
        assert!(guide.contains("Guide"));

        let image = respond(&root, "/docs/pic.png");
        assert_eq!(image.status_code().0, 200);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_paths_outside_the_served_directory() {
        let root = fixture("escape");
        for url in [
            "/../",
            "/docs/../../etc/passwd",
            "/%2e%2e/%2e%2e/etc/passwd",
            "/__stamp?path=../x",
        ] {
            assert_eq!(body(respond(&root.join("docs"), url)).0, 404, "{url}");
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_stamp_changes_when_the_file_does() {
        let root = fixture("stamp");
        let before = body(respond(&root, "/__stamp?path=README.md")).1;
        assert!(!before.is_empty());
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(root.join("README.md"), "# Changed\n").unwrap();
        assert_ne!(before, body(respond(&root, "/__stamp?path=README.md")).1);
        std::fs::remove_dir_all(root).unwrap();
    }
}

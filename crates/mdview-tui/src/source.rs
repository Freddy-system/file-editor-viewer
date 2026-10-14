//! Files that are not Markdown, shown as one highlighted code listing.

use std::path::Path;

use anyhow::{bail, Context};
use mdview_core::{Block, Document};

/// Largest file shown as a listing; bigger ones are refused.
pub const MAX_BYTES: u64 = 2 * 1024 * 1024;

/// Whether `bytes` looks like text: valid UTF-8 and no NUL byte.
pub fn is_text(bytes: &[u8]) -> bool {
    !bytes.contains(&0) && std::str::from_utf8(bytes).is_ok()
}

/// Syntax token for a path, as syntect's `find_syntax_by_token` expects:
/// normally the lowercased extension, a fitting token for well-known names
/// that have no telling extension, and an empty string when unknown.
pub fn language_for(path: &Path) -> String {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let known = match name {
        "Dockerfile" | "Containerfile" => "dockerfile",
        "Makefile" | "makefile" | "GNUmakefile" => "makefile",
        ".bashrc" | ".bash_profile" | ".bash_aliases" | ".zshrc" | ".zshenv" | ".zprofile"
        | ".profile" | ".env" => "sh",
        "Cargo.lock" => "toml",
        _ if name.starts_with("Dockerfile.") => "dockerfile",
        _ if name.starts_with(".env.") => "sh",
        _ => "",
    };
    if !known.is_empty() {
        return known.to_string();
    }
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default()
}

/// Reads `path` for display as a listing. Fails with a short message for the
/// user when the file is larger than [`MAX_BYTES`], is not text, or cannot be
/// read.
pub fn read(path: &Path) -> anyhow::Result<String> {
    let size = std::fs::metadata(path)
        .with_context(|| format!("cannot read {}", path.display()))?
        .len();
    if size > MAX_BYTES {
        bail!(
            "{} is too large to show ({} MB limit)",
            path.display(),
            MAX_BYTES / (1024 * 1024)
        );
    }
    let bytes = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    if !is_text(&bytes) {
        bail!("{} is not a text file", path.display());
    }
    Ok(String::from_utf8(bytes)?)
}

/// A document that shows `text` as one code listing in the language of `path`.
pub fn document(path: &Path, text: &str) -> Document {
    Document {
        blocks: vec![Block::CodeBlock {
            lang: language_for(path),
            code: text.strip_suffix('\n').unwrap_or(text).to_string(),
        }],
        ..Document::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("mdview-source-{}-{name}", std::process::id()))
    }

    #[test]
    fn text_is_valid_utf8_without_nul_bytes() {
        assert!(is_text("fn main() {}\n".as_bytes()));
        assert!(is_text("año ✓".as_bytes()));
        assert!(is_text(b""));
        assert!(!is_text(b"ELF\0\x01\x02"));
        assert!(!is_text(&[0xff, 0xfe, b'a']));
    }

    #[test]
    fn language_is_the_lowercased_extension() {
        assert_eq!(language_for(Path::new("src/main.rs")), "rs");
        assert_eq!(language_for(Path::new("tool.py")), "py");
        assert_eq!(language_for(Path::new("Cargo.toml")), "toml");
        assert_eq!(language_for(Path::new("PHOTO.JSON")), "json");
        assert_eq!(language_for(Path::new("archive.tar.GZ")), "gz");
    }

    #[test]
    fn well_known_names_have_a_language_without_an_extension() {
        assert_eq!(language_for(Path::new("Dockerfile")), "dockerfile");
        assert_eq!(
            language_for(Path::new("docker/Dockerfile.dev")),
            "dockerfile"
        );
        assert_eq!(language_for(Path::new("Makefile")), "makefile");
        assert_eq!(language_for(Path::new("/home/me/.bashrc")), "sh");
        assert_eq!(language_for(Path::new(".zshrc")), "sh");
        assert_eq!(language_for(Path::new(".profile")), "sh");
        assert_eq!(language_for(Path::new(".env")), "sh");
        assert_eq!(language_for(Path::new("Cargo.lock")), "toml");
    }

    #[test]
    fn unknown_names_have_no_language() {
        assert_eq!(language_for(Path::new("LICENSE")), "");
        assert_eq!(language_for(Path::new(".gitignore")), "");
        assert_eq!(language_for(Path::new("")), "");
    }

    #[test]
    fn document_is_a_single_code_block_in_the_language_of_the_path() {
        let doc = document(Path::new("src/main.rs"), "fn main() {}\n\n");
        assert!(doc.frontmatter.is_none() && doc.footnotes.is_empty());
        assert_eq!(doc.blocks.len(), 1);
        match &doc.blocks[0] {
            Block::CodeBlock { lang, code } => {
                assert_eq!(lang, "rs");
                assert_eq!(code, "fn main() {}\n");
            }
            other => panic!("expected a code block, got {other:?}"),
        }
        match &document(Path::new("notes"), "no newline").blocks[0] {
            Block::CodeBlock { lang, code } => {
                assert_eq!(lang, "");
                assert_eq!(code, "no newline");
            }
            other => panic!("expected a code block, got {other:?}"),
        }
    }

    #[test]
    fn reading_returns_the_text_of_a_text_file() {
        let path = temp_file("text.rs");
        std::fs::write(&path, "fn main() {}\n").unwrap();
        let text = read(&path);
        std::fs::remove_file(&path).unwrap();
        assert_eq!(text.unwrap(), "fn main() {}\n");
    }

    #[test]
    fn reading_refuses_binary_files() {
        let path = temp_file("binary.bin");
        std::fs::write(&path, [0x7f, b'E', b'L', b'F', 0, 1, 2]).unwrap();
        let result = read(&path);
        std::fs::remove_file(&path).unwrap();
        let message = result.unwrap_err().to_string();
        assert!(message.contains("not a text file"), "{message}");
    }

    #[test]
    fn reading_fails_for_a_missing_file() {
        let path = temp_file("missing.txt");
        assert!(read(&path).is_err());
    }
}

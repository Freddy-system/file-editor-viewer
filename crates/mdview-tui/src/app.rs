use std::io::stdout;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

use anyhow::Context;
use mdview_core::{is_markdown_path, Document};
use notify::{RecursiveMode, Watcher};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::editor::{Editor, Outcome};
use crate::graphics::{Graphics, KittyScreen, Placement};
use crate::layout::{
    layout, percent_decode, resolve_local, Assets, LayoutOptions, Rendered, DEFAULT_CELL,
};
use crate::source;
use crate::theme::Theme;
use crate::tree::Tree;
use crate::Options;

/// Text is easier to read in a bounded column than across a very wide pane.
const MAX_TEXT_WIDTH: u16 = 120;
const MAX_LISTED_FILES: usize = 5000;

/// The help window: groups of keys and what they do.
const HELP: &[(&str, &[(&str, &str)])] = &[
    (
        "Move",
        &[
            ("↑ ↓  j k  wheel", "Scroll"),
            ("Space  d u", "Page, half page"),
            ("g  G", "Top, bottom"),
            ("t", "Table of contents"),
            ("b", "File tree"),
            ("f", "Choose another file"),
            ("Backspace", "Previous document"),
        ],
    ),
    (
        "Find and follow",
        &[
            ("/", "Search"),
            ("n  N", "Next, previous match"),
            ("Tab  Shift-Tab", "Select a link"),
            ("Enter  click", "Follow the link"),
        ],
    ),
    (
        "Copy",
        &[
            ("drag", "Select text, copied on release"),
            ("y  click on code", "Copy a code block"),
            ("Y  Ctrl-A", "Copy the whole document"),
        ],
    ),
    (
        "View",
        &[
            ("e", "Edit this file"),
            ("o", "Open in the browser"),
            ("i", "Images: kitty or text"),
            ("c", "Light or dark colours"),
            ("r", "Reload"),
            ("q", "Quit"),
        ],
    ),
];

/// A position in the laid-out document: line and column.
type Point = (usize, u16);

#[derive(PartialEq, Eq, Clone, Copy)]
enum Mode {
    View,
    Picker,
    Toc,
    Search,
    Help,
    Edit,
}

struct OpenDoc {
    path: PathBuf,
    source: String,
    /// A non-Markdown file, shown as a numbered listing.
    listing: bool,
    doc: Document,
    rendered: Rendered,
    /// Width `rendered` was laid out for; 0 forces a new layout.
    width: u16,
    scroll: usize,
}

struct App {
    root: PathBuf,
    theme: Theme,
    graphics: Graphics,
    cell: (u32, u32),
    assets: Assets,
    kitty: KittyScreen,
    doc: Option<OpenDoc>,
    history: Vec<(PathBuf, usize)>,
    mode: Mode,
    files: Vec<PathBuf>,
    filter: String,
    selected: usize,
    query: String,
    search: String,
    /// Line, start column and end column of every search match.
    matches: Vec<(usize, u16, u16)>,
    /// Line and index within that line of the focused link.
    link_focus: Option<(usize, usize)>,
    /// Where the left button went down, until it is released.
    press: Option<Point>,
    /// Text selected by dragging, from where the drag began to where it is.
    selection: Option<(Point, Point)>,
    pending_anchor: Option<String>,
    /// The built-in editor, while the document is being edited.
    editor: Option<Editor>,
    /// The file tree sidebar, loaded the first time it is shown.
    tree: Option<Tree>,
    tree_open: bool,
    tree_focus: bool,
    tree_area: Rect,
    message: Option<String>,
    server: Option<mdview_web::Server>,
    watcher: Option<notify::RecommendedWatcher>,
    watched: Option<PathBuf>,
    changes_tx: Sender<PathBuf>,
    changes: Receiver<PathBuf>,
    /// Where the document was drawn last, for mouse hits.
    view: Rect,
    placements: Vec<Placement>,
    quit: bool,
}

pub fn run(options: Options) -> anyhow::Result<()> {
    let (changes_tx, changes) = channel();
    let mut app = App {
        root: options.root,
        theme: match std::env::var("MDVIEW_THEME").as_deref() {
            Ok("light") => Theme::light(),
            _ => Theme::dark(),
        },
        graphics: Graphics::resolve(),
        cell: cell_size(),
        assets: Assets::default(),
        kitty: KittyScreen::default(),
        doc: None,
        history: Vec::new(),
        mode: Mode::View,
        files: Vec::new(),
        filter: String::new(),
        selected: 0,
        query: String::new(),
        search: String::new(),
        matches: Vec::new(),
        link_focus: None,
        press: None,
        selection: None,
        pending_anchor: None,
        editor: None,
        tree: None,
        tree_open: false,
        tree_focus: false,
        tree_area: Rect::default(),
        message: None,
        server: None,
        watcher: None,
        watched: None,
        changes_tx,
        changes,
        view: Rect::default(),
        placements: Vec::new(),
        quit: false,
    };
    let first = options.file.or_else(|| {
        ["README.md", "readme.md", "Readme.md", "index.md"]
            .iter()
            .map(|name| app.root.join(name))
            .find(|path| path.is_file())
    });
    match first {
        Some(path) => app.open(&path, options.anchor)?,
        None => app.enter_picker(),
    }

    let mut terminal = ratatui::init();
    let mouse = std::env::var("MDVIEW_MOUSE").as_deref() != Ok("0");
    if mouse {
        let _ = execute!(stdout(), EnableMouseCapture);
    }
    let _ = execute!(stdout(), EnableBracketedPaste);
    let result = app.event_loop(&mut terminal);
    let _ = execute!(stdout(), DisableBracketedPaste);
    let _ = app.kitty.clear(&mut stdout());
    if mouse {
        let _ = execute!(stdout(), DisableMouseCapture);
    }
    ratatui::restore();
    result
}

/// `MDVIEW_CELL=WxH` overrides the assumed cell size in pixels.
fn cell_size() -> (u32, u32) {
    std::env::var("MDVIEW_CELL")
        .ok()
        .and_then(|value| {
            let (w, h) = value.split_once(['x', 'X'])?;
            Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
        })
        .filter(|(w, h)| *w > 0 && *h > 0)
        .unwrap_or(DEFAULT_CELL)
}

fn open_external(target: &str) -> std::io::Result<()> {
    let mut command = if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.arg(target);
        c
    } else if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", "", target]);
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(target);
        c
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}

/// Puts `text` on the system clipboard: through a clipboard tool when one is
/// installed, and through the terminal (OSC 52), which also works over SSH.
fn copy_to_clipboard(text: &str) {
    use base64::Engine as _;
    use std::io::Write as _;

    let tools: &[(&str, &[&str], &str)] = &[
        ("wl-copy", &[], "WAYLAND_DISPLAY"),
        ("xclip", &["-selection", "clipboard"], "DISPLAY"),
        ("xsel", &["--clipboard", "--input"], "DISPLAY"),
        ("pbcopy", &[], ""),
        ("clip.exe", &[], ""),
    ];
    for (program, args, needs) in tools {
        if !needs.is_empty() && std::env::var_os(needs).is_none() {
            continue;
        }
        let child = Command::new(program)
            .args(*args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let Ok(mut child) = child else {
            continue;
        };
        let written = child
            .stdin
            .take()
            .is_some_and(|mut stdin| stdin.write_all(text.as_bytes()).is_ok());
        if written && child.wait().is_ok_and(|status| status.success()) {
            break;
        }
    }
    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    let mut out = stdout();
    let _ = write!(out, "\x1b]52;c;{encoded}\x07");
    let _ = out.flush();
}

/// The part of `text` between two display columns.
fn slice_columns(text: &str, from: u16, to: u16) -> &str {
    let mut column = 0u16;
    let mut start = text.len();
    let mut end = text.len();
    for (index, c) in text.char_indices() {
        if column >= from && start == text.len() {
            start = index;
        }
        if column >= to {
            end = index;
            break;
        }
        column += unicode_width::UnicodeWidthChar::width(c).unwrap_or(0) as u16;
    }
    &text[start.min(end)..end]
}

/// Markdown is parsed; any other text file becomes one numbered listing.
fn parse_document(path: &Path, text: &str, listing: bool) -> Document {
    if listing {
        source::document(path, text)
    } else {
        mdview_core::parse(text)
    }
}

/// 1-based source line of the `index`-th heading written with `#`s, skipping
/// fenced code.
fn heading_source_line(source: &str, index: usize) -> Option<usize> {
    let mut fence: Option<&str> = None;
    let mut seen = 0;
    for (number, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        let marker = ["```", "~~~"].into_iter().find(|m| trimmed.starts_with(m));
        match (fence, marker) {
            (None, Some(marker)) => fence = Some(marker),
            (Some(open), Some(marker)) if open == marker => fence = None,
            (None, None) if trimmed.starts_with('#') && line.len() - trimmed.len() < 4 => {
                if seen == index {
                    return Some(number + 1);
                }
                seen += 1;
            }
            _ => {}
        }
    }
    None
}

fn fuzzy_match(candidate: &str, filter: &str) -> bool {
    let mut chars = candidate.chars().flat_map(char::to_lowercase);
    filter
        .chars()
        .flat_map(char::to_lowercase)
        .all(|wanted| chars.any(|c| c == wanted))
}

fn popup(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(2)).max(1);
    let height = height.min(area.height.saturating_sub(2)).max(1);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

impl App {
    fn event_loop(&mut self, terminal: &mut ratatui::DefaultTerminal) -> anyhow::Result<()> {
        let mut dirty = true;
        let mut drawn_size = None;
        let mut repaint = false;
        while !self.quit {
            if dirty {
                if repaint {
                    // The multiplexer may have reflowed or shifted what was
                    // on screen, so nothing drawn before can be trusted.
                    terminal.clear()?;
                    self.kitty.forget_screen();
                    repaint = false;
                }
                drawn_size = terminal.size().ok();
                terminal.draw(|frame| self.draw(frame))?;
                if self.graphics == Graphics::Kitty {
                    self.kitty.show(&mut stdout(), &self.placements)?;
                }
                dirty = false;
            }
            if event::poll(Duration::from_millis(150))? {
                // Drain what is queued so a burst of wheel events costs one redraw.
                loop {
                    match event::read()? {
                        Event::Key(key) if key.kind != KeyEventKind::Release => self.key(key),
                        Event::Mouse(mouse) => self.mouse(mouse),
                        Event::Paste(text) => {
                            if let Some(editor) = &mut self.editor {
                                let outcome = editor.paste(&text);
                                self.apply(outcome);
                            }
                        }
                        Event::Resize(..) | Event::FocusGained => repaint = true,
                        _ => {}
                    }
                    if self.quit || !event::poll(Duration::ZERO)? {
                        break;
                    }
                }
                dirty = true;
            }
            // A resize that lands while a slow first draw is still running
            // produces no event, so the size is also checked directly.
            if terminal.size().ok() != drawn_size {
                repaint = true;
                dirty = true;
            }
            let mut changed = false;
            while let Ok(path) = self.changes.try_recv() {
                changed |= self
                    .doc
                    .as_ref()
                    .is_some_and(|d| d.path.file_name() == path.file_name());
            }
            if changed {
                self.reload();
                dirty = true;
            }
        }
        Ok(())
    }

    fn open(&mut self, path: &Path, anchor: Option<String>) -> anyhow::Result<()> {
        let path = path
            .canonicalize()
            .with_context(|| format!("cannot open {}", path.display()))?;
        let listing = !is_markdown_path(&path);
        let source = if listing {
            source::read(&path)?
        } else {
            std::fs::read_to_string(&path)
                .with_context(|| format!("cannot read {}", path.display()))?
        };
        self.doc = Some(OpenDoc {
            path: path.clone(),
            doc: parse_document(&path, &source, listing),
            source,
            listing,
            rendered: Rendered::default(),
            width: 0,
            scroll: 0,
        });
        self.pending_anchor = anchor;
        self.link_focus = None;
        self.search.clear();
        self.mode = Mode::View;
        if let Some(tree) = &mut self.tree {
            tree.reveal(&path);
        }
        self.watch(&path);
        Ok(())
    }

    /// Re-reads the current document after it changed on disk.
    fn reload(&mut self) {
        // The editor holds the text while it is open; the document is read
        // again when it closes.
        if self.editor.is_some() {
            return;
        }
        let Some(doc) = &mut self.doc else {
            return;
        };
        // Editors that replace the file leave a moment with nothing to read.
        let Ok(source) = std::fs::read_to_string(&doc.path) else {
            return;
        };
        doc.doc = parse_document(&doc.path, &source, doc.listing);
        doc.source = source;
        doc.width = 0;
        self.link_focus = None;
    }

    /// Opens the built-in editor on the current file, at the part being read.
    fn start_edit(&mut self) {
        let Some(doc) = &self.doc else {
            return;
        };
        let line = if doc.listing {
            doc.scroll.max(1)
        } else {
            doc.rendered
                .toc
                .iter()
                .rposition(|entry| entry.2 <= doc.scroll)
                .and_then(|index| heading_source_line(&doc.source, index))
                .unwrap_or(1)
        };
        self.editor = Some(Editor::new(&doc.path, &doc.source, line, &self.theme));
        self.mode = Mode::Edit;
        self.selection = None;
        self.link_focus = None;
        self.tree_focus = false;
    }

    /// Reacts to what the editor did with an event.
    fn apply(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::None | Outcome::Changed => {}
            Outcome::Saved => self.message = Some("saved".into()),
            Outcome::Copied(text) => {
                copy_to_clipboard(&text);
                self.message = Some(format!("copied {} characters", text.chars().count()));
            }
            Outcome::Notice(notice) => self.message = Some(notice),
            Outcome::Close => {
                self.editor = None;
                self.mode = Mode::View;
                self.reload();
            }
        }
    }

    /// Shows the file tree and moves to it; from the tree, hides it.
    fn toggle_tree(&mut self) {
        if self.tree_open && self.tree_focus {
            self.tree_open = false;
            self.tree_focus = false;
            return;
        }
        let tree = self
            .tree
            .get_or_insert_with(|| Tree::load(&self.root, false));
        if tree.is_empty() {
            self.message = Some(format!("no Markdown files in {}", self.root.display()));
            return;
        }
        if let Some(doc) = &self.doc {
            tree.reveal(&doc.path);
        }
        self.tree_open = true;
        self.tree_focus = true;
    }

    fn tree_key(&mut self, key: KeyEvent) {
        let Some(tree) = &mut self.tree else {
            self.tree_focus = false;
            return;
        };
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => tree.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => tree.move_by(1),
            KeyCode::PageUp => tree.move_by(-10),
            KeyCode::PageDown => tree.move_by(10),
            KeyCode::Right | KeyCode::Char('l') => tree.expand(),
            KeyCode::Left | KeyCode::Char('h') => tree.collapse(),
            KeyCode::Enter => {
                if let Some(path) = tree.activate() {
                    self.open_from_tree(&path);
                }
            }
            KeyCode::Esc | KeyCode::Tab => self.tree_focus = false,
            KeyCode::Char('b') => self.toggle_tree(),
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('?') => self.mode = Mode::Help,
            _ => {}
        }
    }

    fn open_from_tree(&mut self, path: &Path) {
        let previous = self.doc.as_ref().map(|d| (d.path.clone(), d.scroll));
        match self.open(path, None) {
            Ok(()) => {
                self.history.extend(previous);
                self.tree_focus = false;
            }
            Err(error) => self.message = Some(format!("{error:#}")),
        }
    }

    fn watch(&mut self, path: &Path) {
        let Some(dir) = path.parent().map(Path::to_path_buf) else {
            return;
        };
        if self.watched.as_deref() == Some(&dir) {
            return;
        }
        if self.watcher.is_none() {
            let tx = self.changes_tx.clone();
            self.watcher =
                notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                    if let Ok(event) = event {
                        if event.kind.is_access() {
                            return;
                        }
                        for path in event.paths {
                            let _ = tx.send(path);
                        }
                    }
                })
                .ok();
        }
        if let Some(watcher) = &mut self.watcher {
            if let Some(previous) = self.watched.take() {
                let _ = watcher.unwatch(&previous);
            }
            if watcher.watch(&dir, RecursiveMode::NonRecursive).is_ok() {
                self.watched = Some(dir);
            }
        }
    }

    fn relayout(&mut self, width: u16) {
        let Some(doc) = &mut self.doc else {
            return;
        };
        let base_dir = doc.path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let options = LayoutOptions {
            width,
            theme: &self.theme,
            graphics: self.graphics,
            cell: self.cell,
            base_dir: &base_dir,
            line_numbers: doc.listing,
        };
        doc.rendered = layout(&doc.doc, &options, &mut self.assets);
        doc.width = width;
        // Positions refer to the old layout.
        self.selection = None;
        self.press = None;
        let keys: Vec<u64> = doc.rendered.images.iter().map(|slot| slot.key).collect();
        let _ = self.kitty.retain(&mut stdout(), &keys);
        if let Some(anchor) = self.pending_anchor.take() {
            self.jump_to_anchor(&anchor);
        }
        self.find_matches();
    }

    fn jump_to_anchor(&mut self, anchor: &str) {
        let Some(doc) = &mut self.doc else {
            return;
        };
        let decoded = percent_decode(anchor);
        let line = doc
            .rendered
            .anchors
            .get(&decoded)
            .or_else(|| doc.rendered.anchors.get(&mdview_core::slugify(&decoded)));
        match line {
            Some(line) => doc.scroll = *line,
            None => self.message = Some(format!("no heading #{decoded}")),
        }
    }

    fn find_matches(&mut self) {
        self.matches.clear();
        let Some(doc) = &self.doc else {
            return;
        };
        if self.search.is_empty() {
            return;
        }
        let needle = self.search.to_lowercase();
        for (n, line) in doc.rendered.lines.iter().enumerate() {
            let text = line.text();
            let lower = text.to_lowercase();
            // Lowercasing can change byte lengths; only trust offsets when it
            // did not.
            if lower.len() != text.len() {
                continue;
            }
            let mut from = 0;
            while let Some(found) = lower[from..].find(&needle) {
                let start = from + found;
                let end = start + needle.len();
                if text.is_char_boundary(start) && text.is_char_boundary(end) {
                    self.matches.push((
                        n,
                        text[..start].width() as u16,
                        text[..end].width() as u16,
                    ));
                }
                from = end.max(start + 1);
                while !lower.is_char_boundary(from) {
                    from += 1;
                }
            }
        }
    }

    fn page(&self) -> usize {
        self.view.height.max(1) as usize
    }

    fn scroll_to(&mut self, line: usize) {
        let page = self.page();
        if let Some(doc) = &mut self.doc {
            doc.scroll = line.min(doc.rendered.lines.len().saturating_sub(page));
        }
    }

    fn scroll_by(&mut self, delta: isize) {
        if let Some(doc) = &self.doc {
            self.scroll_to(doc.scroll.saturating_add_signed(delta));
        }
    }

    /// Scrolls just enough to bring `line` on screen.
    fn reveal(&mut self, line: usize) {
        let page = self.page();
        let Some(scroll) = self.doc.as_ref().map(|d| d.scroll) else {
            return;
        };
        if line < scroll || line >= scroll + page {
            self.scroll_to(line.saturating_sub(page / 3));
        }
    }

    fn next_match(&mut self, forward: bool) {
        let Some(scroll) = self.doc.as_ref().map(|d| d.scroll) else {
            return;
        };
        if self.matches.is_empty() {
            if !self.search.is_empty() {
                self.message = Some(format!("no match for \"{}\"", self.search));
            }
            return;
        }
        let target = if forward {
            self.matches
                .iter()
                .find(|m| m.0 > scroll)
                .or(self.matches.first())
        } else {
            self.matches
                .iter()
                .rev()
                .find(|m| m.0 < scroll)
                .or(self.matches.last())
        };
        if let Some(line) = target.map(|m| m.0) {
            self.scroll_to(line);
        }
    }

    fn cycle_link(&mut self, forward: bool) {
        let Some(doc) = &self.doc else {
            return;
        };
        let hits: Vec<(usize, usize)> = doc
            .rendered
            .lines
            .iter()
            .enumerate()
            .flat_map(|(n, line)| (0..line.links.len()).map(move |i| (n, i)))
            .collect();
        if hits.is_empty() {
            self.message = Some("no links in this document".into());
            return;
        }
        let next = match self.link_focus {
            Some(current) => {
                let at = hits.iter().position(|h| *h == current).unwrap_or(0);
                let step = if forward { 1 } else { hits.len() - 1 };
                hits[(at + step) % hits.len()]
            }
            None if forward => *hits.iter().find(|h| h.0 >= doc.scroll).unwrap_or(&hits[0]),
            None => *hits
                .iter()
                .rev()
                .find(|h| h.0 < doc.scroll + self.page())
                .unwrap_or(&hits[hits.len() - 1]),
        };
        self.link_focus = Some(next);
        self.reveal(next.0);
    }

    fn follow(&mut self, link: usize) {
        let Some(doc) = &self.doc else {
            return;
        };
        let Some(url) = doc.rendered.links.get(link).cloned() else {
            return;
        };
        if let Some(anchor) = url.strip_prefix('#') {
            self.history.push((doc.path.clone(), doc.scroll));
            self.jump_to_anchor(anchor);
            return;
        }
        let base_dir = doc.path.parent().unwrap_or(Path::new("."));
        let Some(path) = resolve_local(base_dir, &url) else {
            self.message = Some(match open_external(&url) {
                Ok(()) => format!("opened {url}"),
                Err(error) => format!("cannot open {url}: {error}"),
            });
            return;
        };
        // Wiki links name a page without its extension.
        let path = match path.extension() {
            None if !path.exists() => path.with_extension("md"),
            _ => path,
        };
        if !path.exists() {
            self.message = Some(format!("not found: {}", path.display()));
            return;
        }
        // Text files open here; anything the viewer cannot show (folders,
        // pictures, binaries) goes to the system's own program for it.
        if !path.is_dir() {
            let anchor = url.split_once('#').map(|(_, a)| a.to_string());
            let previous = (doc.path.clone(), doc.scroll);
            match self.open(&path, anchor) {
                Ok(()) => {
                    self.history.push(previous);
                    return;
                }
                Err(error) if is_markdown_path(&path) => {
                    self.message = Some(format!("{error:#}"));
                    return;
                }
                Err(_) => {}
            }
        }
        self.message = Some(match open_external(&path.to_string_lossy()) {
            Ok(()) => format!("opened {}", path.display()),
            Err(error) => format!("cannot open {}: {error}", path.display()),
        });
    }

    fn copy_code(&mut self, index: usize) {
        let Some(code) = self.doc.as_ref().and_then(|d| d.rendered.codes.get(index)) else {
            return;
        };
        copy_to_clipboard(code);
        let lines = code.lines().count();
        self.message = Some(format!(
            "copied {lines} line{}",
            if lines == 1 { "" } else { "s" }
        ));
    }

    /// Copies the whole document as Markdown source.
    fn copy_all(&mut self) {
        let Some(doc) = &self.doc else {
            return;
        };
        copy_to_clipboard(&doc.source);
        self.message = Some(format!(
            "copied the whole document ({} lines)",
            doc.source.lines().count()
        ));
    }

    /// The selection with its ends in reading order.
    fn selected_range(&self) -> Option<(Point, Point)> {
        let (a, b) = self.selection?;
        Some(if a <= b { (a, b) } else { (b, a) })
    }

    fn selected_text(&self) -> Option<String> {
        let (start, end) = self.selected_range()?;
        let doc = self.doc.as_ref()?;
        let mut out = Vec::new();
        for number in start.0..=end.0 {
            let text = doc.rendered.lines.get(number)?.text();
            let from = if number == start.0 { start.1 } else { 0 };
            let to = if number == end.0 { end.1 + 1 } else { u16::MAX };
            out.push(slice_columns(&text, from, to).trim_end().to_string());
        }
        let text = out.join("\n");
        (!text.trim().is_empty()).then_some(text)
    }

    /// Document position under a screen cell, clamped to the view.
    fn point_at(&self, column: u16, row: u16) -> Option<Point> {
        let doc = self.doc.as_ref()?;
        let last_row = self.view.height.checked_sub(1)?;
        let row = row.saturating_sub(self.view.y).min(last_row);
        let line = (doc.scroll + row as usize).min(doc.rendered.lines.len().checked_sub(1)?);
        Some((line, column.saturating_sub(self.view.x)))
    }

    /// Copies the first code block that is on screen.
    fn copy_visible_code(&mut self) {
        let code = self.doc.as_ref().and_then(|doc| {
            doc.rendered
                .lines
                .iter()
                .skip(doc.scroll)
                .take(self.page())
                .find_map(|line| line.code)
        });
        match code {
            Some(code) => self.copy_code(code),
            None => self.message = Some("no code block on screen".into()),
        }
    }

    fn back(&mut self) {
        let Some((path, scroll)) = self.history.pop() else {
            self.message = Some("no earlier document".into());
            return;
        };
        let same = self.doc.as_ref().is_some_and(|d| d.path == path);
        if !same {
            if let Err(error) = self.open(&path, None) {
                self.message = Some(format!("{error:#}"));
                return;
            }
        }
        if let Some(doc) = &mut self.doc {
            doc.scroll = scroll;
        }
    }

    fn enter_picker(&mut self) {
        let mut files: Vec<PathBuf> = ignore::WalkBuilder::new(&self.root)
            .max_depth(Some(12))
            .build()
            .flatten()
            .filter(|entry| entry.file_type().is_some_and(|t| t.is_file()))
            .map(|entry| entry.into_path())
            .filter(|path| is_markdown_path(path))
            .take(MAX_LISTED_FILES)
            .map(|path| path.strip_prefix(&self.root).unwrap_or(&path).to_path_buf())
            .collect();
        files.sort();
        self.files = files;
        self.filter.clear();
        self.selected = 0;
        self.mode = Mode::Picker;
    }

    fn filtered_files(&self) -> Vec<&PathBuf> {
        self.files
            .iter()
            .filter(|path| fuzzy_match(&path.to_string_lossy(), &self.filter))
            .collect()
    }

    fn open_browser(&mut self) {
        let Some(path) = self.doc.as_ref().map(|d| d.path.clone()) else {
            return;
        };
        let covered = self
            .server
            .as_ref()
            .is_some_and(|server| path.starts_with(server.root()));
        if !covered {
            let root = match self.root.canonicalize() {
                Ok(root) if path.starts_with(&root) => root,
                _ => path.parent().unwrap_or(Path::new(".")).to_path_buf(),
            };
            match mdview_web::Server::start(&root, 0) {
                Ok(server) => self.server = Some(server),
                Err(error) => {
                    self.message = Some(format!("{error:#}"));
                    return;
                }
            }
        }
        let url = self.server.as_ref().unwrap().url_for(&path);
        self.message = Some(match open_external(&url) {
            Ok(()) => format!("browser preview at {url}"),
            Err(error) => format!("open {url} yourself ({error})"),
        });
    }

    fn switch_graphics(&mut self) {
        self.graphics = match self.graphics {
            Graphics::Kitty => Graphics::Text,
            Graphics::Text => Graphics::Kitty,
        };
        self.graphics.save();
        let _ = self.kitty.clear(&mut stdout());
        self.placements.clear();
        if let Some(doc) = &mut self.doc {
            doc.width = 0;
        }
        self.message = Some(match self.graphics {
            Graphics::Kitty => {
                "images: kitty protocol (blank boxes? your terminal lacks it, press i)".into()
            }
            Graphics::Text => "images: text mode".into(),
        });
    }

    fn key(&mut self, key: KeyEvent) {
        self.message = None;
        if self.mode == Mode::Edit {
            if key.code == KeyCode::F(1) {
                self.mode = Mode::Help;
                return;
            }
            // Every other key belongs to the editor, Ctrl-C included (it
            // copies).
            if let Some(editor) = &mut self.editor {
                let outcome = editor.key(key);
                self.apply(outcome);
            }
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        match self.mode {
            Mode::Edit => {}
            Mode::View if self.tree_open && self.tree_focus => self.tree_key(key),
            Mode::View => self.view_key(key),
            Mode::Picker => self.picker_key(key),
            Mode::Toc => self.toc_key(key),
            Mode::Help if self.editor.is_some() => self.mode = Mode::Edit,
            Mode::Help => self.mode = Mode::View,
            Mode::Search => match key.code {
                KeyCode::Esc => self.mode = Mode::View,
                KeyCode::Enter => {
                    self.search = std::mem::take(&mut self.query);
                    self.mode = Mode::View;
                    self.find_matches();
                    let visible = self.doc.as_ref().is_some_and(|d| {
                        self.matches
                            .iter()
                            .any(|m| m.0 >= d.scroll && m.0 < d.scroll + self.page())
                    });
                    if !visible {
                        self.next_match(true);
                    }
                }
                KeyCode::Backspace => {
                    self.query.pop();
                }
                KeyCode::Char(c) => self.query.push(c),
                _ => {}
            },
        }
    }

    fn view_key(&mut self, key: KeyEvent) {
        let page = self.page() as isize;
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Esc => {
                self.search.clear();
                self.matches.clear();
                self.link_focus = None;
                self.selection = None;
            }
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => self.copy_all(),
            KeyCode::Char('Y') => self.copy_all(),
            KeyCode::Char('j') | KeyCode::Down => self.scroll_by(1),
            KeyCode::Char('k') | KeyCode::Up => self.scroll_by(-1),
            KeyCode::Char('d') => self.scroll_by(page / 2),
            KeyCode::Char('u') => self.scroll_by(-page / 2),
            KeyCode::Char(' ') | KeyCode::PageDown => self.scroll_by(page - 1),
            KeyCode::PageUp => self.scroll_by(1 - page),
            KeyCode::Char('g') | KeyCode::Home => self.scroll_to(0),
            KeyCode::Char('G') | KeyCode::End => self.scroll_to(usize::MAX),
            KeyCode::Char('/') => {
                self.query.clear();
                self.mode = Mode::Search;
            }
            KeyCode::Char('n') => self.next_match(true),
            KeyCode::Char('N') => self.next_match(false),
            KeyCode::Tab => self.cycle_link(true),
            KeyCode::BackTab => self.cycle_link(false),
            KeyCode::Enter => {
                let link = self.link_focus.and_then(|(line, hit)| {
                    let doc = self.doc.as_ref()?;
                    Some(doc.rendered.lines.get(line)?.links.get(hit)?.link)
                });
                match link {
                    Some(link) => self.follow(link),
                    None => self.message = Some("Tab selects a link, Enter follows it".into()),
                }
            }
            KeyCode::Backspace | KeyCode::Char('-') => self.back(),
            KeyCode::Char('t') => {
                let Some(doc) = &self.doc else {
                    return;
                };
                if doc.rendered.toc.is_empty() {
                    self.message = Some("this document has no headings".into());
                    return;
                }
                self.selected = doc
                    .rendered
                    .toc
                    .iter()
                    .rposition(|entry| entry.2 <= doc.scroll)
                    .unwrap_or(0);
                self.mode = Mode::Toc;
            }
            KeyCode::Char('f') => self.enter_picker(),
            KeyCode::Char('b') => self.toggle_tree(),
            KeyCode::Char('e') => self.start_edit(),
            KeyCode::Char('o') => self.open_browser(),
            KeyCode::Char('i') => self.switch_graphics(),
            KeyCode::Char('c') => {
                self.theme = if self.theme.dark {
                    Theme::light()
                } else {
                    Theme::dark()
                };
                if let Some(doc) = &mut self.doc {
                    doc.width = 0;
                }
            }
            KeyCode::Char('y') => self.copy_visible_code(),
            KeyCode::Char('r') => {
                self.reload();
                self.message = Some("reloaded".into());
            }
            KeyCode::Char('?') => self.mode = Mode::Help,
            _ => {}
        }
    }

    fn picker_key(&mut self, key: KeyEvent) {
        let count = self.filtered_files().len();
        match key.code {
            KeyCode::Esc if self.doc.is_some() => self.mode = Mode::View,
            KeyCode::Esc => self.quit = true,
            KeyCode::Down => self.selected = (self.selected + 1).min(count.saturating_sub(1)),
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::PageDown => self.selected = (self.selected + 10).min(count.saturating_sub(1)),
            KeyCode::PageUp => self.selected = self.selected.saturating_sub(10),
            KeyCode::Enter => self.open_selected_file(),
            KeyCode::Backspace => {
                self.filter.pop();
                self.selected = 0;
            }
            KeyCode::Char(c) => {
                self.filter.push(c);
                self.selected = 0;
            }
            _ => {}
        }
    }

    fn open_selected_file(&mut self) {
        let Some(path) = self
            .filtered_files()
            .get(self.selected)
            .map(|relative| self.root.join(relative))
        else {
            return;
        };
        let previous = self.doc.as_ref().map(|d| (d.path.clone(), d.scroll));
        match self.open(&path, None) {
            Ok(()) => self.history.extend(previous),
            Err(error) => self.message = Some(format!("{error:#}")),
        }
    }

    fn toc_key(&mut self, key: KeyEvent) {
        let count = self.doc.as_ref().map_or(0, |d| d.rendered.toc.len());
        match key.code {
            KeyCode::Esc | KeyCode::Char('t') | KeyCode::Char('q') => self.mode = Mode::View,
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1).min(count.saturating_sub(1))
            }
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Enter => {
                let line = self
                    .doc
                    .as_ref()
                    .and_then(|d| d.rendered.toc.get(self.selected))
                    .map(|entry| entry.2);
                self.mode = Mode::View;
                if let Some(line) = line {
                    self.scroll_to(line);
                }
            }
            _ => {}
        }
    }

    fn mouse(&mut self, mouse: MouseEvent) {
        let step: isize = match mouse.kind {
            MouseEventKind::ScrollDown => 3,
            MouseEventKind::ScrollUp => -3,
            _ => 0,
        };
        if self.mode == Mode::Edit {
            if let Some(editor) = &mut self.editor {
                let outcome = editor.mouse(mouse);
                self.apply(outcome);
            }
            return;
        }
        let over_tree = self.tree_open
            && self.mode == Mode::View
            && mouse.column >= self.tree_area.x
            && mouse.column < self.tree_area.x + self.tree_area.width;
        if over_tree {
            let Some(tree) = &mut self.tree else {
                return;
            };
            if step != 0 {
                tree.move_by(step.signum());
            } else if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
                self.tree_focus = true;
                if let Some(index) = tree.index_at(mouse.row) {
                    tree.select_index(index);
                    if let Some(path) = tree.activate() {
                        self.open_from_tree(&path);
                    }
                }
            }
            return;
        }
        if self.mode != Mode::View {
            if step != 0 && matches!(self.mode, Mode::Picker | Mode::Toc) {
                let count = match self.mode {
                    Mode::Picker => self.filtered_files().len(),
                    _ => self.doc.as_ref().map_or(0, |d| d.rendered.toc.len()),
                };
                self.selected = self
                    .selected
                    .saturating_add_signed(step.signum())
                    .min(count.saturating_sub(1));
            }
            return;
        }
        match mouse.kind {
            _ if step != 0 => self.scroll_by(step),
            MouseEventKind::Down(MouseButton::Left) => {
                self.tree_focus = false;
                self.selection = None;
                self.press = self.point_at(mouse.column, mouse.row);
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                // Dragging past an edge scrolls, so a selection can be
                // longer than the screen.
                if mouse.row <= self.view.y {
                    self.scroll_by(-1);
                } else if mouse.row + 1 >= self.view.y + self.view.height {
                    self.scroll_by(1);
                }
                let here = self.point_at(mouse.column, mouse.row);
                if let (Some(press), Some(here)) = (self.press, here) {
                    if here != press || self.selection.is_some() {
                        self.selection = Some((press, here));
                    }
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let Some(press) = self.press.take() else {
                    return;
                };
                if self.selection.is_some() {
                    match self.selected_text() {
                        Some(text) => {
                            copy_to_clipboard(&text);
                            self.message =
                                Some(format!("copied {} characters", text.chars().count()));
                        }
                        None => self.selection = None,
                    }
                    return;
                }
                self.click(press);
            }
            _ => {}
        }
    }

    /// A press and release on the same cell: follow a link or copy a code
    /// block.
    fn click(&mut self, (line, column): Point) {
        let Some(line) = self.doc.as_ref().and_then(|d| d.rendered.lines.get(line)) else {
            return;
        };
        let link = line
            .links
            .iter()
            .find(|hit| column >= hit.start && column < hit.end)
            .map(|hit| hit.link);
        let code = line.code;
        if let Some(link) = link {
            self.message = None;
            self.follow(link);
        } else if let Some(code) = code {
            self.copy_code(code);
        }
    }

    fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        if area.height < 2 || area.width < 10 {
            return;
        }
        let content = Rect::new(area.x, area.y, area.width, area.height - 1);
        let status = Rect::new(area.x, content.y + content.height, area.width, 1);
        self.placements.clear();
        if self.mode == Mode::Picker {
            self.draw_picker(frame, content);
        } else {
            let content = self.draw_tree(frame, content);
            if self.editor.is_some() {
                self.draw_editor(frame, content);
            } else {
                self.draw_document(frame, content);
            }
        }
        match self.mode {
            Mode::Toc => self.draw_toc(frame, content),
            Mode::Help => self.draw_help(frame, content),
            _ => {}
        }
        // Terminal-drawn images sit above text, so they would cover a popup.
        if matches!(self.mode, Mode::Toc | Mode::Help | Mode::Picker) {
            self.placements.clear();
        }
        self.draw_status(frame.buffer_mut(), status);
    }

    /// Draws the file tree on the left when it is open and there is room,
    /// and returns what is left for the document.
    fn draw_tree(&mut self, frame: &mut Frame, area: Rect) -> Rect {
        self.tree_area = Rect::default();
        let Some(tree) = self.tree.as_mut().filter(|_| self.tree_open) else {
            return area;
        };
        if area.width < 60 {
            return area;
        }
        let width = (area.width / 4).clamp(24, 40);
        let side = Rect::new(area.x, area.y, width, area.height);
        let color = if self.tree_focus {
            self.theme.accent
        } else {
            self.theme.border
        };
        let block = Block::new()
            .borders(Borders::RIGHT)
            .border_style(Style::new().fg(color));
        let inner = block.inner(side);
        frame.render_widget(block, side);
        let current = self.doc.as_ref().map(|d| d.path.as_path());
        tree.render(
            inner,
            frame.buffer_mut(),
            &self.theme,
            self.tree_focus,
            current,
        );
        self.tree_area = inner;
        Rect::new(area.x + width, area.y, area.width - width, area.height)
    }

    /// The editor takes the whole document area: while editing there is
    /// nothing else on screen to compete with the text.
    fn draw_editor(&mut self, frame: &mut Frame, area: Rect) {
        if let Some(editor) = &mut self.editor {
            editor.render(area, frame.buffer_mut());
        }
    }

    /// The help window: every key, grouped, in one or two columns.
    fn draw_help(&mut self, frame: &mut Frame, area: Rect) {
        let theme = &self.theme;
        // From the editor, the window lists the editor's own keys.
        let editing = [("Editor", Editor::keys())];
        let shown: &[(&str, &[(&str, &str)])] = if self.editor.is_some() {
            &editing
        } else {
            HELP
        };
        let key_width = shown
            .iter()
            .flat_map(|(_, keys)| keys.iter())
            .map(|(key, _)| key.width())
            .max()
            .unwrap_or(0);
        let text_width = shown
            .iter()
            .flat_map(|(_, keys)| keys.iter())
            .map(|(_, what)| what.width())
            .max()
            .unwrap_or(0);
        let column_width = key_width + text_width + 3;
        let group = |(title, keys): &(&str, &[(&str, &str)])| {
            let mut lines = vec![Line::from(Span::styled(
                title.to_string(),
                Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
            ))];
            for (key, what) in keys.iter() {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{key:<key_width$}"),
                        Style::new().fg(theme.code).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(format!("   {what}")),
                ]));
            }
            lines.push(Line::default());
            lines
        };
        let groups: Vec<Vec<Line>> = shown.iter().map(group).collect();
        let two_columns = area.width as usize >= column_width * 2 + 12;
        let mut lines: Vec<Line> = Vec::new();
        if two_columns {
            let split = groups.len().div_ceil(2);
            let left: Vec<Line> = groups[..split].iter().flatten().cloned().collect();
            let right: Vec<Line> = groups[split..].iter().flatten().cloned().collect();
            for row in 0..left.len().max(right.len()) {
                let mut spans = left.get(row).map(|l| l.spans.clone()).unwrap_or_default();
                let used: usize = spans.iter().map(|s| s.content.width()).sum();
                spans.push(Span::raw(" ".repeat(column_width + 4 - used)));
                spans.extend(right.get(row).map(|l| l.spans.clone()).unwrap_or_default());
                lines.push(Line::from(spans));
            }
        } else {
            lines.extend(groups.into_iter().flatten());
        }
        while lines.last().is_some_and(|l| l.spans.is_empty()) {
            lines.pop();
        }
        let inner_width = if two_columns {
            column_width * 2 + 4
        } else {
            column_width
        };
        let rect = popup(area, inner_width as u16 + 6, lines.len() as u16 + 4);
        let block = Block::new()
            .borders(Borders::ALL)
            .border_style(Style::new().fg(theme.accent))
            .padding(ratatui::widgets::Padding::new(2, 2, 1, 1))
            .title(Span::styled(
                " Keys ",
                Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
            ))
            .title_bottom(Span::styled(" any key closes ", Style::new().fg(theme.dim)));
        frame.render_widget(Clear, rect);
        frame.render_widget(Paragraph::new(lines).block(block), rect);
    }

    fn draw_document(&mut self, frame: &mut Frame, area: Rect) {
        let width = area.width.saturating_sub(2).min(MAX_TEXT_WIDTH).max(8);
        let view = Rect::new(
            area.x + (area.width - width) / 2,
            area.y,
            width,
            area.height,
        );
        self.view = view;
        if self.doc.as_ref().is_some_and(|d| d.width != width) {
            self.relayout(width);
        }
        let Some(doc) = &mut self.doc else {
            return;
        };
        doc.scroll = doc.scroll.min(
            doc.rendered
                .lines
                .len()
                .saturating_sub(view.height as usize),
        );
        let buffer = frame.buffer_mut();
        let search_style = Style::new()
            .fg(self.theme.search_fg)
            .bg(self.theme.search_bg);
        let selected = self
            .selection
            .map(|(a, b)| if a <= b { (a, b) } else { (b, a) });
        for row in 0..view.height {
            let number = doc.scroll + row as usize;
            let Some(line) = doc.rendered.lines.get(number) else {
                break;
            };
            let y = view.y + row;
            if let Some(cell) = line.image {
                let slot = &doc.rendered.images[cell.slot];
                match self.placements.last_mut() {
                    Some(last) if last.key == slot.key && last.y + last.rows == y => last.rows += 1,
                    _ => self.placements.push(Placement {
                        key: slot.key,
                        image: slot.image.clone(),
                        x: view.x + cell.col,
                        y,
                        cols: slot.cols,
                        rows: 1,
                        first_row: cell.row,
                        total_rows: slot.rows,
                    }),
                }
                continue;
            }
            buffer.set_line(view.x, y, &Line::from(line.spans.clone()), view.width);
            for (_, start, end) in self.matches.iter().filter(|m| m.0 == number) {
                let start = (*start).min(view.width);
                let end = (*end).min(view.width);
                buffer.set_style(Rect::new(view.x + start, y, end - start, 1), search_style);
            }
            if let Some((start, end)) = selected.filter(|(s, e)| s.0 <= number && number <= e.0) {
                let from = if number == start.0 { start.1 } else { 0 };
                let to = if number == end.0 {
                    end.1.saturating_add(1)
                } else {
                    line.width().max(1)
                };
                let to = to.min(view.width);
                let from = from.min(to);
                buffer.set_style(
                    Rect::new(view.x + from, y, to - from, 1),
                    Style::new().add_modifier(Modifier::REVERSED),
                );
            }
            if let Some((_, hit)) = self.link_focus.filter(|focus| focus.0 == number) {
                if let Some(hit) = line.links.get(hit) {
                    let end = hit.end.min(view.width);
                    let start = hit.start.min(end);
                    buffer.set_style(
                        Rect::new(view.x + start, y, end - start, 1),
                        Style::new().add_modifier(Modifier::REVERSED),
                    );
                }
            }
        }
    }

    fn draw_picker(&mut self, frame: &mut Frame, area: Rect) {
        let theme = &self.theme;
        let files = self.filtered_files();
        let block = Block::new()
            .borders(Borders::ALL)
            .border_style(Style::new().fg(theme.border))
            .title(Span::styled(
                format!(" Markdown in {} ", self.root.display()),
                Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
            ));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.height < 3 {
            return;
        }
        let prompt = Line::from(vec![
            Span::styled(" filter ", Style::new().fg(theme.dim)),
            Span::raw(self.filter.clone()),
            Span::styled("▏", Style::new().fg(theme.accent)),
            Span::styled(
                format!("  {} of {}", files.len(), self.files.len()),
                Style::new().fg(theme.dim),
            ),
        ]);
        frame.render_widget(
            Paragraph::new(prompt),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
        let list_area = Rect::new(inner.x, inner.y + 2, inner.width, inner.height - 2);
        if files.is_empty() {
            let hint = if self.files.is_empty() {
                "No Markdown files here."
            } else {
                "Nothing matches the filter."
            };
            frame.render_widget(
                Paragraph::new(Span::styled(format!(" {hint}"), Style::new().fg(theme.dim))),
                list_area,
            );
            return;
        }
        let items: Vec<ListItem> = files
            .iter()
            .map(|path| ListItem::new(format!(" {}", path.display())))
            .collect();
        let list = List::new(items).highlight_style(
            Style::new()
                .fg(theme.bar_fg)
                .bg(theme.bar_bg)
                .add_modifier(Modifier::BOLD),
        );
        let mut state = ListState::default().with_selected(Some(self.selected));
        frame.render_stateful_widget(list, list_area, &mut state);
    }

    fn draw_toc(&mut self, frame: &mut Frame, area: Rect) {
        let Some(doc) = &self.doc else {
            return;
        };
        let theme = &self.theme;
        let items: Vec<ListItem> = doc
            .rendered
            .toc
            .iter()
            .map(|(level, title, _)| {
                let color = theme.headings[(*level as usize).clamp(1, 6) - 1];
                ListItem::new(Span::styled(
                    format!(" {}{title}", "  ".repeat(*level as usize - 1)),
                    Style::new().fg(color),
                ))
            })
            .collect();
        let widest = doc
            .rendered
            .toc
            .iter()
            .map(|(level, title, _)| title.width() + *level as usize * 2 + 2)
            .max()
            .unwrap_or(20) as u16;
        let rect = popup(area, widest.clamp(30, 80) + 2, items.len() as u16 + 2);
        let block = Block::new()
            .borders(Borders::ALL)
            .border_style(Style::new().fg(theme.accent))
            .title(" Contents ");
        let list = List::new(items)
            .block(block)
            .highlight_style(Style::new().bg(theme.bar_bg).add_modifier(Modifier::BOLD));
        let mut state = ListState::default().with_selected(Some(self.selected));
        frame.render_widget(Clear, rect);
        frame.render_stateful_widget(list, rect, &mut state);
    }

    fn draw_status(&self, buffer: &mut Buffer, area: Rect) {
        let theme = &self.theme;
        let bar = Style::new().fg(theme.bar_fg).bg(theme.bar_bg);
        buffer.set_style(area, bar);
        if self.mode == Mode::Search {
            let line = Line::from(vec![
                Span::styled(" /", bar.fg(theme.accent)),
                Span::styled(self.query.clone(), bar),
                Span::styled("▏", bar.fg(theme.accent)),
            ]);
            buffer.set_line(area.x, area.y, &line, area.width);
            return;
        }
        let name = match (&self.doc, self.mode) {
            (_, Mode::Picker) => "files".to_string(),
            // Relative to the root; a file outside it shows just its name.
            (Some(doc), _) => doc
                .path
                .strip_prefix(self.root.canonicalize().unwrap_or_default())
                .ok()
                .or_else(|| doc.path.file_name().map(Path::new))
                .unwrap_or(&doc.path)
                .display()
                .to_string(),
            (None, _) => String::new(),
        };
        let right = match (&self.doc, self.mode) {
            (_, Mode::Edit) => {
                let (line, column) = self.editor.as_ref().map_or((1, 1), Editor::cursor);
                let modified = match &self.editor {
                    Some(editor) if editor.is_dirty() => "modified · ",
                    _ => "",
                };
                format!(" {modified}Ln {line}, Col {column} · F1 keys ")
            }
            (_, Mode::View) if self.tree_open && self.tree_focus => {
                " ↑↓ move · ←→ fold · Enter open · b hide ".to_string()
            }
            (_, Mode::Picker) => " ↑↓ move · Enter open · Esc back ".to_string(),
            (_, Mode::Toc) => " ↑↓ move · Enter go · Esc close ".to_string(),
            (Some(doc), _) => {
                let total = doc.rendered.lines.len().max(1);
                let bottom = (doc.scroll + self.view.height as usize).min(total);
                format!(
                    " {}% · {} · ? help ",
                    bottom * 100 / total,
                    self.graphics.name()
                )
            }
            (None, _) => String::new(),
        };
        let middle = match &self.message {
            Some(message) => message.clone(),
            None if !self.search.is_empty() => {
                format!("/{} · {} matches · n N", self.search, self.matches.len())
            }
            None => String::new(),
        };
        let left = Line::from(vec![
            Span::styled(format!(" {name} "), bar.add_modifier(Modifier::BOLD)),
            Span::styled(format!(" {middle}"), bar.fg(theme.accent)),
        ]);
        let right_width = (right.width() as u16).min(area.width);
        buffer.set_line(
            area.x,
            area.y,
            &left,
            area.width.saturating_sub(right_width),
        );
        buffer.set_string(
            area.x + area.width - right_width,
            area.y,
            &right,
            bar.fg(theme.dim),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headings_are_located_in_the_source_outside_code_fences() {
        let source = "# One\n\n```sh\n# not a heading\n```\n\n## Two\ntext\n### Three\n";
        assert_eq!(heading_source_line(source, 0), Some(1));
        assert_eq!(heading_source_line(source, 1), Some(7));
        assert_eq!(heading_source_line(source, 2), Some(9));
        assert_eq!(heading_source_line(source, 3), None);
    }

    #[test]
    fn columns_slice_by_display_width() {
        assert_eq!(slice_columns("hello world", 6, 11), "world");
        assert_eq!(slice_columns("hello world", 0, 5), "hello");
        assert_eq!(slice_columns("hello", 2, u16::MAX), "llo");
        assert_eq!(slice_columns("hello", 9, 12), "");
        // Wide characters take two columns each.
        assert_eq!(slice_columns("日本語 ok", 2, 6), "本語");
        assert_eq!(slice_columns("│ a │ b │", 2, 3), "a");
    }
}

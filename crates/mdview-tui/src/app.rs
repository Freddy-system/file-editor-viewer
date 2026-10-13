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
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::graphics::{Graphics, KittyScreen, Placement};
use crate::layout::{
    layout, percent_decode, resolve_local, Assets, LayoutOptions, Rendered, DEFAULT_CELL,
};
use crate::theme::Theme;
use crate::Options;

/// Text is easier to read in a bounded column than across a very wide pane.
const MAX_TEXT_WIDTH: u16 = 120;
const MAX_LISTED_FILES: usize = 5000;

const HELP: &[(&str, &str)] = &[
    ("j k ↑ ↓ / wheel", "scroll"),
    ("d u / space / PgUp PgDn", "half page / page"),
    ("g G", "top / bottom"),
    ("/  n N", "search, next / previous match"),
    ("Tab  Shift-Tab", "next / previous link"),
    ("Enter / click", "follow link"),
    ("Backspace", "go back"),
    ("t", "table of contents"),
    ("f", "file picker"),
    ("o", "open in the browser (full-fidelity diagrams)"),
    ("i", "switch image mode (kitty / text)"),
    ("c", "switch light / dark colours"),
    ("r", "reload"),
    ("q", "quit"),
];

#[derive(PartialEq, Eq, Clone, Copy)]
enum Mode {
    View,
    Picker,
    Toc,
    Search,
    Help,
}

struct OpenDoc {
    path: PathBuf,
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
    pending_anchor: Option<String>,
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
        pending_anchor: None,
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
    let result = app.event_loop(&mut terminal);
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
        while !self.quit {
            if dirty {
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
        let source = std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        self.doc = Some(OpenDoc {
            path: path.clone(),
            doc: mdview_core::parse(&source),
            rendered: Rendered::default(),
            width: 0,
            scroll: 0,
        });
        self.pending_anchor = anchor;
        self.link_focus = None;
        self.search.clear();
        self.mode = Mode::View;
        self.watch(&path);
        Ok(())
    }

    /// Re-reads the current document after it changed on disk.
    fn reload(&mut self) {
        let Some(doc) = &mut self.doc else {
            return;
        };
        // Editors that replace the file leave a moment with nothing to read.
        let Ok(source) = std::fs::read_to_string(&doc.path) else {
            return;
        };
        doc.doc = mdview_core::parse(&source);
        doc.width = 0;
        self.link_focus = None;
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
        };
        doc.rendered = layout(&doc.doc, &options, &mut self.assets);
        doc.width = width;
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
        } else if is_markdown_path(&path) {
            let anchor = url.split_once('#').map(|(_, a)| a.to_string());
            let previous = (doc.path.clone(), doc.scroll);
            match self.open(&path, anchor) {
                Ok(()) => self.history.push(previous),
                Err(error) => self.message = Some(format!("{error:#}")),
            }
        } else {
            self.message = Some(match open_external(&path.to_string_lossy()) {
                Ok(()) => format!("opened {}", path.display()),
                Err(error) => format!("cannot open {}: {error}", path.display()),
            });
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
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        match self.mode {
            Mode::View => self.view_key(key),
            Mode::Picker => self.picker_key(key),
            Mode::Toc => self.toc_key(key),
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
            Mode::Help => self.mode = Mode::View,
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
            }
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
        let step = match mouse.kind {
            MouseEventKind::ScrollDown => 3,
            MouseEventKind::ScrollUp => -3,
            MouseEventKind::Down(MouseButton::Left) => 0,
            _ => return,
        };
        match self.mode {
            Mode::View if step != 0 => self.scroll_by(step),
            Mode::View => {
                let inside = mouse.row >= self.view.y
                    && mouse.row < self.view.y + self.view.height
                    && mouse.column >= self.view.x;
                let Some(doc) = self.doc.as_ref().filter(|_| inside) else {
                    return;
                };
                let line = doc.scroll + (mouse.row - self.view.y) as usize;
                let column = mouse.column - self.view.x;
                let link = doc.rendered.lines.get(line).and_then(|l| {
                    l.links
                        .iter()
                        .find(|hit| column >= hit.start && column < hit.end)
                        .map(|hit| hit.link)
                });
                if let Some(link) = link {
                    self.message = None;
                    self.follow(link);
                }
            }
            Mode::Picker | Mode::Toc if step != 0 => {
                let count = match self.mode {
                    Mode::Picker => self.filtered_files().len(),
                    _ => self.doc.as_ref().map_or(0, |d| d.rendered.toc.len()),
                };
                self.selected = self
                    .selected
                    .saturating_add_signed(step.signum())
                    .min(count.saturating_sub(1));
            }
            _ => {}
        }
    }

    fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        if area.height < 2 || area.width < 10 {
            return;
        }
        // Status line, plus a row of key hints when there is room for it.
        let bars = if area.height >= 8 { 2 } else { 1 };
        let content = Rect::new(area.x, area.y, area.width, area.height - bars);
        let status = Rect::new(area.x, content.y + content.height, area.width, 1);
        self.placements.clear();
        if self.mode == Mode::Picker {
            self.draw_picker(frame, content);
        } else {
            self.draw_document(frame, content);
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
        if bars == 2 {
            let hints = Rect::new(area.x, status.y + 1, area.width, 1);
            self.draw_hints(frame.buffer_mut(), hints);
        }
    }

    /// The keys that matter in the current mode, always on screen.
    fn draw_hints(&self, buffer: &mut Buffer, area: Rect) {
        let hints: &[(&str, &str)] = match self.mode {
            Mode::View | Mode::Help => &[
                ("?", "Help"),
                ("q", "Quit"),
                ("f", "Files"),
                ("t", "Contents"),
                ("/", "Search"),
                ("o", "Browser"),
                ("Tab", "Link"),
                ("Enter", "Follow"),
                ("Bksp", "Back"),
                ("i", "Images"),
                ("c", "Colours"),
                ("r", "Reload"),
            ],
            Mode::Picker => &[
                ("↑↓", "Move"),
                ("Enter", "Open"),
                ("Esc", "Back"),
                ("type", "Filter"),
            ],
            Mode::Toc => &[("↑↓", "Move"), ("Enter", "Go"), ("Esc", "Close")],
            Mode::Search => &[("Enter", "Search"), ("Esc", "Cancel")],
        };
        let key = Style::new()
            .fg(self.theme.bar_fg)
            .bg(self.theme.bar_bg)
            .add_modifier(Modifier::BOLD);
        let label = Style::new().fg(self.theme.dim);
        let mut spans = vec![Span::raw(" ")];
        let mut used = 1;
        for (name, what) in hints {
            let width = name.width() + what.width() + 5;
            if used + width > area.width as usize {
                break;
            }
            used += width;
            spans.push(Span::styled(format!(" {name} "), key));
            spans.push(Span::styled(format!(" {what}  "), label));
        }
        buffer.set_line(area.x, area.y, &Line::from(spans), area.width);
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

    fn draw_help(&mut self, frame: &mut Frame, area: Rect) {
        let theme = &self.theme;
        let keys = HELP.iter().map(|(k, _)| k.width()).max().unwrap_or(0);
        let lines: Vec<Line> = HELP
            .iter()
            .map(|(key, what)| {
                Line::from(vec![
                    Span::styled(format!(" {key:<keys$}  "), Style::new().fg(theme.accent)),
                    Span::raw(*what),
                ])
            })
            .collect();
        let widest = HELP.iter().map(|(_, w)| w.width()).max().unwrap_or(0);
        let rect = popup(area, (keys + widest + 6) as u16, lines.len() as u16 + 2);
        let block = Block::new()
            .borders(Borders::ALL)
            .border_style(Style::new().fg(theme.accent))
            .title(" Keys ");
        frame.render_widget(Clear, rect);
        frame.render_widget(Paragraph::new(lines).block(block), rect);
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
            (Some(doc), _) => doc
                .path
                .strip_prefix(self.root.canonicalize().unwrap_or_default())
                .unwrap_or(&doc.path)
                .display()
                .to_string(),
            (None, _) => String::new(),
        };
        let right = match (&self.doc, self.mode) {
            (_, Mode::Picker) => String::new(),
            (Some(doc), _) => {
                let total = doc.rendered.lines.len().max(1);
                let bottom = (doc.scroll + self.view.height as usize).min(total);
                format!(" {}% · {} ", bottom * 100 / total, self.graphics.name())
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

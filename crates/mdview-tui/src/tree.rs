//! Sidebar listing the documents under a root as a collapsible directory
//! tree.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::theme::Theme;

/// Directories deeper than this are not scanned.
const MAX_DEPTH: usize = 12;
/// Upper bound on listed files, so a huge checkout cannot stall the scan.
const MAX_FILES: usize = 5000;
/// Trees with at most this many rows start fully expanded.
const SMALL_TREE: usize = 40;
const INDENT: usize = 2;
const COLLAPSED: &str = "▸ ";
const EXPANDED: &str = "▾ ";
const NO_MARKER: &str = "  ";

#[cfg(test)]
/// One visible row of the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub path: PathBuf,
    pub name: String,
    /// 0 for entries directly under the root.
    pub depth: usize,
    pub is_dir: bool,
    /// Meaningful for directories only.
    pub expanded: bool,
}

#[derive(Debug)]
struct Node {
    name: String,
    /// Path relative to the root.
    relative: PathBuf,
    depth: usize,
    is_dir: bool,
    expanded: bool,
    parent: Option<usize>,
    children: Vec<usize>,
}

#[derive(Debug)]
pub struct Tree {
    root: PathBuf,
    /// Set when the tree was read from disk, so documents opened through
    /// their canonical path are still found.
    canonical_root: Option<PathBuf>,
    nodes: Vec<Node>,
    /// Entries directly under the root, in display order.
    top: Vec<usize>,
    /// Relative path to node.
    index: HashMap<PathBuf, usize>,
    /// Position within the visible rows.
    selected: usize,
    scroll: usize,
    /// Where the tree was last drawn, for mapping clicks back to rows.
    area: Rect,
}

impl Tree {
    /// Scans `root`, honouring ignore files and skipping hidden entries.
    /// Markdown files are always listed; `all_text` lists every other file
    /// too. Directories without listed files are left out.
    pub fn load(root: &Path, all_text: bool) -> Tree {
        let files: Vec<PathBuf> = ignore::WalkBuilder::new(root)
            .max_depth(Some(MAX_DEPTH))
            .build()
            .flatten()
            .filter(|entry| entry.file_type().is_some_and(|t| t.is_file()))
            .map(|entry| entry.into_path())
            .filter(|path| all_text || mdview_core::is_markdown_path(path))
            .take(MAX_FILES)
            .map(|path| path.strip_prefix(root).unwrap_or(&path).to_path_buf())
            .collect();
        let mut tree = Tree::from_paths(root, &files);
        tree.canonical_root = root.canonicalize().ok();
        tree
    }

    /// Builds a tree from file paths relative to `root`, without touching
    /// the disk. Directories come before files, each group ordered by name
    /// ignoring case.
    pub fn from_paths(root: &Path, files: &[PathBuf]) -> Tree {
        let mut nodes: Vec<Node> = Vec::new();
        let mut index: HashMap<PathBuf, usize> = HashMap::new();
        let mut top = Vec::new();
        for file in files {
            let relative = file.strip_prefix(root).unwrap_or(file);
            let names: Vec<_> = relative
                .components()
                .filter_map(|component| match component {
                    Component::Normal(name) => Some(name),
                    _ => None,
                })
                .collect();
            let mut parent: Option<usize> = None;
            let mut path = PathBuf::new();
            for (depth, name) in names.iter().enumerate() {
                path.push(name);
                let id = match index.get(&path).copied() {
                    Some(id) => id,
                    None => {
                        let id = nodes.len();
                        nodes.push(Node {
                            name: name.to_string_lossy().into_owned(),
                            relative: path.clone(),
                            depth,
                            is_dir: depth + 1 < names.len(),
                            expanded: false,
                            parent,
                            children: Vec::new(),
                        });
                        index.insert(path.clone(), id);
                        match parent {
                            Some(parent) => nodes[parent].children.push(id),
                            None => top.push(id),
                        }
                        id
                    }
                };
                parent = Some(id);
            }
        }
        for id in 0..nodes.len() {
            let mut children = std::mem::take(&mut nodes[id].children);
            children.sort_by_cached_key(|&child| sort_key(&nodes[child]));
            nodes[id].children = children;
        }
        top.sort_by_cached_key(|&child| sort_key(&nodes[child]));
        let mut tree = Tree {
            root: root.to_path_buf(),
            canonical_root: None,
            nodes,
            top,
            index,
            selected: 0,
            scroll: 0,
            area: Rect::default(),
        };
        let small = tree.nodes.len() <= SMALL_TREE;
        tree.set_all_expanded(small);
        tree
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    #[cfg(test)]
    /// Rows currently visible, in display order. Children of collapsed
    /// directories are left out.
    pub fn rows(&self) -> Vec<Row> {
        self.visible().into_iter().map(|id| self.row(id)).collect()
    }

    #[cfg(test)]
    pub fn selected(&self) -> Option<Row> {
        let visible = self.visible();
        let index = self.selected.min(visible.len().checked_sub(1)?);
        Some(self.row(visible[index]))
    }

    /// Index of the selected row within [`Tree::rows`].
    pub fn selected_index(&self) -> usize {
        self.selected.min(self.visible().len().saturating_sub(1))
    }

    pub fn select_index(&mut self, index: usize) {
        self.selected = index.min(self.visible().len().saturating_sub(1));
    }

    /// Moves the selection, stopping at the first and last rows.
    pub fn move_by(&mut self, delta: isize) {
        let last = self.visible().len().saturating_sub(1);
        self.selected = self.selected_index().saturating_add_signed(delta).min(last);
    }

    /// On a directory toggles it and returns `None`; on a file returns its
    /// path.
    pub fn activate(&mut self) -> Option<PathBuf> {
        let id = self.selected_node()?;
        if self.nodes[id].is_dir {
            self.nodes[id].expanded = !self.nodes[id].expanded;
            return None;
        }
        Some(self.root.join(&self.nodes[id].relative))
    }

    /// Expands a collapsed directory; on an expanded one moves to its first
    /// child.
    pub fn expand(&mut self) {
        let Some(id) = self.selected_node() else {
            return;
        };
        let node = &mut self.nodes[id];
        if !node.is_dir {
            return;
        }
        if !node.expanded {
            node.expanded = true;
        } else if !node.children.is_empty() {
            self.selected = self.selected_index() + 1;
        }
    }

    /// Collapses an expanded directory; anywhere else moves to the parent
    /// directory.
    pub fn collapse(&mut self) {
        let Some(id) = self.selected_node() else {
            return;
        };
        if self.nodes[id].is_dir && self.nodes[id].expanded {
            self.nodes[id].expanded = false;
            return;
        }
        let Some(parent) = self.nodes[id].parent else {
            return;
        };
        if let Some(index) = self.visible().iter().position(|&n| n == parent) {
            self.selected = index;
        }
    }

    /// Expands every ancestor of `path` and selects it. Returns `false`,
    /// changing nothing, when the path is not in the tree.
    pub fn reveal(&mut self, path: &Path) -> bool {
        let Some(id) = self.find(path) else {
            return false;
        };
        let mut ancestor = self.nodes[id].parent;
        while let Some(parent) = ancestor {
            self.nodes[parent].expanded = true;
            ancestor = self.nodes[parent].parent;
        }
        if let Some(index) = self.visible().iter().position(|&n| n == id) {
            self.selected = index;
        }
        true
    }

    /// Draws the tree into `area`, scrolling so the selection stays in
    /// view. `current` is the open document, drawn emphasised; `focused`
    /// makes the selection bar stand out.
    pub fn render(
        &mut self,
        area: Rect,
        buffer: &mut Buffer,
        theme: &Theme,
        focused: bool,
        current: Option<&Path>,
    ) {
        let area = area.intersection(buffer.area);
        self.area = area;
        if area.is_empty() {
            return;
        }
        let visible = self.visible();
        let height = area.height as usize;
        self.selected = self.selected.min(visible.len().saturating_sub(1));
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + height {
            self.scroll = self.selected + 1 - height;
        }
        self.scroll = self.scroll.min(visible.len().saturating_sub(height));
        let current = current.and_then(|path| self.find(path));
        let mut bar = Style::new().bg(theme.bar_bg);
        if focused {
            bar = bar.add_modifier(Modifier::BOLD);
        }
        let shown = visible.iter().enumerate().skip(self.scroll).take(height);
        for (line, (index, &id)) in shown.enumerate() {
            let node = &self.nodes[id];
            let marker = match (node.is_dir, node.expanded) {
                (true, true) => EXPANDED,
                (true, false) => COLLAPSED,
                (false, _) => NO_MARKER,
            };
            let suffix = if node.is_dir { "/" } else { "" };
            let text = format!(
                "{}{marker}{}{suffix}",
                " ".repeat(node.depth * INDENT),
                node.name
            );
            let style = if current == Some(id) {
                Style::new().fg(theme.accent).add_modifier(Modifier::BOLD)
            } else if node.is_dir {
                Style::new().fg(theme.headings[2])
            } else {
                Style::new()
            };
            let y = area.y + line as u16;
            let text = fit(&text, area.width as usize);
            buffer.set_line(
                area.x,
                y,
                &Line::from(Span::styled(text, style)),
                area.width,
            );
            if index == self.selected {
                buffer.set_style(Rect::new(area.x, y, area.width, 1), bar);
            }
        }
    }

    /// Row index drawn at screen row `y` by the last render, if any.
    pub fn index_at(&self, y: u16) -> Option<usize> {
        if y < self.area.y || y - self.area.y >= self.area.height {
            return None;
        }
        let index = self.scroll + (y - self.area.y) as usize;
        (index < self.visible().len()).then_some(index)
    }

    fn set_all_expanded(&mut self, expanded: bool) {
        for node in &mut self.nodes {
            node.expanded = expanded && node.is_dir;
        }
    }

    /// Node ids of the visible rows, in display order.
    fn visible(&self) -> Vec<usize> {
        let mut out = Vec::new();
        let mut pending: Vec<usize> = self.top.iter().rev().copied().collect();
        while let Some(id) = pending.pop() {
            out.push(id);
            let node = &self.nodes[id];
            if node.is_dir && node.expanded {
                pending.extend(node.children.iter().rev());
            }
        }
        out
    }

    fn selected_node(&self) -> Option<usize> {
        let visible = self.visible();
        let index = self.selected.min(visible.len().checked_sub(1)?);
        Some(visible[index])
    }

    #[cfg(test)]
    fn row(&self, id: usize) -> Row {
        let node = &self.nodes[id];
        Row {
            path: self.root.join(&node.relative),
            name: node.name.clone(),
            depth: node.depth,
            is_dir: node.is_dir,
            expanded: node.expanded,
        }
    }

    /// Looks a path up as given, then relative to the root in either of its
    /// spellings.
    fn find(&self, path: &Path) -> Option<usize> {
        if let Some(&id) = self.index.get(path) {
            return Some(id);
        }
        [Some(&self.root), self.canonical_root.as_ref()]
            .into_iter()
            .flatten()
            .find_map(|root| {
                let relative = path.strip_prefix(root).ok()?;
                self.index.get(relative).copied()
            })
    }
}

/// Directories first, then by name ignoring case; the exact name breaks
/// ties so the order is stable across scans.
fn sort_key(node: &Node) -> (bool, String, String) {
    (!node.is_dir, node.name.to_lowercase(), node.name.clone())
}

/// Shortens `text` to `width` columns, ending in an ellipsis when cut.
fn fit(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    let mut out = String::new();
    if width == 0 {
        return out;
    }
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > width - 1 {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "/docs";

    fn tree(files: &[&str]) -> Tree {
        let files: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
        Tree::from_paths(Path::new(ROOT), &files)
    }

    /// Nine rows when fully expanded.
    fn small() -> Tree {
        tree(&[
            "c.md",
            "B.md",
            "a.md",
            "docs/guide.md",
            "docs/Zeta/deep.md",
            "api/x.md",
        ])
    }

    fn collapsed() -> Tree {
        let mut tree = small();
        tree.set_all_expanded(false);
        tree
    }

    fn names(tree: &Tree) -> Vec<String> {
        tree.rows().into_iter().map(|row| row.name).collect()
    }

    fn screen(buffer: &Buffer, area: Rect) -> Vec<String> {
        (area.y..area.y + area.height)
            .map(|y| {
                (area.x..area.x + area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn directories_come_first_then_names_ignoring_case() {
        let tree = small();
        assert_eq!(
            names(&tree),
            ["api", "x.md", "docs", "Zeta", "deep.md", "guide.md", "a.md", "B.md", "c.md"]
        );
        let rows = tree.rows();
        assert_eq!(rows[0].path, PathBuf::from("/docs/api"));
        assert!(rows[0].is_dir && rows[0].depth == 0);
        assert_eq!(rows[4].path, PathBuf::from("/docs/docs/Zeta/deep.md"));
        assert!(!rows[4].is_dir && rows[4].depth == 2);
    }

    #[test]
    fn collapsed_directories_hide_their_children() {
        let tree = collapsed();
        assert_eq!(names(&tree), ["api", "docs", "a.md", "B.md", "c.md"]);
        assert!(tree.rows().iter().all(|row| !row.expanded));
    }

    #[test]
    fn activate_toggles_a_directory_and_returns_a_file_path() {
        let mut tree = collapsed();
        assert_eq!(tree.activate(), None);
        assert_eq!(
            names(&tree),
            ["api", "x.md", "docs", "a.md", "B.md", "c.md"]
        );
        assert!(tree.selected().unwrap().expanded);
        assert_eq!(tree.activate(), None);
        assert_eq!(names(&tree).len(), 5);
        tree.select_index(2);
        assert_eq!(tree.activate(), Some(PathBuf::from("/docs/a.md")));
        assert_eq!(names(&tree).len(), 5);
    }

    #[test]
    fn expand_opens_a_directory_then_moves_to_its_first_child() {
        let mut tree = collapsed();
        tree.select_index(1);
        tree.expand();
        assert_eq!(tree.selected().unwrap().name, "docs");
        assert!(tree.selected().unwrap().expanded);
        tree.expand();
        assert_eq!(tree.selected().unwrap().name, "Zeta");
        tree.select_index(tree.rows().len());
        tree.expand();
        assert_eq!(tree.selected().unwrap().name, "c.md");
    }

    #[test]
    fn collapse_closes_a_directory_or_moves_to_the_parent() {
        let mut tree = small();
        tree.select_index(4);
        assert_eq!(tree.selected().unwrap().name, "deep.md");
        tree.collapse();
        assert_eq!(tree.selected().unwrap().name, "Zeta");
        tree.collapse();
        assert_eq!(tree.selected().unwrap().name, "Zeta");
        assert!(!tree.selected().unwrap().expanded);
        tree.collapse();
        assert_eq!(tree.selected().unwrap().name, "docs");
        tree.collapse();
        assert!(!tree.selected().unwrap().expanded);
        tree.collapse();
        assert_eq!(tree.selected().unwrap().name, "docs");
    }

    #[test]
    fn reveal_expands_ancestors_and_selects_the_path() {
        let mut tree = collapsed();
        assert!(tree.reveal(Path::new("/docs/docs/Zeta/deep.md")));
        assert_eq!(tree.selected().unwrap().name, "deep.md");
        assert_eq!(tree.selected_index(), 3);
        assert_eq!(
            names(&tree),
            ["api", "docs", "Zeta", "deep.md", "guide.md", "a.md", "B.md", "c.md"]
        );
        let before = names(&tree);
        assert!(!tree.reveal(Path::new("/docs/missing.md")));
        assert!(!tree.reveal(Path::new("/elsewhere/a.md")));
        assert_eq!(names(&tree), before);
        assert_eq!(tree.selected_index(), 3);
        assert!(tree.reveal(Path::new("api/x.md")));
        assert_eq!(tree.selected().unwrap().name, "x.md");
    }

    #[test]
    fn move_by_clamps_at_both_ends() {
        let mut tree = collapsed();
        tree.move_by(-3);
        assert_eq!(tree.selected_index(), 0);
        tree.move_by(2);
        assert_eq!(tree.selected_index(), 2);
        tree.move_by(100);
        assert_eq!(tree.selected_index(), 4);
        tree.move_by(isize::MIN);
        assert_eq!(tree.selected_index(), 0);
        tree.select_index(99);
        assert_eq!(tree.selected_index(), 4);
    }

    #[test]
    fn small_trees_start_expanded_and_large_ones_collapsed() {
        let tree = small();
        assert_eq!(tree.rows().len(), 9);
        assert!(tree.rows().iter().all(|row| !row.is_dir || row.expanded));

        let files: Vec<PathBuf> = (0..30)
            .map(|i| PathBuf::from(format!("d{i:02}/f.md")))
            .collect();
        let large = Tree::from_paths(Path::new(ROOT), &files);
        assert_eq!(large.rows().len(), 30);
        assert!(large.rows().iter().all(|row| row.is_dir && !row.expanded));
    }

    #[test]
    fn an_empty_tree_has_no_rows_and_ignores_input() {
        let mut tree = tree(&[]);
        assert!(tree.is_empty());
        assert_eq!(tree.selected(), None);
        tree.move_by(1);
        tree.expand();
        tree.collapse();
        assert_eq!(tree.activate(), None);
        assert_eq!(tree.selected_index(), 0);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 10, 3));
        tree.render(
            Rect::new(0, 0, 10, 3),
            &mut buffer,
            &Theme::dark(),
            true,
            None,
        );
        assert_eq!(tree.index_at(0), None);
    }

    #[test]
    fn render_draws_one_indented_row_per_line() {
        let mut tree = small();
        tree.select_index(2);
        tree.activate();
        let theme = Theme::dark();
        let area = Rect::new(0, 0, 20, 8);
        let mut buffer = Buffer::empty(area);
        tree.render(
            area,
            &mut buffer,
            &theme,
            true,
            Some(Path::new("/docs/a.md")),
        );
        assert_eq!(
            screen(&buffer, area),
            [
                "▾ api/",
                "    x.md",
                "▸ docs/",
                "  a.md",
                "  B.md",
                "  c.md",
                "",
                ""
            ]
        );
        assert_eq!(buffer[(0, 2)].style().bg, Some(theme.bar_bg));
        assert_eq!(
            buffer[(0, 1)].style().bg,
            Buffer::empty(area)[(0, 0)].style().bg
        );
        assert_eq!(buffer[(2, 3)].style().fg, Some(theme.accent));
        assert_eq!(buffer[(0, 0)].style().fg, Some(theme.headings[2]));
        assert_eq!(tree.index_at(3), Some(3));
        assert_eq!(tree.index_at(6), None);
    }

    #[test]
    fn render_truncates_long_names_within_the_area() {
        let mut tree = tree(&["a-very-long-document-name.md", "guides/b.md"]);
        let area = Rect::new(2, 1, 12, 3);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 24, 5));
        tree.render(area, &mut buffer, &Theme::dark(), false, None);
        let lines = screen(&buffer, area);
        assert_eq!(lines, ["▾ guides/", "    b.md", "  a-very-lo…"]);
        assert!(lines.iter().all(|line| line.width() <= 12));
        for y in 0..5 {
            for x in 0..24 {
                let inside = (2..14).contains(&x) && (1..4).contains(&y);
                assert!(inside || buffer[(x, y)].symbol() == " ", "wrote at {x},{y}");
            }
        }
        assert_eq!(tree.index_at(0), None);
        assert_eq!(tree.index_at(1), Some(0));
    }

    #[test]
    fn render_scrolls_to_keep_the_selection_visible() {
        let files: Vec<PathBuf> = (0..50)
            .map(|i| PathBuf::from(format!("f{i:02}.md")))
            .collect();
        let mut tree = Tree::from_paths(Path::new(ROOT), &files);
        let theme = Theme::dark();
        let area = Rect::new(0, 0, 16, 5);
        let mut buffer = Buffer::empty(area);
        tree.select_index(30);
        tree.render(area, &mut buffer, &theme, true, None);
        assert_eq!(screen(&buffer, area)[4], "  f30.md");
        assert_eq!(tree.index_at(4), Some(30));

        let mut buffer = Buffer::empty(area);
        tree.select_index(2);
        tree.render(area, &mut buffer, &theme, true, None);
        assert_eq!(screen(&buffer, area)[0], "  f02.md");
        assert_eq!(tree.index_at(0), Some(2));

        let mut buffer = Buffer::empty(area);
        tree.move_by(1);
        tree.render(area, &mut buffer, &theme, true, None);
        assert_eq!(screen(&buffer, area)[0], "  f02.md");
        assert_eq!(tree.index_at(1), Some(3));
    }

    #[test]
    fn render_ignores_areas_without_room() {
        let mut tree = small();
        let mut buffer = Buffer::empty(Rect::new(0, 0, 10, 4));
        tree.render(
            Rect::new(0, 0, 0, 4),
            &mut buffer,
            &Theme::dark(),
            true,
            None,
        );
        tree.render(
            Rect::new(3, 3, 10, 0),
            &mut buffer,
            &Theme::dark(),
            true,
            None,
        );
        tree.render(
            Rect::new(40, 40, 5, 5),
            &mut buffer,
            &Theme::dark(),
            true,
            None,
        );
        assert_eq!(tree.index_at(0), None);
        tree.render(
            Rect::new(0, 0, 1, 1),
            &mut buffer,
            &Theme::dark(),
            true,
            None,
        );
        assert_eq!(buffer[(0, 0)].symbol(), "…");
    }
}

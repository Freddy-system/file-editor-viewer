# mdview

English · [Español](README.es.md)

A Markdown previewer for the terminal that draws **Mermaid diagrams, Graphviz
graphs and LaTeX math**. It runs as a [herdr](https://herdr.dev) plugin, as a
standalone terminal viewer, and as a live browser preview. Written in Rust.

![mdview previewing a document with diagrams in a terminal](docs/demo.gif)

The recording is a plain terminal with no image support: what every terminal
gets.

- **Full GitHub-flavoured Markdown**: tables, task lists, alerts, footnotes,
  definition lists, front matter, sub/superscripts, wikilinks, syntax
  highlighting.
- **Diagrams and math in the terminal**: real images where the terminal can
  show them, box-drawing text where it cannot.
- **Live reload**: the preview follows the file as you save it.
- **Made for reading docs**: table of contents, search, links between
  documents, a file tree, click to copy a code block.
- **Read-only by default, with an editor built in**: press `e` to edit the
  file with a live preview beside it. Nothing else to install.
- **Source files too**: links to code open as a highlighted, numbered listing.

## Install as a herdr plugin

Needs herdr 0.9 or newer.

```bash
herdr plugin install Freddy-system/herdr-mdview
```

The install downloads a prebuilt binary for Linux and macOS (x86_64 and
arm64). On other platforms, or with `MDVIEW_BUILD_FROM_SOURCE=1`, it builds
with `cargo`.

Bind it to a key in `~/.config/herdr/config.toml`, then run
`herdr server reload-config`:

```toml
[[keys.command]]
key = "prefix+m"
type = "plugin_action"
command = "mdview.open"
description = "preview Markdown"
```

### Using it in herdr

| You do | What happens |
|---|---|
| Press the key | A pane opens on the right with the `README.md` of the focused pane's directory, or a file picker if there is none |
| Select a `.md` path, then press the key | That file opens |
| Ctrl-click a link to a local `.md` file | That file opens |
| Run `mdview open notes.md` in a pane | That file opens in a pane beside it |
| `herdr plugin action invoke mdview.open-tab` | Same, in a new tab |
| `herdr plugin action invoke mdview.open-overlay` | Same, in an overlay |

`mdview open` needs the `mdview` command on your `PATH`; see
[Standalone](#standalone).

## Keys

Press `?` in the viewer for this list.

| Key | Action |
|---|---|
| `j` `k` `↑` `↓`, wheel | Scroll |
| `Space`, `d` `u`, `PgUp` `PgDn` | Page, half page |
| `g` `G` | Top, bottom |
| `/`, `n` `N` | Search, next and previous match |
| `Tab`, `Shift-Tab`, `Enter`, click | Select and follow links, across documents and to headings |
| `Backspace` | Previous document |
| `t` | Table of contents |
| `b` | File tree sidebar |
| `f` | File picker |
| `e` | Edit the file (see [Editing](#editing)) |
| Drag | Select text; it is copied when you release |
| `y`, click on a code block | Copy the code block |
| `Y`, `Ctrl-A` | Copy the whole document as Markdown |
| `o` | Open in the browser |
| `i` | Switch image mode (`kitty` / `text`) |
| `c` | Light or dark colours |
| `r`, `q` | Reload, quit |

## Editing

The viewer never changes a file until you ask. `e` opens the built-in editor
on the section you are reading; on a wide pane the preview stays beside it and
follows what you type. It works like a plain text box: no modes, no setup, no
external editor.

| Key | Action |
|---|---|
| `Ctrl-S` | Save |
| `Esc`, `Ctrl-Q` | Back to reading; asks again if there are unsaved changes |
| `Ctrl-Z`, `Ctrl-Y` | Undo, redo |
| `Shift`+arrows, `Ctrl-A` | Select, select all |
| `Ctrl-C`, `Ctrl-X`, `Ctrl-V` | Copy, cut, paste |
| `Ctrl-K` | Delete the line |
| Click, wheel | Place the cursor, scroll |
| `F1` | All editor keys |

## How diagrams are drawn

herdr gives plugins terminal panes, so what graphics can look like depends on
the terminal herdr runs in. mdview has three levels:

| Level | Where | Result |
|---|---|---|
| `kitty` | Kitty, Ghostty, WezTerm, Konsole | Diagrams, math and images as sharp pictures, through the Kitty graphics protocol |
| `text` | Any terminal, e.g. Alacritty | Mermaid and Graphviz as box-drawing characters, math as Unicode, images as coloured half blocks |
| Browser (`o`) | Anywhere | The original Mermaid, KaTeX and Graphviz renderers, with live reload |

mdview picks `kitty` or `text` from the terminal it detects. `i` switches and
remembers your choice.

Known limits:

- In `text` mode Graphviz graphs keep their nodes, edges and labels but not
  their colours, shapes or exact layout.
- Complex diagrams can come out crowded in `text` mode; `o` shows them in the
  browser.
- The browser preview loads Mermaid, KaTeX, highlight.js and Graphviz from a
  CDN, so it needs a connection the first time.
- Windows is not supported yet.

If `plantuml` or `d2` are installed, their code blocks are drawn too.

### Environment

| Variable | Effect |
|---|---|
| `MDVIEW_GRAPHICS=kitty\|text` | Force the image mode |
| `MDVIEW_THEME=light\|dark` | Starting colours |
| `MDVIEW_CELL=10x20` | Cell size in pixels assumed when sizing images |
| `MDVIEW_MOUSE=0` | Leave the mouse to the terminal |

## Standalone

mdview does not need herdr.

```bash
cargo install --git https://github.com/Freddy-system/herdr-mdview mdview
```

Prebuilt binaries are attached to each
[release](https://github.com/Freddy-system/herdr-mdview/releases).

```bash
mdview README.md            # terminal viewer
mdview docs/                # file picker for a directory
mdview serve docs/          # browser preview with live reload
mdview export README.md     # writes README.html
```

The preview server listens on `127.0.0.1` only and serves nothing outside the
directory you give it.

## Architecture

Each part is its own crate. The frontends do not know about each other; they
all start from the same document model.

| Crate | Role |
|---|---|
| `mdview-core` | Parses Markdown into a tree of blocks. New syntax is added here once, for every frontend |
| `mdview-render` | Diagram engines behind the `DiagramEngine` trait. A new language is a new engine in the `Registry` |
| `mdview-tui` | Layout to terminal lines, Kitty or text graphics, and the interactive viewer |
| `mdview-web` | Self-contained HTML page and the local live-reload server |
| `mdview` | Command line and the herdr glue (`herdr-plugin.toml`) |

## Roadmap

- **VS Code extension**: a webview showing the `mdview-web` page, with scroll
  sync through the heading ids the core already produces.
- **Desktop app** for Linux, Windows and macOS: a Tauri shell around the same
  page.
- Bundle the web assets so the browser preview works offline.
- Windows support for the herdr plugin.

## Development

```bash
cargo test --workspace
cargo fmt --all
cargo run -- examples/demo.md
```

`examples/demo.md` walks through everything the viewer draws. Pushing a `v*`
tag builds and publishes the release binaries. `docs/demo.gif` is recorded
from `docs/demo.tape` with [VHS](https://github.com/charmbracelet/vhs).

## License

MIT © 2026 Freddy Sedano. See [LICENSE](LICENSE). Copies and derived work must
keep the copyright notice and the license text.

use mdview_core::Alert;
use ratatui::style::Color;

#[derive(Debug, Clone)]
pub struct Theme {
    pub dark: bool,
    /// Used to flatten transparent images when drawing them as text.
    pub background: [u8; 3],
    pub text: [u8; 3],
    pub dim: Color,
    pub border: Color,
    pub headings: [Color; 6],
    pub link: Color,
    pub code: Color,
    pub code_bg: Color,
    pub quote: Color,
    pub math: Color,
    pub diagram: Color,
    pub marker: Color,
    pub error: Color,
    pub search_fg: Color,
    pub search_bg: Color,
    pub bar_fg: Color,
    pub bar_bg: Color,
    pub accent: Color,
    pub syntax_theme: &'static str,
    alerts: [Color; 5],
}

const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

impl Theme {
    pub fn dark() -> Self {
        Theme {
            dark: true,
            background: [0x1e, 0x1e, 0x2e],
            text: [0xcd, 0xd6, 0xf4],
            dim: rgb(0x7f849c),
            border: rgb(0x585b70),
            headings: [
                rgb(0xf5c2e7),
                rgb(0xcba6f7),
                rgb(0x89b4fa),
                rgb(0x94e2d5),
                rgb(0xa6e3a1),
                rgb(0xf9e2af),
            ],
            link: rgb(0x89b4fa),
            code: rgb(0xfab387),
            code_bg: rgb(0x2a2b3c),
            quote: rgb(0xa6adc8),
            math: rgb(0xf9e2af),
            diagram: rgb(0x94e2d5),
            marker: rgb(0xf38ba8),
            error: rgb(0xf38ba8),
            search_fg: rgb(0x1e1e2e),
            search_bg: rgb(0xf9e2af),
            bar_fg: rgb(0xcdd6f4),
            bar_bg: rgb(0x313244),
            accent: rgb(0xcba6f7),
            syntax_theme: "base16-ocean.dark",
            alerts: [
                rgb(0x89b4fa),
                rgb(0xa6e3a1),
                rgb(0xcba6f7),
                rgb(0xf9e2af),
                rgb(0xf38ba8),
            ],
        }
    }

    pub fn light() -> Self {
        Theme {
            dark: false,
            background: [0xef, 0xf1, 0xf5],
            text: [0x4c, 0x4f, 0x69],
            dim: rgb(0x8c8fa1),
            border: rgb(0xacb0be),
            headings: [
                rgb(0xea76cb),
                rgb(0x8839ef),
                rgb(0x1e66f5),
                rgb(0x179299),
                rgb(0x40a02b),
                rgb(0xdf8e1d),
            ],
            link: rgb(0x1e66f5),
            code: rgb(0xfe640b),
            code_bg: rgb(0xe6e9ef),
            quote: rgb(0x6c6f85),
            math: rgb(0xdf8e1d),
            diagram: rgb(0x179299),
            marker: rgb(0xd20f39),
            error: rgb(0xd20f39),
            search_fg: rgb(0xeff1f5),
            search_bg: rgb(0xdf8e1d),
            bar_fg: rgb(0x4c4f69),
            bar_bg: rgb(0xccd0da),
            accent: rgb(0x8839ef),
            syntax_theme: "InspiredGitHub",
            alerts: [
                rgb(0x1e66f5),
                rgb(0x40a02b),
                rgb(0x8839ef),
                rgb(0xdf8e1d),
                rgb(0xd20f39),
            ],
        }
    }

    pub fn alert(&self, alert: Alert) -> (Color, &'static str) {
        match alert {
            Alert::Note => (self.alerts[0], "ℹ"),
            Alert::Tip => (self.alerts[1], "✦"),
            Alert::Important => (self.alerts[2], "❖"),
            Alert::Warning => (self.alerts[3], "⚠"),
            Alert::Caution => (self.alerts[4], "✖"),
        }
    }
}

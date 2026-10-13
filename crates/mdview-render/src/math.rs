use anyhow::anyhow;
use image::RgbaImage;
use ratex_types::color::Color;
use ratex_types::math_style::MathStyle;

use crate::Palette;

/// Typesets LaTeX math to a transparent image, `em_px` pixels per em.
pub fn math_to_image(
    tex: &str,
    display: bool,
    palette: Palette,
    em_px: f32,
) -> anyhow::Result<RgbaImage> {
    let tex = tex.to_string();
    let [r, g, b] = palette.foreground;
    let png = std::panic::catch_unwind(move || -> Result<Vec<u8>, String> {
        let ast = ratex_parser::parse(&tex).map_err(|e| e.to_string())?;
        let style = if display {
            MathStyle::Display
        } else {
            MathStyle::Text
        };
        let color = Color::rgb(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
        let options = ratex_layout::LayoutOptions::default()
            .with_style(style)
            .with_color(color);
        let list = ratex_layout::to_display_list(&ratex_layout::layout(&ast, &options));
        ratex_render::render_to_png(
            &list,
            &ratex_render::RenderOptions {
                font_size: em_px,
                padding: em_px / 5.0,
                background_color: Color::new(0.0, 0.0, 0.0, 0.0),
                ..ratex_render::RenderOptions::default()
            },
        )
    })
    .map_err(|_| anyhow!("the math renderer failed on this formula"))?
    .map_err(|e| anyhow!("invalid LaTeX: {e}"))?;
    Ok(image::load_from_memory(&png)?.to_rgba8())
}

const SYMBOLS: &[(&str, &str)] = &[
    ("alpha", "α"),
    ("beta", "β"),
    ("gamma", "γ"),
    ("delta", "δ"),
    ("epsilon", "ε"),
    ("varepsilon", "ε"),
    ("zeta", "ζ"),
    ("eta", "η"),
    ("theta", "θ"),
    ("vartheta", "ϑ"),
    ("iota", "ι"),
    ("kappa", "κ"),
    ("lambda", "λ"),
    ("mu", "μ"),
    ("nu", "ν"),
    ("xi", "ξ"),
    ("pi", "π"),
    ("rho", "ρ"),
    ("sigma", "σ"),
    ("tau", "τ"),
    ("upsilon", "υ"),
    ("phi", "φ"),
    ("varphi", "φ"),
    ("chi", "χ"),
    ("psi", "ψ"),
    ("omega", "ω"),
    ("Gamma", "Γ"),
    ("Delta", "Δ"),
    ("Theta", "Θ"),
    ("Lambda", "Λ"),
    ("Xi", "Ξ"),
    ("Pi", "Π"),
    ("Sigma", "Σ"),
    ("Phi", "Φ"),
    ("Psi", "Ψ"),
    ("Omega", "Ω"),
    ("sum", "∑"),
    ("prod", "∏"),
    ("int", "∫"),
    ("iint", "∬"),
    ("oint", "∮"),
    ("partial", "∂"),
    ("nabla", "∇"),
    ("infty", "∞"),
    ("pm", "±"),
    ("mp", "∓"),
    ("times", "×"),
    ("div", "÷"),
    ("cdot", "·"),
    ("cdots", "⋯"),
    ("ldots", "…"),
    ("dots", "…"),
    ("vdots", "⋮"),
    ("ddots", "⋱"),
    ("leq", "≤"),
    ("le", "≤"),
    ("geq", "≥"),
    ("ge", "≥"),
    ("neq", "≠"),
    ("ne", "≠"),
    ("approx", "≈"),
    ("equiv", "≡"),
    ("sim", "∼"),
    ("simeq", "≃"),
    ("propto", "∝"),
    ("ll", "≪"),
    ("gg", "≫"),
    ("in", "∈"),
    ("notin", "∉"),
    ("ni", "∋"),
    ("subset", "⊂"),
    ("supset", "⊃"),
    ("subseteq", "⊆"),
    ("supseteq", "⊇"),
    ("cup", "∪"),
    ("cap", "∩"),
    ("emptyset", "∅"),
    ("varnothing", "∅"),
    ("forall", "∀"),
    ("exists", "∃"),
    ("neg", "¬"),
    ("land", "∧"),
    ("wedge", "∧"),
    ("lor", "∨"),
    ("vee", "∨"),
    ("oplus", "⊕"),
    ("otimes", "⊗"),
    ("to", "→"),
    ("rightarrow", "→"),
    ("leftarrow", "←"),
    ("gets", "←"),
    ("leftrightarrow", "↔"),
    ("Rightarrow", "⇒"),
    ("Leftarrow", "⇐"),
    ("Leftrightarrow", "⇔"),
    ("implies", "⟹"),
    ("iff", "⟺"),
    ("mapsto", "↦"),
    ("uparrow", "↑"),
    ("downarrow", "↓"),
    ("langle", "⟨"),
    ("rangle", "⟩"),
    ("lfloor", "⌊"),
    ("rfloor", "⌋"),
    ("lceil", "⌈"),
    ("rceil", "⌉"),
    ("angle", "∠"),
    ("perp", "⊥"),
    ("parallel", "∥"),
    ("circ", "∘"),
    ("bullet", "•"),
    ("star", "⋆"),
    ("ast", "∗"),
    ("dagger", "†"),
    ("ell", "ℓ"),
    ("hbar", "ℏ"),
    ("Re", "ℜ"),
    ("Im", "ℑ"),
    ("aleph", "ℵ"),
    ("degree", "°"),
    ("prime", "′"),
    ("therefore", "∴"),
    ("because", "∵"),
    ("quad", "  "),
    ("qquad", "    "),
    ("lbrace", "{"),
    ("rbrace", "}"),
    ("mid", "|"),
    ("vert", "|"),
    ("Vert", "‖"),
    ("setminus", "∖"),
];

const BLACKBOARD: &[(char, char)] = &[
    ('C', 'ℂ'),
    ('H', 'ℍ'),
    ('N', 'ℕ'),
    ('P', 'ℙ'),
    ('Q', 'ℚ'),
    ('R', 'ℝ'),
    ('Z', 'ℤ'),
];

fn superscript(c: char) -> Option<char> {
    Some(match c {
        '0' => '⁰',
        '1' => '¹',
        '2' => '²',
        '3' => '³',
        '4' => '⁴',
        '5' => '⁵',
        '6' => '⁶',
        '7' => '⁷',
        '8' => '⁸',
        '9' => '⁹',
        '+' => '⁺',
        '-' | '−' => '⁻',
        '=' => '⁼',
        '(' => '⁽',
        ')' => '⁾',
        'n' => 'ⁿ',
        'i' => 'ⁱ',
        'a' => 'ᵃ',
        'b' => 'ᵇ',
        'c' => 'ᶜ',
        'd' => 'ᵈ',
        'e' => 'ᵉ',
        'k' => 'ᵏ',
        'm' => 'ᵐ',
        't' => 'ᵗ',
        'x' => 'ˣ',
        'y' => 'ʸ',
        'T' => 'ᵀ',
        ' ' => ' ',
        _ => return None,
    })
}

fn subscript(c: char) -> Option<char> {
    Some(match c {
        '0' => '₀',
        '1' => '₁',
        '2' => '₂',
        '3' => '₃',
        '4' => '₄',
        '5' => '₅',
        '6' => '₆',
        '7' => '₇',
        '8' => '₈',
        '9' => '₉',
        '+' => '₊',
        '-' | '−' => '₋',
        '=' => '₌',
        '(' => '₍',
        ')' => '₎',
        'a' => 'ₐ',
        'e' => 'ₑ',
        'i' => 'ᵢ',
        'j' => 'ⱼ',
        'k' => 'ₖ',
        'm' => 'ₘ',
        'n' => 'ₙ',
        'o' => 'ₒ',
        'r' => 'ᵣ',
        't' => 'ₜ',
        'x' => 'ₓ',
        ' ' => ' ',
        _ => return None,
    })
}

/// Best-effort plain-text rendering of LaTeX math, for terminals that cannot
/// show images. Unknown commands are kept as written.
pub fn math_to_unicode(tex: &str) -> String {
    let chars: Vec<char> = tex.chars().collect();
    let mut pos = 0;
    let out = convert(&chars, &mut pos, false);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Reads one argument: a braced group, a command, or a single character.
fn argument(chars: &[char], pos: &mut usize) -> String {
    while *pos < chars.len() && chars[*pos] == ' ' {
        *pos += 1;
    }
    match chars.get(*pos) {
        Some('{') => {
            *pos += 1;
            convert(chars, pos, true)
        }
        Some('\\') => command(chars, pos),
        Some(c) => {
            *pos += 1;
            c.to_string()
        }
        None => String::new(),
    }
}

fn scripted(arg: &str, map: fn(char) -> Option<char>, marker: char) -> String {
    match arg.chars().map(map).collect::<Option<String>>() {
        Some(mapped) => mapped,
        None if arg.chars().count() == 1 => format!("{marker}{arg}"),
        None => format!("{marker}({arg})"),
    }
}

fn group(text: &str) -> String {
    if text.chars().count() > 1 && text.chars().any(|c| !c.is_alphanumeric()) {
        format!("({text})")
    } else {
        text.to_string()
    }
}

fn command(chars: &[char], pos: &mut usize) -> String {
    *pos += 1; // backslash
    let start = *pos;
    while *pos < chars.len() && chars[*pos].is_ascii_alphabetic() {
        *pos += 1;
    }
    if *pos == start {
        // Control symbol such as `\,` `\\` `\{`.
        let c = chars.get(*pos).copied();
        *pos += 1;
        return match c {
            Some('\\') => "\n".into(),
            Some(',' | ';' | ':' | '!' | ' ') => " ".into(),
            Some(c) => c.to_string(),
            None => String::new(),
        };
    }
    let name: String = chars[start..*pos].iter().collect();
    match name.as_str() {
        "frac" | "dfrac" | "tfrac" => {
            let num = argument(chars, pos);
            let den = argument(chars, pos);
            format!("{}/{}", group(&num), group(&den))
        }
        "sqrt" => {
            let arg = argument(chars, pos);
            format!("√{}", group(&arg))
        }
        "mathbb" => {
            let arg = argument(chars, pos);
            arg.chars()
                .map(|c| {
                    BLACKBOARD
                        .iter()
                        .find(|(k, _)| *k == c)
                        .map_or(c, |(_, v)| *v)
                })
                .collect()
        }
        "text" | "textbf" | "textit" | "mathrm" | "mathbf" | "mathit" | "mathcal" | "mathsf"
        | "mathtt" | "operatorname" | "boldsymbol" | "bm" => argument(chars, pos),
        "hat" => format!("{}\u{302}", argument(chars, pos)),
        "bar" | "overline" => format!("{}\u{304}", argument(chars, pos)),
        "vec" => format!("{}\u{20d7}", argument(chars, pos)),
        "tilde" => format!("{}\u{303}", argument(chars, pos)),
        "dot" => format!("{}\u{307}", argument(chars, pos)),
        "left" | "right" | "big" | "Big" | "bigg" | "Bigg" | "displaystyle" | "textstyle"
        | "limits" | "nolimits" => String::new(),
        "begin" | "end" => {
            argument(chars, pos);
            String::new()
        }
        _ => match SYMBOLS.iter().find(|(k, _)| *k == name) {
            Some((_, symbol)) => symbol.to_string(),
            // Function names (`\sin`, `\log`, ...) read fine without the slash.
            None => name,
        },
    }
}

fn convert(chars: &[char], pos: &mut usize, in_group: bool) -> String {
    let mut out = String::new();
    while *pos < chars.len() {
        match chars[*pos] {
            '}' if in_group => {
                *pos += 1;
                break;
            }
            '{' => {
                *pos += 1;
                out.push_str(&convert(chars, pos, true));
            }
            '\\' => out.push_str(&command(chars, pos)),
            '^' => {
                *pos += 1;
                out.push_str(&scripted(&argument(chars, pos), superscript, '^'));
            }
            '_' => {
                *pos += 1;
                out.push_str(&scripted(&argument(chars, pos), subscript, '_'));
            }
            '&' => {
                *pos += 1;
                out.push(' ');
            }
            '~' => {
                *pos += 1;
                out.push(' ');
            }
            c => {
                *pos += 1;
                out.push(c);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_fallback_covers_common_notation() {
        assert_eq!(math_to_unicode(r"E = mc^2"), "E = mc²");
        assert_eq!(math_to_unicode(r"\alpha_i \leq \beta^{n+1}"), "αᵢ ≤ βⁿ⁺¹");
        assert_eq!(math_to_unicode(r"\frac{a+b}{2}"), "(a+b)/2");
        assert_eq!(math_to_unicode(r"\sqrt{x} \in \mathbb{R}"), "√x ∈ ℝ");
        assert_eq!(math_to_unicode(r"\sum_{i=1}^{n} x_i"), "∑ᵢ₌₁ⁿ xᵢ");
        assert_eq!(math_to_unicode(r"x^{\pi}"), "x^π");
    }

    #[test]
    fn formulas_render_to_a_transparent_image() {
        let palette = Palette {
            dark: true,
            foreground: [255, 255, 255],
        };
        let image = math_to_image(r"\int_0^1 x^2\,dx = \frac{1}{3}", true, palette, 32.0).unwrap();
        assert!(image.width() > 100 && image.height() > 30);
        assert_eq!(image.get_pixel(0, 0)[3], 0);
        assert!(image.pixels().any(|p| p[3] > 200 && p[0] > 200));
        assert!(math_to_image(r"\frac{", true, palette, 32.0).is_err());
    }
}

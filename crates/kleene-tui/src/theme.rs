//! Colours and the shared building blocks of every panel. The palettes are
//! the four Catppuccin flavours, mapped by the Catppuccin style guide: Base
//! for the ground, Text and the Subtexts for type, Mauve as the accent,
//! Green, Yellow and Red for states, the Surfaces for borders and selection,
//! Lavender for focus.

use ratatui::layout::Alignment;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Padding};

/// A Catppuccin flavour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    /// The darkest; the default.
    Mocha,
    /// Dark, a little warmer.
    Macchiato,
    /// The softest dark.
    Frappe,
    /// Light.
    Latte,
}

impl Flavor {
    /// All flavours in `t` order.
    pub const ALL: [Flavor; 4] = [
        Flavor::Mocha,
        Flavor::Macchiato,
        Flavor::Frappe,
        Flavor::Latte,
    ];

    /// Name as Catppuccin spells it.
    pub const fn name(self) -> &'static str {
        match self {
            Flavor::Mocha => "mocha",
            Flavor::Macchiato => "macchiato",
            Flavor::Frappe => "frappé",
            Flavor::Latte => "latte",
        }
    }

    /// The next flavour in the cycle.
    pub const fn next(self) -> Flavor {
        match self {
            Flavor::Mocha => Flavor::Macchiato,
            Flavor::Macchiato => Flavor::Frappe,
            Flavor::Frappe => Flavor::Latte,
            Flavor::Latte => Flavor::Mocha,
        }
    }

    /// By name, case-insensitive (`frappe` works too).
    pub fn parse(s: &str) -> Option<Flavor> {
        match s.trim().to_lowercase().as_str() {
            "mocha" => Some(Flavor::Mocha),
            "macchiato" => Some(Flavor::Macchiato),
            "frappe" | "frappé" => Some(Flavor::Frappe),
            "latte" => Some(Flavor::Latte),
            _ => None,
        }
    }
}

/// The Catppuccin palette of one flavour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// Ground.
    pub base: Color,
    /// A step below the ground: header and footer bands.
    pub mantle: Color,
    /// The darkest (lightest, on Latte) shade.
    pub crust: Color,
    /// Selection ground.
    pub surface0: Color,
    /// Borders.
    pub surface1: Color,
    /// Stronger borders and rules.
    pub surface2: Color,
    /// Faint text.
    pub overlay0: Color,
    /// Secondary text.
    pub subtext0: Color,
    /// Body text.
    pub text: Color,
    /// Focus.
    pub lavender: Color,
    /// Links and info.
    pub blue: Color,
    /// SQL.
    pub sapphire: Color,
    /// Tools.
    pub teal: Color,
    /// Success and "live".
    pub green: Color,
    /// Attention.
    pub yellow: Color,
    /// Money.
    pub peach: Color,
    /// Failure.
    pub red: Color,
    /// The accent.
    pub mauve: Color,
    /// Model calls.
    pub pink: Color,
}

const fn rgb(hex: u32) -> Color {
    Color::Rgb(
        ((hex >> 16) & 0xff) as u8,
        ((hex >> 8) & 0xff) as u8,
        (hex & 0xff) as u8,
    )
}

impl Palette {
    /// The palette of a flavour, values from the Catppuccin specification.
    pub const fn of(flavor: Flavor) -> Palette {
        match flavor {
            Flavor::Mocha => Palette {
                base: rgb(0x1e1e2e),
                mantle: rgb(0x181825),
                crust: rgb(0x11111b),
                surface0: rgb(0x313244),
                surface1: rgb(0x45475a),
                surface2: rgb(0x585b70),
                overlay0: rgb(0x6c7086),
                subtext0: rgb(0xa6adc8),
                text: rgb(0xcdd6f4),
                lavender: rgb(0xb4befe),
                blue: rgb(0x89b4fa),
                sapphire: rgb(0x74c7ec),
                teal: rgb(0x94e2d5),
                green: rgb(0xa6e3a1),
                yellow: rgb(0xf9e2af),
                peach: rgb(0xfab387),
                red: rgb(0xf38ba8),
                mauve: rgb(0xcba6f7),
                pink: rgb(0xf5c2e7),
            },
            Flavor::Macchiato => Palette {
                base: rgb(0x24273a),
                mantle: rgb(0x1e2030),
                crust: rgb(0x181926),
                surface0: rgb(0x363a4f),
                surface1: rgb(0x494d64),
                surface2: rgb(0x5b6078),
                overlay0: rgb(0x6e738d),
                subtext0: rgb(0xa5adcb),
                text: rgb(0xcad3f5),
                lavender: rgb(0xb7bdf8),
                blue: rgb(0x8aadf4),
                sapphire: rgb(0x7dc4e4),
                teal: rgb(0x8bd5ca),
                green: rgb(0xa6da95),
                yellow: rgb(0xeed49f),
                peach: rgb(0xf5a97f),
                red: rgb(0xed8796),
                mauve: rgb(0xc6a0f6),
                pink: rgb(0xf5bde6),
            },
            Flavor::Frappe => Palette {
                base: rgb(0x303446),
                mantle: rgb(0x292c3c),
                crust: rgb(0x232634),
                surface0: rgb(0x414559),
                surface1: rgb(0x51576d),
                surface2: rgb(0x626880),
                overlay0: rgb(0x737994),
                subtext0: rgb(0xa5adce),
                text: rgb(0xc6d0f5),
                lavender: rgb(0xbabbf1),
                blue: rgb(0x8caaee),
                sapphire: rgb(0x85c1dc),
                teal: rgb(0x81c8be),
                green: rgb(0xa6d189),
                yellow: rgb(0xe5c890),
                peach: rgb(0xef9f76),
                red: rgb(0xe78284),
                mauve: rgb(0xca9ee6),
                pink: rgb(0xf4b8e4),
            },
            Flavor::Latte => Palette {
                base: rgb(0xeff1f5),
                mantle: rgb(0xe6e9ef),
                crust: rgb(0xdce0e8),
                surface0: rgb(0xccd0da),
                surface1: rgb(0xbcc0cc),
                surface2: rgb(0xacb0be),
                overlay0: rgb(0x9ca0b0),
                subtext0: rgb(0x6c6f85),
                text: rgb(0x4c4f69),
                lavender: rgb(0x7287fd),
                blue: rgb(0x1e66f5),
                sapphire: rgb(0x209fb5),
                teal: rgb(0x179299),
                green: rgb(0x40a02b),
                yellow: rgb(0xdf8e1d),
                peach: rgb(0xfe640b),
                red: rgb(0xd20f39),
                mauve: rgb(0x8839ef),
                pink: rgb(0xea76cb),
            },
        }
    }
}

/// A flavour plus the styles derived from it. The role fields are what the
/// views use; the palette is there for the odd one-off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Which flavour this is.
    pub flavor: Flavor,
    /// Its name, for the header.
    pub name: &'static str,
    /// The full palette.
    pub palette: Palette,
    /// Screen background (Base).
    pub bg: Color,
    /// Header and footer bands (Mantle).
    pub band: Color,
    /// Body text (Text).
    pub fg: Color,
    /// Secondary text (Subtext0).
    pub muted: Color,
    /// Faint text (Overlay0).
    pub faint: Color,
    /// Brand and emphasis (Mauve).
    pub accent: Color,
    /// Text on an accent background (Crust).
    pub on_accent: Color,
    /// Success and "live" (Green).
    pub ok: Color,
    /// Attention without failure (Yellow).
    pub warn: Color,
    /// Failure (Red).
    pub err: Color,
    /// Money (Peach).
    pub money: Color,
    /// SQL (Sapphire).
    pub sql: Color,
    /// Model calls (Pink).
    pub call: Color,
    /// Tools (Teal).
    pub tool: Color,
    /// Panel borders (Surface1).
    pub border: Color,
    /// Border of the focused panel (Lavender).
    pub border_focus: Color,
    /// Selected row background (Surface0).
    pub sel_bg: Color,
    /// Selected row foreground (Text).
    pub sel_fg: Color,
}

impl Theme {
    /// The theme of a flavour.
    pub const fn flavor(flavor: Flavor) -> Self {
        let p = Palette::of(flavor);
        Self {
            flavor,
            name: flavor.name(),
            palette: p,
            bg: p.base,
            band: p.mantle,
            fg: p.text,
            muted: p.subtext0,
            faint: p.overlay0,
            accent: p.mauve,
            on_accent: p.crust,
            ok: p.green,
            warn: p.yellow,
            err: p.red,
            money: p.peach,
            sql: p.sapphire,
            call: p.pink,
            tool: p.teal,
            border: p.surface1,
            border_focus: p.lavender,
            sel_bg: p.surface0,
            sel_fg: p.text,
        }
    }

    /// The default: Mocha.
    pub const fn dark() -> Self {
        Self::flavor(Flavor::Mocha)
    }

    /// Latte.
    pub const fn light() -> Self {
        Self::flavor(Flavor::Latte)
    }

    /// The next flavour in the cycle (`t`).
    pub const fn toggled(self) -> Self {
        Self::flavor(self.flavor.next())
    }

    /// Body text style.
    pub fn text(&self) -> Style {
        Style::default().fg(self.fg)
    }

    /// Secondary text style.
    pub fn dim(&self) -> Style {
        Style::default().fg(self.muted)
    }

    /// Accent text style.
    pub fn accent_text(&self) -> Style {
        Style::default()
            .fg(self.accent)
            .add_modifier(Modifier::BOLD)
    }

    /// A key chip: the key on an accent background, for the footer.
    pub fn chip<'a>(&self, key: &'a str, label: &'a str) -> Vec<Span<'a>> {
        vec![
            Span::styled(
                format!(" {key} "),
                Style::default()
                    .fg(self.on_accent)
                    .bg(self.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" {label}  "), self.dim()),
        ]
    }

    /// A rounded, titled panel. `focus` brightens the border.
    pub fn panel<'a>(&self, title: impl Into<String>, focus: bool) -> Block<'a> {
        let title: String = title.into();
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(if focus {
                self.border_focus
            } else {
                self.border
            }))
            .title(Line::from(vec![Span::styled(
                format!(" {title} "),
                self.accent_text(),
            )]))
            .padding(Padding::horizontal(1))
            .style(Style::default().bg(self.bg).fg(self.fg))
    }

    /// A panel with a right-aligned secondary title.
    pub fn panel_with_meta<'a>(
        &self,
        title: impl Into<String>,
        meta: impl Into<String>,
        focus: bool,
    ) -> Block<'a> {
        let meta: String = meta.into();
        self.panel(title, focus).title(
            Line::from(Span::styled(format!(" {meta} "), self.dim())).alignment(Alignment::Right),
        )
    }

    /// The colour for a session or statement state.
    pub fn state(&self, running: bool, failed: bool) -> Color {
        if failed {
            self.err
        } else if running {
            self.ok
        } else {
            self.fg
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

/// The brand mark used in the header and the welcome card.
pub const BRAND: &str = "◆ kleene";
/// One-line tagline.
pub const TAGLINE: &str = "relational algebra for recursive model calls";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flavours_cycle_and_parse() {
        let mut t = Theme::default();
        assert_eq!(t.flavor, Flavor::Mocha);
        let mut seen = vec![t.name];
        for _ in 0..3 {
            t = t.toggled();
            seen.push(t.name);
        }
        assert_eq!(seen, ["mocha", "macchiato", "frappé", "latte"]);
        assert_eq!(t.toggled().flavor, Flavor::Mocha);
        assert_eq!(Flavor::parse("Frappe"), Some(Flavor::Frappe));
        assert_eq!(Flavor::parse("nord"), None);
        // Latte is the light one: its ground is lighter than its text.
        let l = Palette::of(Flavor::Latte);
        let (Color::Rgb(r, ..), Color::Rgb(tr, ..)) = (l.base, l.text) else {
            panic!("rgb")
        };
        assert!(r > tr);
    }
}

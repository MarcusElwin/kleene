//! Colours and the shared building blocks of every panel, so the views agree
//! with each other and a theme swap is one value.

use ratatui::layout::Alignment;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Padding};

/// A palette plus the styles derived from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Which palette this is.
    pub name: &'static str,
    /// Screen background.
    pub bg: Color,
    /// Body text.
    pub fg: Color,
    /// Secondary text: hints, labels, timestamps.
    pub muted: Color,
    /// Brand and focus colour.
    pub accent: Color,
    /// Text on an accent background.
    pub on_accent: Color,
    /// Success and "live".
    pub ok: Color,
    /// Attention without failure: budgets, notices.
    pub warn: Color,
    /// Failure.
    pub err: Color,
    /// Panel borders.
    pub border: Color,
    /// Border of the focused panel.
    pub border_focus: Color,
    /// Selected row background.
    pub sel_bg: Color,
    /// Selected row foreground.
    pub sel_fg: Color,
}

impl Theme {
    /// The default: a dark palette in the style of modern terminal agents.
    pub const fn dark() -> Self {
        Self {
            name: "dark",
            bg: Color::Rgb(17, 19, 24),
            fg: Color::Rgb(214, 218, 226),
            muted: Color::Rgb(122, 130, 145),
            accent: Color::Rgb(125, 207, 255),
            on_accent: Color::Rgb(17, 19, 24),
            ok: Color::Rgb(120, 220, 140),
            warn: Color::Rgb(255, 196, 96),
            err: Color::Rgb(255, 110, 110),
            border: Color::Rgb(58, 63, 74),
            border_focus: Color::Rgb(125, 207, 255),
            sel_bg: Color::Rgb(38, 48, 64),
            sel_fg: Color::Rgb(240, 244, 250),
        }
    }

    /// A light palette for pale terminals.
    pub const fn light() -> Self {
        Self {
            name: "light",
            bg: Color::Rgb(250, 250, 248),
            fg: Color::Rgb(36, 40, 48),
            muted: Color::Rgb(112, 118, 130),
            accent: Color::Rgb(0, 110, 190),
            on_accent: Color::Rgb(255, 255, 255),
            ok: Color::Rgb(20, 140, 70),
            warn: Color::Rgb(180, 110, 0),
            err: Color::Rgb(200, 40, 40),
            border: Color::Rgb(200, 204, 212),
            border_focus: Color::Rgb(0, 110, 190),
            sel_bg: Color::Rgb(220, 232, 248),
            sel_fg: Color::Rgb(20, 24, 32),
        }
    }

    /// The other palette.
    pub const fn toggled(self) -> Self {
        if matches!(self.bg, Color::Rgb(17, 19, 24)) {
            Self::light()
        } else {
            Self::dark()
        }
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

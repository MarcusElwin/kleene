//! First-run onboarding: pick the providers to use, enter their keys, save
//! the config file. A pure state machine ([`SetupApp`]) plus a renderer, so
//! `kleene setup` and the TUI's first start share it and tests drive it with
//! key events.

use crate::theme::{Theme, BRAND, TAGLINE};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use kleene_llm::{AnthropicSettings, OpenAiCompatSettings, ProviderSettings};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use ratatui::Frame;
use std::path::PathBuf;

/// A provider the wizard can configure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// Anthropic, native Messages API.
    Anthropic,
    /// OpenAI itself.
    OpenAi,
    /// Any OpenAI-compatible endpoint: a local server, a gateway.
    Compatible,
}

impl Choice {
    /// All choices, in display order.
    pub const ALL: [Choice; 3] = [Choice::Anthropic, Choice::OpenAi, Choice::Compatible];

    fn title(self) -> &'static str {
        match self {
            Choice::Anthropic => "Anthropic",
            Choice::OpenAi => "OpenAI",
            Choice::Compatible => "OpenAI-compatible endpoint",
        }
    }

    fn blurb(self) -> &'static str {
        match self {
            Choice::Anthropic => {
                "Claude models. Routes root/worker/proxy/judge across Opus, Sonnet and Haiku by default."
            }
            Choice::OpenAi => "GPT models on api.openai.com. One model serves every alias.",
            Choice::Compatible => {
                "Ollama, vLLM, LM Studio, a gateway: anything speaking the chat-completions API."
            }
        }
    }

    fn env_hint(self) -> &'static str {
        match self {
            Choice::Anthropic => "ANTHROPIC_API_KEY",
            Choice::OpenAi => "OPENAI_API_KEY",
            Choice::Compatible => "OPENAI_BASE_URL",
        }
    }

    /// OpenAI and a compatible endpoint both fill the same settings slot.
    fn conflicts_with(self, other: Choice) -> bool {
        matches!(
            (self, other),
            (Choice::OpenAi, Choice::Compatible) | (Choice::Compatible, Choice::OpenAi)
        )
    }
}

/// One text field of a provider page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    /// Label shown left of the input.
    pub label: &'static str,
    /// What the field is for, shown under it.
    pub help: &'static str,
    /// Current text.
    pub value: String,
    /// Render as dots.
    pub secret: bool,
    /// May be left empty.
    pub optional: bool,
}

impl Field {
    fn new(
        label: &'static str,
        help: &'static str,
        value: String,
        secret: bool,
        optional: bool,
    ) -> Self {
        Self {
            label,
            help,
            value,
            secret,
            optional,
        }
    }

    fn shown(&self) -> String {
        if self.secret {
            mask(&self.value)
        } else {
            self.value.clone()
        }
    }
}

/// A key shown with only its edges: enough to recognise, useless to copy.
pub fn mask(s: &str) -> String {
    let n = s.chars().count();
    if n == 0 {
        String::new()
    } else if n <= 8 {
        "•".repeat(n)
    } else {
        let head: String = s.chars().take(4).collect();
        let tail: String = s.chars().skip(n - 4).collect();
        format!("{head}{}{tail}", "•".repeat(n.min(24) - 8))
    }
}

/// Where the wizard is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Toggle providers.
    Choose,
    /// Fill the fields of the n-th selected provider.
    Fields(usize),
    /// Confirm and save.
    Review,
}

/// What a key press decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetupAction {
    /// Keep going.
    None,
    /// Leave without saving.
    Cancel,
    /// Save these settings.
    Save(ProviderSettings),
}

/// The wizard state.
#[derive(Debug, Clone)]
pub struct SetupApp {
    /// Current step.
    pub step: Step,
    /// Which choices are ticked.
    pub chosen: [bool; 3],
    /// Highlighted row on the choose page.
    pub cursor: usize,
    /// Fields per choice, in [`Choice::ALL`] order.
    pub fields: [Vec<Field>; 3],
    /// Focused field on a provider page.
    pub field_cursor: usize,
    /// Settings the wizard started from; untouched providers are kept.
    pub existing: ProviderSettings,
    /// Where the result will be written, for the review page.
    pub path: PathBuf,
    /// A validation message.
    pub error: Option<String>,
    /// Colours.
    pub theme: Theme,
}

impl SetupApp {
    /// Start from what is configured now (its fields are pre-filled and its
    /// providers pre-ticked) and the path the result goes to.
    pub fn new(existing: ProviderSettings, path: PathBuf) -> Self {
        let a = existing.anthropic.clone().unwrap_or_default();
        let o = existing.openai_compat.clone().unwrap_or_default();
        let o_is_openai = o
            .base_url
            .as_deref()
            .map(|u| u.contains("api.openai.com"))
            .unwrap_or(o.api_key.is_some());
        let fields = [
            vec![
                Field::new(
                    "API key",
                    "from console.anthropic.com; ANTHROPIC_AUTH_TOKEN works too",
                    a.api_key
                        .clone()
                        .or(a.auth_token.clone())
                        .unwrap_or_default(),
                    true,
                    false,
                ),
                Field::new(
                    "Base URL",
                    "leave empty for api.anthropic.com",
                    a.base_url.clone().unwrap_or_default(),
                    false,
                    true,
                ),
            ],
            vec![
                Field::new(
                    "API key",
                    "from platform.openai.com",
                    if o_is_openai {
                        o.api_key.clone().unwrap_or_default()
                    } else {
                        String::new()
                    },
                    true,
                    false,
                ),
                Field::new(
                    "Model",
                    "the model every alias resolves to",
                    if o_is_openai {
                        o.model.clone().unwrap_or_default()
                    } else {
                        kleene_llm::env::DEFAULT_OPENAI_MODEL.to_string()
                    },
                    false,
                    true,
                ),
            ],
            vec![
                Field::new(
                    "Base URL",
                    "e.g. http://localhost:11434/v1 for Ollama",
                    o.base_url
                        .clone()
                        .filter(|_| !o_is_openai)
                        .unwrap_or_else(|| "http://localhost:11434/v1".to_string()),
                    false,
                    false,
                ),
                Field::new(
                    "API key",
                    "leave empty for a local server without one",
                    if o_is_openai {
                        String::new()
                    } else {
                        o.api_key.clone().unwrap_or_default()
                    },
                    true,
                    true,
                ),
                Field::new(
                    "Model",
                    "the model every alias resolves to",
                    if o_is_openai {
                        String::new()
                    } else {
                        o.model.clone().unwrap_or_default()
                    },
                    false,
                    false,
                ),
            ],
        ];
        let chosen = [
            a.is_configured(),
            o.is_configured() && o_is_openai,
            o.is_configured() && !o_is_openai,
        ];
        Self {
            step: Step::Choose,
            chosen,
            cursor: 0,
            fields,
            field_cursor: 0,
            existing,
            path,
            error: None,
            theme: Theme::default(),
        }
    }

    /// The ticked choices in order.
    pub fn selected(&self) -> Vec<Choice> {
        Choice::ALL
            .iter()
            .copied()
            .enumerate()
            .filter(|(i, _)| self.chosen[*i])
            .map(|(_, c)| c)
            .collect()
    }

    fn index(c: Choice) -> usize {
        Choice::ALL.iter().position(|x| *x == c).unwrap_or(0)
    }

    /// The settings the current answers describe.
    pub fn settings(&self) -> ProviderSettings {
        let mut s = self.existing.clone();
        let get = |c: Choice, i: usize| -> Option<String> {
            self.fields[Self::index(c)]
                .get(i)
                .map(|f| f.value.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        if self.chosen[Self::index(Choice::Anthropic)] {
            let cred = get(Choice::Anthropic, 0);
            let looks_like_token = cred.as_deref().is_some_and(|c| !c.starts_with("sk-"));
            s.anthropic = Some(AnthropicSettings {
                api_key: if looks_like_token { None } else { cred.clone() },
                auth_token: if looks_like_token { cred } else { None },
                base_url: get(Choice::Anthropic, 1),
            });
        } else {
            s.anthropic = None;
        }
        if self.chosen[Self::index(Choice::OpenAi)] {
            s.openai_compat = Some(OpenAiCompatSettings {
                api_key: get(Choice::OpenAi, 0),
                base_url: None,
                model: get(Choice::OpenAi, 1),
            });
        } else if self.chosen[Self::index(Choice::Compatible)] {
            s.openai_compat = Some(OpenAiCompatSettings {
                base_url: get(Choice::Compatible, 0),
                api_key: get(Choice::Compatible, 1),
                model: get(Choice::Compatible, 2),
            });
        } else {
            s.openai_compat = None;
        }
        s
    }

    fn validate_page(&self, c: Choice) -> Option<String> {
        self.fields[Self::index(c)]
            .iter()
            .find(|f| !f.optional && f.value.trim().is_empty())
            .map(|f| format!("{} needs a {}", c.title(), f.label.to_lowercase()))
    }

    /// Handle a key.
    pub fn key(&mut self, key: KeyEvent) -> SetupAction {
        if key.kind != KeyEventKind::Press {
            return SetupAction::None;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return SetupAction::Cancel;
        }
        self.error = None;
        match self.step {
            Step::Choose => match key.code {
                KeyCode::Char('q') | KeyCode::Esc => SetupAction::Cancel,
                KeyCode::Char('t') => {
                    self.theme = self.theme.toggled();
                    SetupAction::None
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.cursor = self.cursor.saturating_sub(1);
                    SetupAction::None
                }
                KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                    self.cursor = (self.cursor + 1).min(Choice::ALL.len() - 1);
                    SetupAction::None
                }
                KeyCode::Char(' ') | KeyCode::Char('x') => {
                    let c = Choice::ALL[self.cursor];
                    self.chosen[self.cursor] = !self.chosen[self.cursor];
                    if self.chosen[self.cursor] {
                        for (i, other) in Choice::ALL.iter().enumerate() {
                            if c.conflicts_with(*other) {
                                self.chosen[i] = false;
                            }
                        }
                    }
                    SetupAction::None
                }
                KeyCode::Enter => {
                    if self.selected().is_empty() {
                        self.error = Some("tick at least one provider (space)".into());
                        return SetupAction::None;
                    }
                    self.step = Step::Fields(0);
                    self.field_cursor = 0;
                    SetupAction::None
                }
                _ => SetupAction::None,
            },
            Step::Fields(n) => {
                let selected = self.selected();
                let Some(&c) = selected.get(n) else {
                    self.step = Step::Review;
                    return SetupAction::None;
                };
                let idx = Self::index(c);
                let count = self.fields[idx].len();
                match key.code {
                    KeyCode::Esc => {
                        if n == 0 {
                            self.step = Step::Choose;
                        } else {
                            self.step = Step::Fields(n - 1);
                        }
                        self.field_cursor = 0;
                        SetupAction::None
                    }
                    KeyCode::Tab | KeyCode::Down => {
                        self.field_cursor = (self.field_cursor + 1) % count;
                        SetupAction::None
                    }
                    KeyCode::BackTab | KeyCode::Up => {
                        self.field_cursor = (self.field_cursor + count - 1) % count;
                        SetupAction::None
                    }
                    KeyCode::Backspace => {
                        self.fields[idx][self.field_cursor].value.pop();
                        SetupAction::None
                    }
                    KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.fields[idx][self.field_cursor].value.clear();
                        SetupAction::None
                    }
                    KeyCode::Enter => {
                        if self.field_cursor + 1 < count {
                            self.field_cursor += 1;
                            return SetupAction::None;
                        }
                        if let Some(e) = self.validate_page(c) {
                            self.error = Some(e);
                            return SetupAction::None;
                        }
                        self.field_cursor = 0;
                        if n + 1 < selected.len() {
                            self.step = Step::Fields(n + 1);
                        } else {
                            self.step = Step::Review;
                        }
                        SetupAction::None
                    }
                    KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.fields[idx][self.field_cursor].value.push(ch);
                        SetupAction::None
                    }
                    _ => SetupAction::None,
                }
            }
            Step::Review => match key.code {
                KeyCode::Esc => {
                    let n = self.selected().len();
                    self.step = if n == 0 {
                        Step::Choose
                    } else {
                        Step::Fields(n - 1)
                    };
                    SetupAction::None
                }
                KeyCode::Char('q') => SetupAction::Cancel,
                KeyCode::Char('t') => {
                    self.theme = self.theme.toggled();
                    SetupAction::None
                }
                KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('s') => {
                    let s = self.settings();
                    if !s.is_configured() {
                        self.error = Some(
                            "nothing usable: every provider needs a credential or a URL".into(),
                        );
                        return SetupAction::None;
                    }
                    SetupAction::Save(s)
                }
                _ => SetupAction::None,
            },
        }
    }
}

/// Draw the wizard.
pub fn draw(f: &mut Frame<'_>, app: &SetupApp) {
    let t = app.theme;
    let area = f.area();
    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new("").style(Style::default().bg(t.bg)), area);
    let width = area.width.clamp(40, 78);
    let height = area.height.clamp(12, 32);
    let card = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    let title = match app.step {
        Step::Choose => "Welcome",
        Step::Fields(_) => "Credentials",
        Step::Review => "Review",
    };
    let step_no = match app.step {
        Step::Choose => 1,
        Step::Fields(n) => 2 + n,
        Step::Review => 2 + app.selected().len(),
    };
    let total = 2 + app.selected().len().max(1);
    let block = t.panel_with_meta(
        format!("{BRAND} · setup · {title}"),
        format!("step {step_no} of {total}"),
        true,
    );
    let inner = block.inner(card);
    f.render_widget(block, card);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(2)])
        .split(inner);
    let mut lines: Vec<Line> = Vec::new();
    match app.step {
        Step::Choose => {
            if inner.width >= crate::theme::WORDMARK_WIDTH + 2 && inner.height >= 24 {
                lines.extend(t.wordmark());
            } else {
                lines.push(Line::from(Span::styled(TAGLINE, t.dim())));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Which providers should Kleene use? Space ticks, Enter continues.",
                t.text(),
            )));
            lines.push(Line::from(""));
            for (i, c) in Choice::ALL.iter().enumerate() {
                let tick = if app.chosen[i] { "◉" } else { "○" };
                let pointer = if i == app.cursor { "▸" } else { " " };
                let style = if i == app.cursor {
                    Style::default()
                        .fg(t.sel_fg)
                        .bg(t.sel_bg)
                        .add_modifier(Modifier::BOLD)
                } else {
                    t.text()
                };
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{pointer} {tick} "),
                        if app.chosen[i] {
                            Style::default().fg(t.accent)
                        } else {
                            t.dim()
                        },
                    ),
                    Span::styled(format!("{:<28}", c.title()), style),
                    Span::styled(c.env_hint(), t.dim()),
                ]));
                lines.push(Line::from(vec![
                    Span::raw("      "),
                    Span::styled(c.blurb(), t.dim()),
                ]));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Keys are stored owner-readable in the config file and can be overridden by the environment.",
                t.dim(),
            )));
        }
        Step::Fields(n) => {
            let selected = app.selected();
            if let Some(&c) = selected.get(n) {
                lines.push(Line::from(vec![
                    Span::styled(c.title(), t.accent_text()),
                    Span::styled(format!("  ({})", c.env_hint()), t.dim()),
                ]));
                lines.push(Line::from(""));
                let idx = Choice::ALL.iter().position(|x| *x == c).unwrap_or(0);
                for (i, field) in app.fields[idx].iter().enumerate() {
                    let focused = i == app.field_cursor;
                    let caret = if focused { "▏" } else { " " };
                    let shown = field.shown();
                    let value_style = if focused {
                        Style::default().fg(t.sel_fg).bg(t.sel_bg)
                    } else {
                        t.text()
                    };
                    let placeholder = if field.value.is_empty() {
                        if field.optional {
                            "(optional)"
                        } else {
                            "(required)"
                        }
                    } else {
                        ""
                    };
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("{:<10}", field.label),
                            if focused { t.accent_text() } else { t.dim() },
                        ),
                        Span::styled(caret, Style::default().fg(t.accent)),
                        Span::styled(format!("{shown:<40}"), value_style),
                        Span::styled(placeholder, t.dim()),
                    ]));
                    lines.push(Line::from(vec![
                        Span::raw("           "),
                        Span::styled(field.help, t.dim()),
                    ]));
                    lines.push(Line::from(""));
                }
                lines.push(Line::from(Span::styled(
                    "Type or paste. Tab moves between fields, Enter continues, Esc goes back, Ctrl-U clears.",
                    t.dim(),
                )));
            }
        }
        Step::Review => {
            let s = app.settings();
            lines.push(Line::from(Span::styled("About to write", t.text())));
            lines.push(Line::from(Span::styled(
                app.path.display().to_string(),
                t.accent_text(),
            )));
            lines.push(Line::from(""));
            if let Some(a) = &s.anthropic {
                lines.push(Line::from(vec![
                    Span::styled("Anthropic        ", t.text()),
                    Span::styled(
                        mask(
                            a.api_key
                                .as_deref()
                                .or(a.auth_token.as_deref())
                                .unwrap_or(""),
                        ),
                        t.dim(),
                    ),
                    Span::styled(
                        a.base_url
                            .as_deref()
                            .map(|u| format!("  {u}"))
                            .unwrap_or_default(),
                        t.dim(),
                    ),
                ]));
            }
            if let Some(o) = &s.openai_compat {
                lines.push(Line::from(vec![
                    Span::styled("OpenAI-compat    ", t.text()),
                    Span::styled(
                        o.base_url
                            .clone()
                            .unwrap_or_else(|| "api.openai.com".into()),
                        t.dim(),
                    ),
                    Span::styled(
                        o.api_key
                            .as_deref()
                            .map(|k| format!("  key {}", mask(k)))
                            .unwrap_or_else(|| "  no key".into()),
                        t.dim(),
                    ),
                    Span::styled(
                        o.model
                            .as_deref()
                            .map(|m| format!("  model {m}"))
                            .unwrap_or_default(),
                        t.dim(),
                    ),
                ]));
            }
            if let Some(r) = &s.router {
                lines.push(Line::from(vec![
                    Span::styled("Router           ", t.text()),
                    Span::styled(r.display().to_string(), t.dim()),
                ]));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Environment variables still win over this file, so a one-off KEY=... kleene run keeps working.",
                t.dim(),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Enter saves. Esc goes back. q quits without saving.",
                t.text(),
            )));
        }
    }
    f.render_widget(
        Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }),
        rows[0],
    );
    let footer = match &app.error {
        Some(e) => Line::from(Span::styled(format!("✗ {e}"), Style::default().fg(t.err))),
        None => Line::from(match app.step {
            Step::Choose => [
                t.chip("space", "tick"),
                t.chip("⏎", "continue"),
                t.chip("t", "theme"),
                t.chip("q", "quit"),
            ]
            .concat(),
            Step::Fields(_) => [
                t.chip("tab", "next field"),
                t.chip("⏎", "continue"),
                t.chip("esc", "back"),
            ]
            .concat(),
            Step::Review => [
                t.chip("⏎", "save"),
                t.chip("esc", "back"),
                t.chip("q", "quit"),
            ]
            .concat(),
        }),
    };
    f.render_widget(Paragraph::new(footer).alignment(Alignment::Left), rows[1]);
}

/// Run the wizard in the terminal. Returns the settings to save, or `None`
/// when the user left without saving. Saving is the caller's job so the CLI
/// can report the path.
pub async fn run(
    existing: ProviderSettings,
    path: PathBuf,
) -> Result<Option<ProviderSettings>, crate::TuiError> {
    use crossterm::event::{Event, EventStream};
    use futures::StreamExt;
    let mut app = SetupApp::new(existing, path);
    let mut terminal = ratatui::init();
    let mut keys = EventStream::new();
    let result = loop {
        if let Err(e) = terminal.draw(|f| draw(f, &app)) {
            break Err(crate::TuiError::Io(e));
        }
        match keys.next().await {
            Some(Ok(Event::Key(k))) => match app.key(k) {
                SetupAction::None => {}
                SetupAction::Cancel => break Ok(None),
                SetupAction::Save(s) => break Ok(Some(s)),
            },
            Some(Ok(_)) => {}
            Some(Err(e)) => break Err(crate::TuiError::Io(e)),
            None => break Ok(None),
        }
    };
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn press(app: &mut SetupApp, code: KeyCode) -> SetupAction {
        app.key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn type_text(app: &mut SetupApp, s: &str) {
        for ch in s.chars() {
            press(app, KeyCode::Char(ch));
        }
    }

    #[test]
    fn anthropic_key_end_to_end() {
        let mut app = SetupApp::new(ProviderSettings::default(), "/tmp/x/config.toml".into());
        assert_eq!(press(&mut app, KeyCode::Enter), SetupAction::None);
        assert!(app.error.is_some(), "nothing ticked yet");
        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.step, Step::Fields(0));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.field_cursor, 1, "enter moves to the next field first");
        press(&mut app, KeyCode::Enter);
        assert!(app.error.is_some(), "the key is required");
        press(&mut app, KeyCode::BackTab);
        type_text(&mut app, "sk-ant-secret-1234");
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.step, Step::Review);
        match press(&mut app, KeyCode::Enter) {
            SetupAction::Save(s) => {
                assert_eq!(
                    s.anthropic.unwrap().api_key.as_deref(),
                    Some("sk-ant-secret-1234")
                );
                assert!(s.openai_compat.is_none());
            }
            other => panic!("expected save, got {other:?}"),
        }
    }

    #[test]
    fn openai_and_compatible_are_exclusive_and_local_needs_no_key() {
        let mut app = SetupApp::new(ProviderSettings::default(), "/tmp/x/config.toml".into());
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.selected(), vec![Choice::Compatible]);
        press(&mut app, KeyCode::Enter);
        // Base URL is pre-filled with the Ollama default; key optional; model required.
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Enter);
        assert!(app.error.is_some(), "model required: {:?}", app.error);
        type_text(&mut app, "llama3");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.step, Step::Review);
        match press(&mut app, KeyCode::Char('y')) {
            SetupAction::Save(s) => {
                let o = s.openai_compat.unwrap();
                assert_eq!(o.base_url.as_deref(), Some("http://localhost:11434/v1"));
                assert_eq!(o.api_key, None);
                assert_eq!(o.model.as_deref(), Some("llama3"));
            }
            other => panic!("expected save, got {other:?}"),
        }
    }

    #[test]
    fn existing_settings_prefill_and_survive() {
        let existing = ProviderSettings::parse(
            "router = '/r.toml'\n[anthropic]\napi_key = 'sk-ant-old'\n[openai_compat]\napi_key = 'sk-old'\nmodel = 'gpt-5.4-mini'\n",
        )
        .unwrap();
        let app = SetupApp::new(existing, "/tmp/x/config.toml".into());
        assert_eq!(app.chosen, [true, true, false]);
        assert_eq!(app.fields[0][0].value, "sk-ant-old");
        let s = app.settings();
        assert_eq!(s.router.as_deref(), Some(std::path::Path::new("/r.toml")));
        assert_eq!(s.openai_compat.unwrap().api_key.as_deref(), Some("sk-old"));
    }

    #[test]
    fn masks_keys_and_renders_every_step() {
        assert_eq!(mask(""), "");
        assert_eq!(mask("short"), "•••••");
        let m = mask("sk-ant-api03-abcdefghijklmnop");
        assert!(
            m.starts_with("sk-a") && m.ends_with("mnop") && !m.contains("api03"),
            "{m}"
        );
        let mut app = SetupApp::new(ProviderSettings::default(), "/tmp/x/config.toml".into());
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::Enter);
        type_text(&mut app, "sk-ant-secret-1234");
        terminal.draw(|f| draw(f, &app)).unwrap();
        let text: String = {
            let buf = terminal.backend().buffer();
            (0..buf.area.height)
                .map(|y| {
                    (0..buf.area.width)
                        .map(|x| buf[(x, y)].symbol().to_string())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        assert!(
            !text.contains("secret"),
            "the key is never drawn in clear: {text}"
        );
        assert!(text.contains("Credentials"), "{text}");
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Enter);
        terminal.draw(|f| draw(f, &app)).unwrap();
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.step, Step::Fields(0));
    }
}

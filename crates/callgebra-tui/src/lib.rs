//! Terminal UI: a thin ratatui client over the daemon protocol.
//!
//! Views: session (call tree, transcript, plan sidebar), plan (the selected
//! statement's `EXPLAIN` full width), trace explorer (SQL over the store),
//! and help. State is a [`model::Model`] folded from the daemon's messages,
//! so the UI is a pure function of received events and can be rendered into
//! a test buffer.

#![forbid(unsafe_code)]

pub mod model;
pub mod ui;

use callgebra_daemon::client::{Client, ClientReader, ClientWriter};
use callgebra_daemon::{ClientRequest, ServerMessage};
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures::StreamExt;
use model::{flatten, Folds, Model, TreeRow};
use std::path::Path;
use std::time::Duration;

/// Views the TUI can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// Call tree, transcript, plan sidebar.
    Session,
    /// Full-width plan of the selected statement.
    Plan,
    /// SQL over the trace database.
    Trace,
    /// Continual harness task board (M6).
    Tasks,
    /// Keymap.
    Help,
}

/// TUI errors.
#[derive(Debug, thiserror::Error)]
pub enum TuiError {
    /// Terminal or socket failure.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// The daemon connection failed.
    #[error(transparent)]
    Daemon(#[from] callgebra_daemon::DaemonError),
}

/// Everything the UI needs besides the model.
#[derive(Debug, Clone)]
pub struct App {
    /// What the daemon told us.
    pub model: Model,
    /// Current view.
    pub view: View,
    /// Folded tree rows.
    pub folds: Folds,
    /// Selected row in the flattened tree.
    pub selected: usize,
    /// Transcript scroll offset.
    pub scroll: u16,
    /// Trace explorer input.
    pub query: String,
    /// Whether the query input has focus.
    pub editing: bool,
    /// Compact layout (narrow terminals).
    pub compact: bool,
    /// Whether we are still connected.
    pub connected: bool,
}

impl Default for App {
    fn default() -> Self {
        Self {
            model: Model::default(),
            view: View::Session,
            folds: Folds::default(),
            selected: 0,
            scroll: 0,
            query: "SELECT depth, role, outcome, turns, calls, dollars FROM trace_sessions ORDER BY started_at".into(),
            editing: false,
            compact: false,
            connected: true,
        }
    }
}

/// What a key press asks the daemon to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Nothing outward.
    None,
    /// Send a request.
    Send(ClientRequest),
    /// Leave the UI; the daemon keeps running.
    Detach,
}

impl App {
    /// The flattened tree.
    pub fn rows(&self) -> Vec<(usize, TreeRow)> {
        flatten(&self.model, &self.folds)
    }

    /// The selected row, if any.
    pub fn selected_row(&self) -> Option<TreeRow> {
        self.rows().get(self.selected).map(|(_, r)| r.clone())
    }

    /// Fold a server message in.
    pub fn apply(&mut self, msg: ServerMessage) {
        self.model.apply(msg);
        let n = self.rows().len();
        if n > 0 && self.selected >= n {
            self.selected = n - 1;
        }
    }

    /// Handle a key; returns what to do about it.
    pub fn key(&mut self, key: KeyEvent) -> Action {
        if key.kind != KeyEventKind::Press {
            return Action::None;
        }
        if self.editing {
            return match key.code {
                KeyCode::Esc => {
                    self.editing = false;
                    Action::None
                }
                KeyCode::Enter => {
                    self.editing = false;
                    Action::Send(ClientRequest::Query {
                        sql: self.query.clone(),
                    })
                }
                KeyCode::Backspace => {
                    self.query.pop();
                    Action::None
                }
                KeyCode::Char(c) => {
                    self.query.push(c);
                    Action::None
                }
                _ => Action::None,
            };
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Char('d') => Action::Detach,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Action::Detach,
            KeyCode::Char('1') => {
                self.view = View::Session;
                Action::None
            }
            KeyCode::Char('2') => {
                self.view = View::Plan;
                Action::None
            }
            KeyCode::Char('3') => {
                self.view = View::Trace;
                Action::None
            }
            KeyCode::Char('4') => {
                self.view = View::Tasks;
                Action::None
            }
            KeyCode::Char('?') => {
                self.view = View::Help;
                Action::None
            }
            KeyCode::Char('j') | KeyCode::Down => {
                let n = self.rows().len();
                if n > 0 {
                    self.selected = (self.selected + 1).min(n - 1);
                }
                self.scroll = 0;
                Action::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                self.scroll = 0;
                Action::None
            }
            KeyCode::Char('J') | KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_add(10);
                Action::None
            }
            KeyCode::Char('K') | KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(10);
                Action::None
            }
            KeyCode::Char('f') | KeyCode::Enter | KeyCode::Char(' ') => {
                if let Some(row) = self.selected_row() {
                    self.folds.toggle(&row);
                }
                Action::None
            }
            KeyCode::Char('x') | KeyCode::Esc => match self.selected_row() {
                Some(TreeRow::Statement(s)) => Action::Send(ClientRequest::Cancel { statement: s }),
                Some(TreeRow::Call(c)) => match self.model.call_owner.get(&c) {
                    Some(s) => Action::Send(ClientRequest::Cancel { statement: *s }),
                    None => Action::None,
                },
                Some(TreeRow::Session(s)) => match self.model.sessions.get(&s) {
                    Some(node) if node.parent.is_none() => {
                        Action::Send(ClientRequest::CancelRun { run: node.run })
                    }
                    _ => Action::None,
                },
                None => Action::None,
            },
            KeyCode::Char('/') | KeyCode::Char('e') if self.view == View::Trace => {
                self.editing = true;
                Action::None
            }
            KeyCode::Char('r') if self.view == View::Trace => Action::Send(ClientRequest::Query {
                sql: self.query.clone(),
            }),
            _ => Action::None,
        }
    }
}

/// Run the TUI against a daemon socket until the user detaches or the
/// daemon goes away. `start` optionally starts a run on connect.
pub async fn run(socket: &Path, start: Option<ClientRequest>) -> Result<(), TuiError> {
    let client = Client::connect(socket).await?;
    let generation = client.generation;
    let (reader, mut writer) = client.into_split();
    writer
        .send(&ClientRequest::Subscribe { after: None })
        .await?;
    if let Some(req) = start {
        writer.send(&req).await?;
    }
    let mut app = App::default();
    app.model.generation = Some(generation);

    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app, reader, &mut writer).await;
    ratatui::restore();
    let _ = writer.send(&ClientRequest::Detach).await;
    result
}

async fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    mut reader: ClientReader,
    writer: &mut ClientWriter,
) -> Result<(), TuiError> {
    let mut keys = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    loop {
        let size = terminal.size()?;
        app.compact = size.width < 120 || size.height < 30;
        terminal.draw(|f| ui::draw(f, app))?;
        tokio::select! {
            msg = reader.recv() => match msg {
                Ok(Some(m)) => app.apply(m),
                Ok(None) | Err(_) => {
                    app.connected = false;
                    app.model.notice = Some("daemon connection closed; press q".into());
                }
            },
            ev = keys.next() => match ev {
                Some(Ok(Event::Key(k))) => match app.key(k) {
                    Action::None => {}
                    Action::Send(req) => {
                        if let Err(e) = writer.send(&req).await {
                            app.model.notice = Some(format!("send failed: {e}"));
                        }
                    }
                    Action::Detach => return Ok(()),
                },
                Some(Ok(Event::Resize(_, _))) => {}
                Some(Ok(_)) => {}
                Some(Err(e)) => return Err(TuiError::Io(e)),
                None => return Ok(()),
            },
            _ = tick.tick() => {}
        }
    }
}

/// Print every daemon message as a JSON line: the headless client.
pub async fn headless(socket: &Path, start: Option<ClientRequest>) -> Result<(), TuiError> {
    let mut client = Client::connect(socket).await?;
    client
        .send(&ClientRequest::Subscribe { after: None })
        .await?;
    let mut watching: Option<callgebra_core::RunId> = None;
    if let Some(req) = start {
        client.send(&req).await?;
    }
    while let Some(msg) = client.recv().await? {
        println!("{}", serde_json::to_string(&msg).unwrap_or_default());
        match &msg {
            ServerMessage::RunAccepted { run } if watching.is_none() => watching = Some(*run),
            ServerMessage::RunFinished { run, .. } if watching == Some(*run) => break,
            _ => {}
        }
    }
    let _ = client.send(&ClientRequest::Detach).await;
    Ok(())
}

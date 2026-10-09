//! Terminal UI: one scrolling stream over the daemon protocol.
//!
//! The prompt at the bottom always has focus. A task typed there starts a
//! run on the daemon; `/…` runs a command (`/sql`, `/trace`, `/board`,
//! `/runs`, …). Everything that happens, the model's reply streaming in, its
//! statements and their results, child sessions, the answer, and every
//! command's output, is appended to the stream above. State is a
//! [`model::Model`] folded from the daemon's messages plus the list of
//! [`Entry`] blocks, so the UI is a pure function of received events and
//! renders into a test buffer. [`setup`] is the onboarding wizard, run on
//! the first start and by `/setup`, and [`theme`] the Catppuccin palettes
//! every view draws with.

#![forbid(unsafe_code)]

#[cfg(test)]
mod gallery;
pub mod model;
pub mod setup;
pub mod theme;
pub mod ui;

use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures::StreamExt;
use kleene_core::{ConversationId, RunId, SessionId};
use kleene_daemon::client::{Client, ClientReader, ClientWriter};
use kleene_daemon::{ClientRequest, ServerMessage, StatementOutput};
use kleene_llm::ProviderSettings;
use kleene_trace::TraceEvent;
use model::Model;
use setup::{SetupAction, SetupApp};
use std::path::{Path, PathBuf};
use std::time::Duration;
pub use theme::{Flavor, Theme};

/// TUI errors.
#[derive(Debug, thiserror::Error)]
pub enum TuiError {
    /// Terminal or socket failure.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// The daemon connection failed.
    #[error(transparent)]
    Daemon(#[from] kleene_daemon::DaemonError),
}

/// A slash command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Command {
    /// `/name`.
    pub name: &'static str,
    /// Argument hint.
    pub args: &'static str,
    /// One line of help.
    pub help: &'static str,
}

/// Every command, in the order the popup lists them.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "/help",
        args: "",
        help: "the commands and keys",
    },
    Command {
        name: "/sql",
        args: "<statement>",
        help: "run one CallSQL statement in an interactive session",
    },
    Command {
        name: "/trace",
        args: "<sql>",
        help: "query the store: trace_*, memo, tasks, evals",
    },
    Command {
        name: "/board",
        args: "",
        help: "the continual loop's task board",
    },
    Command {
        name: "/runs",
        args: "",
        help: "live runs on this daemon",
    },
    Command {
        name: "/follow",
        args: "<run>",
        help: "show a run by (the tail of) its id",
    },
    Command {
        name: "/cancel",
        args: "",
        help: "cancel the run being followed",
    },
    Command {
        name: "/plans",
        args: "",
        help: "show or hide EXPLAIN plans under statements",
    },
    Command {
        name: "/theme",
        args: "[flavour]",
        help: "next Catppuccin flavour, or mocha | macchiato | frappé | latte",
    },
    Command {
        name: "/setup",
        args: "",
        help: "add or change API keys: model providers and web search",
    },
    Command {
        name: "/mcp",
        args: "[add <name> <command> [args...] | remove <name>]",
        help: "MCP servers: list them with their tools, or add or remove one in mcp.json",
    },
    Command {
        name: "/skills",
        args: "[name]",
        help: "the loaded skills, or one skill in full",
    },
    Command {
        name: "/new",
        args: "",
        help: "start a new conversation; the next task does not see this one",
    },
    Command {
        name: "/clear",
        args: "",
        help: "clear the stream",
    },
    Command {
        name: "/quit",
        args: "",
        help: "detach; the daemon and its runs keep going",
    },
];

/// One block of the stream.
#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    /// The wordmark and the first hints.
    Welcome,
    /// What the user typed.
    Input(String),
    /// A run, rendered live from the model.
    Run(RunId),
    /// A table: a `/trace` result, the board, the run list.
    Table {
        /// Block title.
        title: String,
        /// Column names.
        columns: Vec<String>,
        /// Rows.
        rows: Vec<Vec<String>>,
    },
    /// Something to notice: an error, a confirmation.
    Notice(String),
    /// The command and key reference.
    Help,
    /// Plain text.
    Text(String),
    /// Rendered statement results (`/sql`).
    Results(Vec<StatementOutput>),
}

/// Everything the UI needs besides the model.
#[derive(Debug, Clone)]
pub struct App {
    /// What the daemon told us.
    pub model: Model,
    /// The stream, oldest first.
    pub entries: Vec<Entry>,
    /// The prompt's text.
    pub input: String,
    /// Earlier inputs, oldest first.
    pub history: Vec<String>,
    /// Position while walking the history with the arrows.
    pub history_pos: Option<usize>,
    /// Highlighted row of the command popup.
    pub completion: usize,
    /// Lines scrolled up from the bottom; 0 follows the newest output.
    pub scroll: usize,
    /// Show `EXPLAIN` under each statement.
    pub show_plans: bool,
    /// The run the header and the prompt describe; `None` means the newest.
    pub follow: Option<RunId>,
    /// Whether we are still connected.
    pub connected: bool,
    /// Colours.
    pub theme: Theme,
    /// Workspace sent with runs started from the prompt.
    pub workspace: String,
    /// The interactive session `/sql` statements go to.
    pub repl_session: SessionId,
    /// The conversation tasks typed at the prompt continue: each run is
    /// told the earlier tasks and answers. `/new` starts another.
    pub conversation: ConversationId,
    /// The stream's width in columns, for rules; set by the event loop.
    pub width: usize,
    /// The setup wizard while `/setup` has it open; it takes every key.
    pub setup: Option<SetupApp>,
    /// Where `/setup` reads and writes the config file.
    pub config_path: PathBuf,
}

impl Default for App {
    fn default() -> Self {
        Self {
            model: Model::default(),
            entries: vec![Entry::Welcome],
            input: String::new(),
            history: Vec::new(),
            history_pos: None,
            completion: 0,
            scroll: 0,
            show_plans: false,
            follow: None,
            connected: true,
            theme: std::env::var("KLEENE_THEME")
                .ok()
                .and_then(|v| Flavor::parse(&v))
                .map(Theme::flavor)
                .unwrap_or_default(),
            workspace: String::new(),
            repl_session: SessionId::new(),
            conversation: ConversationId::new(),
            width: 100,
            setup: None,
            config_path: ProviderSettings::path(),
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

/// The query behind `/board`.
pub fn board_query() -> ClientRequest {
    ClientRequest::Query {
        sql: "SELECT t.generator, ROUND(COALESCE(g.dial, 0.3), 2) AS dial, \
              SUM(CASE WHEN t.status = 'pending' THEN 1 ELSE 0 END) AS pending, \
              SUM(CASE WHEN t.status = 'running' THEN 1 ELSE 0 END) AS running, \
              SUM(CASE WHEN t.status = 'solved' THEN 1 ELSE 0 END) AS solved, \
              SUM(CASE WHEN t.status = 'failed' THEN 1 ELSE 0 END) AS failed, \
              SUM(CASE WHEN t.status = 'needs_review' THEN 1 ELSE 0 END) AS review \
              FROM tasks t LEFT JOIN generator_state g ON g.generator = t.generator \
              GROUP BY t.generator, g.dial ORDER BY t.generator"
            .into(),
        tag: Some("board".into()),
    }
}

impl App {
    /// The run the UI is showing: the one chosen with `/follow`, else the
    /// newest.
    pub fn followed(&self) -> Option<RunId> {
        self.follow.or_else(|| self.model.run_order.last().copied())
    }

    /// Commands matching the input, while it is a bare `/word`.
    pub fn completions(&self) -> Vec<&'static Command> {
        if !self.input.starts_with('/') || self.input.contains(' ') {
            return vec![];
        }
        let prefix = self.input.as_str();
        COMMANDS
            .iter()
            .filter(|c| c.name.starts_with(prefix))
            .collect()
    }

    /// Fold a server message in, appending to the stream where it belongs.
    pub fn apply(&mut self, msg: ServerMessage) {
        match &msg {
            ServerMessage::Event { traced, .. } => {
                if let TraceEvent::RunStarted { run, .. } = &traced.event {
                    if !self.entries.iter().any(|e| e == &Entry::Run(*run)) {
                        self.entries.push(Entry::Run(*run));
                    }
                }
            }
            ServerMessage::Table { columns, rows, tag } => {
                let title = match tag.as_deref() {
                    Some("board") => "board",
                    Some(other) => other,
                    None => "trace",
                };
                if title == "skill" {
                    let body = rows
                        .first()
                        .and_then(|r| r.first())
                        .cloned()
                        .unwrap_or_default();
                    self.entries.push(Entry::Text(body));
                } else {
                    self.entries.push(Entry::Table {
                        title: title.to_string(),
                        columns: columns.clone(),
                        rows: rows.clone(),
                    });
                }
            }
            ServerMessage::Submitted { results, .. } => {
                self.entries.push(Entry::Results(results.clone()));
            }
            ServerMessage::Runs { runs } => {
                let rows: Vec<Vec<String>> = runs
                    .iter()
                    .map(|r| {
                        let node = self.model.runs.get(r);
                        vec![
                            ui::short(r),
                            node.map(|n| n.task.clone()).unwrap_or_default(),
                            node.and_then(|n| n.outcome.clone())
                                .unwrap_or_else(|| "running".into()),
                        ]
                    })
                    .collect();
                self.entries.push(Entry::Table {
                    title: "runs".into(),
                    columns: vec!["run".into(), "task".into(), "state".into()],
                    rows,
                });
            }
            ServerMessage::Error { message } => {
                self.entries.push(Entry::Notice(message.clone()));
            }
            ServerMessage::Ok { message } => {
                self.entries.push(Entry::Notice(message.clone()));
            }
            _ => {}
        }
        self.model.apply(msg);
    }

    /// What the prompt's text asks for. Appends the input to the stream and
    /// clears it; returns the request to send, if any.
    pub fn submit(&mut self) -> Action {
        let text = self.input.trim().to_string();
        if text.is_empty() {
            return Action::None;
        }
        self.input.clear();
        self.completion = 0;
        self.history_pos = None;
        if self.history.last() != Some(&text) {
            self.history.push(text.clone());
        }
        self.scroll = 0;
        if let Some(rest) = text.strip_prefix('/') {
            let (name, arg) = match rest.split_once(char::is_whitespace) {
                Some((n, a)) => (n.trim(), a.trim()),
                None => (rest.trim(), ""),
            };
            return self.command(name, arg, &text);
        }
        self.entries.push(Entry::Input(text.clone()));
        self.follow = None;
        Action::Send(ClientRequest::StartRun {
            task: text,
            workspace: self.workspace.clone(),
            context: None,
            max_turns: None,
            max_depth: None,
            budget_calls: None,
            check: None,
            conversation: Some(self.conversation),
        })
    }

    /// Where `/mcp add` and `/mcp remove` write: `mcp.json` beside the
    /// config file.
    pub fn mcp_path(&self) -> PathBuf {
        self.config_path
            .parent()
            .map(|d| d.join("mcp.json"))
            .unwrap_or_else(|| PathBuf::from("mcp.json"))
    }

    /// `/mcp`, `/mcp add <name> <command> [args...]`, `/mcp remove <name>`.
    fn mcp_command(&mut self, arg: &str) -> Action {
        let words: Vec<&str> = arg.split_whitespace().collect();
        match words.as_slice() {
            [] => Action::Send(ClientRequest::ListMcp),
            ["add", name, command, args @ ..] => {
                let server = kleene_tools::mcp::McpServerConfig {
                    name: (*name).to_string(),
                    command: (*command).to_string(),
                    args: args.iter().map(|a| (*a).to_string()).collect(),
                    env: Default::default(),
                };
                match kleene_tools::mcp::add_server(&self.mcp_path(), &server) {
                    Ok(()) => {
                        self.entries.push(Entry::Notice(format!(
                            "added {name} to {}; reloading",
                            self.mcp_path().display()
                        )));
                        Action::Send(ClientRequest::Reload)
                    }
                    Err(e) => {
                        self.entries
                            .push(Entry::Notice(format!("mcp add failed: {e}")));
                        Action::None
                    }
                }
            }
            ["remove", name] => match kleene_tools::mcp::remove_server(&self.mcp_path(), name) {
                Ok(true) => {
                    self.entries
                        .push(Entry::Notice(format!("removed {name}; reloading")));
                    Action::Send(ClientRequest::Reload)
                }
                Ok(false) => {
                    self.entries.push(Entry::Notice(format!(
                        "no server named {name} in {}",
                        self.mcp_path().display()
                    )));
                    Action::None
                }
                Err(e) => {
                    self.entries
                        .push(Entry::Notice(format!("mcp remove failed: {e}")));
                    Action::None
                }
            },
            _ => {
                self.entries.push(Entry::Notice(
                    "usage: /mcp, /mcp add <name> <command> [args...], /mcp remove <name>".into(),
                ));
                Action::None
            }
        }
    }

    fn command(&mut self, name: &str, arg: &str, typed: &str) -> Action {
        match name {
            "help" | "?" => {
                self.entries.push(Entry::Help);
                Action::None
            }
            "sql" if !arg.is_empty() => {
                self.entries.push(Entry::Input(typed.to_string()));
                Action::Send(ClientRequest::Submit {
                    session: self.repl_session,
                    sql: arg.to_string(),
                })
            }
            "trace" if !arg.is_empty() => {
                self.entries.push(Entry::Input(typed.to_string()));
                Action::Send(ClientRequest::Query {
                    sql: arg.to_string(),
                    tag: None,
                })
            }
            "sql" | "trace" => {
                self.entries
                    .push(Entry::Notice(format!("/{name} needs SQL after it")));
                Action::None
            }
            "board" => {
                self.entries.push(Entry::Input(typed.to_string()));
                Action::Send(board_query())
            }
            "runs" => {
                self.entries.push(Entry::Input(typed.to_string()));
                Action::Send(ClientRequest::ListRuns)
            }
            "skills" => {
                self.entries.push(Entry::Input(typed.to_string()));
                Action::Send(ClientRequest::ListSkills {
                    name: (!arg.is_empty()).then(|| arg.to_string()),
                })
            }
            "mcp" => {
                self.entries.push(Entry::Input(typed.to_string()));
                self.mcp_command(arg)
            }
            "follow" => {
                let found = self
                    .model
                    .run_order
                    .iter()
                    .rev()
                    .find(|r| {
                        let s = r.to_string();
                        s.ends_with(arg) || s.starts_with(arg)
                    })
                    .copied();
                match found {
                    Some(r) if !arg.is_empty() => {
                        self.follow = Some(r);
                        self.entries
                            .push(Entry::Notice(format!("following run {}", ui::short(r))));
                    }
                    _ => self
                        .entries
                        .push(Entry::Notice("no such run; /runs lists them".to_string())),
                }
                Action::None
            }
            "cancel" => match self.followed() {
                Some(run) => Action::Send(ClientRequest::CancelRun { run }),
                None => {
                    self.entries
                        .push(Entry::Notice("no run to cancel".to_string()));
                    Action::None
                }
            },
            "plans" => {
                self.show_plans = !self.show_plans;
                self.entries.push(Entry::Notice(format!(
                    "plans {}",
                    if self.show_plans { "shown" } else { "hidden" }
                )));
                Action::None
            }
            "theme" => {
                self.theme = match Flavor::parse(arg) {
                    Some(f) => Theme::flavor(f),
                    None => self.theme.toggled(),
                };
                self.entries
                    .push(Entry::Notice(format!("theme {}", self.theme.name)));
                Action::None
            }
            "new" => {
                self.conversation = ConversationId::new();
                self.entries.push(Entry::Notice(
                    "new conversation; the next task starts from scratch".to_string(),
                ));
                Action::None
            }
            "clear" => {
                self.entries = vec![Entry::Welcome];
                Action::None
            }
            "setup" => {
                self.entries.push(Entry::Input(typed.to_string()));
                let existing = match ProviderSettings::load_from(&self.config_path) {
                    Ok(s) => s.unwrap_or_default(),
                    Err(e) => {
                        self.entries.push(Entry::Notice(format!(
                            "cannot read the config file: {e}; fix or remove it and try again"
                        )));
                        return Action::None;
                    }
                };
                let mut wizard = SetupApp::new(existing, self.config_path.clone());
                wizard.theme = self.theme;
                self.setup = Some(wizard);
                Action::None
            }
            "quit" | "q" | "exit" | "detach" => Action::Detach,
            _ => {
                self.entries.push(Entry::Notice(format!(
                    "unknown command /{name}; /help lists them"
                )));
                Action::None
            }
        }
    }

    /// Handle a key; returns what to do about it.
    pub fn key(&mut self, key: KeyEvent) -> Action {
        if key.kind != KeyEventKind::Press {
            return Action::None;
        }
        if self.setup.is_some() {
            return self.setup_key(key);
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('c') if ctrl => Action::Detach,
            KeyCode::Char('u') if ctrl => {
                self.input.clear();
                Action::None
            }
            KeyCode::Char('l') if ctrl => {
                self.entries = vec![Entry::Welcome];
                Action::None
            }
            KeyCode::Char('p') if ctrl => {
                self.show_plans = !self.show_plans;
                Action::None
            }
            KeyCode::Char('t') if ctrl => {
                self.theme = self.theme.toggled();
                Action::None
            }
            KeyCode::Char('x') if ctrl => match self.followed() {
                Some(run) => Action::Send(ClientRequest::CancelRun { run }),
                None => Action::None,
            },
            KeyCode::Enter => self.submit(),
            KeyCode::Tab => {
                let matches = self.completions();
                if let Some(c) = matches.get(self.completion % matches.len().max(1)) {
                    self.input = format!("{}{}", c.name, if c.args.is_empty() { "" } else { " " });
                    self.completion = 0;
                }
                Action::None
            }
            KeyCode::Esc => {
                self.input.clear();
                self.completion = 0;
                self.history_pos = None;
                Action::None
            }
            KeyCode::Backspace => {
                self.input.pop();
                self.completion = 0;
                Action::None
            }
            KeyCode::Up => {
                let n = self.completions().len();
                if n > 0 {
                    self.completion = (self.completion + n - 1) % n;
                } else if !self.history.is_empty() {
                    let pos = match self.history_pos {
                        None => self.history.len() - 1,
                        Some(p) => p.saturating_sub(1),
                    };
                    self.history_pos = Some(pos);
                    self.input = self.history[pos].clone();
                }
                Action::None
            }
            KeyCode::Down => {
                let n = self.completions().len();
                if n > 0 {
                    self.completion = (self.completion + 1) % n;
                } else if let Some(p) = self.history_pos {
                    if p + 1 < self.history.len() {
                        self.history_pos = Some(p + 1);
                        self.input = self.history[p + 1].clone();
                    } else {
                        self.history_pos = None;
                        self.input.clear();
                    }
                }
                Action::None
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_add(10);
                Action::None
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_sub(10);
                Action::None
            }
            KeyCode::Home => {
                self.scroll = usize::MAX / 2;
                Action::None
            }
            KeyCode::End => {
                self.scroll = 0;
                Action::None
            }
            KeyCode::Char(c) if !ctrl => {
                self.input.push(c);
                self.completion = 0;
                self.history_pos = None;
                Action::None
            }
            _ => Action::None,
        }
    }
}

impl App {
    /// A key while the wizard is open. Saving writes the config file and
    /// asks the daemon to reload, so the next run uses the new keys.
    fn setup_key(&mut self, key: KeyEvent) -> Action {
        let Some(wizard) = self.setup.as_mut() else {
            return Action::None;
        };
        match wizard.key(key) {
            SetupAction::None => Action::None,
            SetupAction::Cancel => {
                self.theme = wizard.theme;
                self.setup = None;
                self.entries
                    .push(Entry::Notice("setup cancelled; nothing written".into()));
                Action::None
            }
            SetupAction::Save(settings) => {
                self.theme = wizard.theme;
                self.setup = None;
                match settings.save_to(&self.config_path) {
                    Ok(()) => {
                        let mut what: Vec<String> =
                            settings.configured().map(str::to_string).collect();
                        if let Some(w) = settings.web_search_configured() {
                            what.push(format!("web search {}", w.provider_name()));
                        }
                        self.entries.push(Entry::Notice(format!(
                            "wrote {} ({}); asking the daemon to reload",
                            self.config_path.display(),
                            what.join(", ")
                        )));
                        Action::Send(ClientRequest::Reload)
                    }
                    Err(e) => {
                        self.entries
                            .push(Entry::Notice(format!("setup not saved: {e}")));
                        Action::None
                    }
                }
            }
        }
    }
}

/// Run the TUI against a daemon socket until the user detaches or the
/// daemon goes away. `start` optionally starts a run on connect; `workspace`
/// is sent with runs typed into the prompt.
pub async fn run(
    socket: &Path,
    start: Option<ClientRequest>,
    workspace: String,
) -> Result<(), TuiError> {
    let client = Client::connect(socket).await?;
    let generation = client.generation;
    let daemon_version = client.version.clone();
    let (reader, mut writer) = client.into_split();
    writer
        .send(&ClientRequest::Subscribe { after: None })
        .await?;
    let mut app = App {
        workspace,
        ..App::default()
    };
    if let Some(req) = start {
        if let ClientRequest::StartRun { task, .. } = &req {
            app.entries.push(Entry::Input(task.clone()));
        }
        writer.send(&req).await?;
    }
    app.model.generation = Some(generation);
    app.model.daemon_version = daemon_version;
    if let Some(notice) = stale_daemon_notice(&app.model) {
        app.entries.push(Entry::Notice(notice));
    }

    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app, reader, &mut writer).await;
    ratatui::restore();
    let _ = writer.send(&ClientRequest::Detach).await;
    result
}

/// This build's kleene version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// What to tell the user when the daemon on the socket was started by a
/// different kleene than this one: it keeps serving the old code (an older
/// install's daemon outlives `brew upgrade`), so runs look as they did
/// before the upgrade until it is restarted.
pub fn stale_daemon_notice(m: &Model) -> Option<String> {
    let theirs = m.daemon_version.as_deref().unwrap_or("older than 0.2.0");
    if m.daemon_version.as_deref() == Some(VERSION) {
        return None;
    }
    Some(format!(
        "this kleene is {VERSION} but the daemon it connected to is {theirs}; stop it with `pkill -f 'kleene.*daemon'` and open kleene again to run the new version"
    ))
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
        app.width = size.width.saturating_sub(2) as usize;
        terminal.draw(|f| ui::draw(f, app))?;
        tokio::select! {
            msg = reader.recv() => match msg {
                Ok(Some(m)) => app.apply(m),
                Ok(None) | Err(_) => {
                    app.connected = false;
                    app.model.notice = Some("daemon connection closed; ctrl-c to leave".into());
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
                Some(Ok(Event::Paste(text))) => app.input.push_str(&text),
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
    let mut watching: Option<RunId> = None;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn key(app: &mut App, code: KeyCode) -> Action {
        app.key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn type_text(app: &mut App, s: &str) {
        for c in s.chars() {
            key(app, KeyCode::Char(c));
        }
    }

    #[test]
    fn a_task_starts_a_run_and_lands_in_the_stream() {
        let mut app = App {
            workspace: "/ws".into(),
            ..App::default()
        };
        type_text(&mut app, "Sum 1..4");
        match key(&mut app, KeyCode::Enter) {
            Action::Send(ClientRequest::StartRun {
                task, workspace, ..
            }) => {
                assert_eq!(task, "Sum 1..4");
                assert_eq!(workspace, "/ws");
            }
            other => panic!("expected StartRun, got {other:?}"),
        }
        assert!(app.input.is_empty());
        assert_eq!(app.entries.last(), Some(&Entry::Input("Sum 1..4".into())));
        assert_eq!(app.history, vec!["Sum 1..4".to_string()]);
        key(&mut app, KeyCode::Up);
        assert_eq!(app.input, "Sum 1..4", "history walks back");
    }

    #[test]
    fn tasks_share_a_conversation_until_new() {
        let mut app = App::default();
        let conversation = |app: &mut App, task: &str| {
            type_text(app, task);
            match key(app, KeyCode::Enter) {
                Action::Send(ClientRequest::StartRun { conversation, .. }) => conversation,
                other => panic!("expected StartRun, got {other:?}"),
            }
        };
        let first = conversation(&mut app, "How many?");
        let second = conversation(&mut app, "And the sum?");
        assert_eq!(first, Some(app.conversation));
        assert_eq!(first, second, "a follow-up continues the conversation");
        type_text(&mut app, "/new");
        assert_eq!(key(&mut app, KeyCode::Enter), Action::None);
        assert_eq!(
            app.entries.last(),
            Some(&Entry::Notice(
                "new conversation; the next task starts from scratch".into()
            ))
        );
        let third = conversation(&mut app, "Fresh start");
        assert_ne!(third, first, "/new starts another conversation");
    }

    #[test]
    fn commands_complete_and_dispatch() {
        let mut app = App::default();
        type_text(&mut app, "/tr");
        assert_eq!(app.completions().len(), 1);
        key(&mut app, KeyCode::Tab);
        assert_eq!(app.input, "/trace ");
        type_text(&mut app, "SELECT 1");
        assert!(matches!(
            key(&mut app, KeyCode::Enter),
            Action::Send(ClientRequest::Query { sql, tag: None }) if sql == "SELECT 1"
        ));
        type_text(&mut app, "/sql SELECT 2");
        assert!(matches!(
            key(&mut app, KeyCode::Enter),
            Action::Send(ClientRequest::Submit { sql, session }) if sql == "SELECT 2" && session == app.repl_session
        ));
        type_text(&mut app, "/board");
        assert!(matches!(
            key(&mut app, KeyCode::Enter),
            Action::Send(ClientRequest::Query { tag: Some(t), .. }) if t == "board"
        ));
        type_text(&mut app, "/help");
        assert_eq!(key(&mut app, KeyCode::Enter), Action::None);
        assert_eq!(app.entries.last(), Some(&Entry::Help));
        type_text(&mut app, "/theme latte");
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.theme.flavor, Flavor::Latte);
        type_text(&mut app, "/nope");
        key(&mut app, KeyCode::Enter);
        assert!(matches!(app.entries.last(), Some(Entry::Notice(n)) if n.contains("unknown")));
        type_text(&mut app, "/quit");
        assert_eq!(key(&mut app, KeyCode::Enter), Action::Detach);
        // Typing q is text, not detach; ctrl-c is.
        type_text(&mut app, "q");
        assert_eq!(app.input, "q");
        assert_eq!(
            app.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Action::Detach
        );
    }

    #[test]
    fn setup_opens_the_wizard_and_saving_reloads_the_daemon() {
        let dir = std::env::temp_dir().join(format!("kleene-tui-setup-{}", std::process::id()));
        let mut app = App {
            config_path: dir.join("config.toml"),
            ..App::default()
        };
        type_text(&mut app, "/setup");
        assert_eq!(key(&mut app, KeyCode::Enter), Action::None);
        assert!(app.setup.is_some(), "the wizard is open");
        // Keys go to the wizard, not the prompt: Esc on its first page cancels.
        assert_eq!(key(&mut app, KeyCode::Esc), Action::None);
        assert!(app.setup.is_none());
        assert!(matches!(app.entries.last(), Some(Entry::Notice(n)) if n.contains("cancelled")));

        type_text(&mut app, "/setup");
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Char(' ')); // tick Anthropic
        key(&mut app, KeyCode::Enter);
        type_text(&mut app, "sk-ant-secret-1234");
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Enter);
        assert!(app.input.is_empty(), "the prompt saw none of that");
        assert_eq!(
            key(&mut app, KeyCode::Enter),
            Action::Send(ClientRequest::Reload)
        );
        assert!(app.setup.is_none());
        let saved = ProviderSettings::load_from(&dir.join("config.toml"))
            .unwrap()
            .unwrap();
        assert_eq!(
            saved.anthropic.unwrap().api_key.as_deref(),
            Some("sk-ant-secret-1234")
        );
        match app.entries.last() {
            Some(Entry::Notice(n)) => {
                assert!(n.contains("anthropic") && !n.contains("secret"), "{n}")
            }
            other => panic!("expected a notice, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn replies_and_tables_append_to_the_stream() {
        let mut app = App::default();
        app.apply(ServerMessage::Table {
            columns: vec!["a".into()],
            rows: vec![vec!["1".into()]],
            tag: None,
        });
        assert!(matches!(app.entries.last(), Some(Entry::Table { title, .. }) if title == "trace"));
        app.apply(ServerMessage::Error {
            message: "boom".into(),
        });
        assert_eq!(app.entries.last(), Some(&Entry::Notice("boom".into())));
        app.apply(ServerMessage::Submitted {
            session: app.repl_session,
            results: vec![StatementOutput {
                text: "x\n-\n1\n1 row".into(),
                is_error: false,
                is_final: false,
            }],
        });
        assert!(matches!(app.entries.last(), Some(Entry::Results(r)) if r.len() == 1));
        key(&mut app, KeyCode::PageUp);
        assert_eq!(app.scroll, 10);
        key(&mut app, KeyCode::End);
        assert_eq!(app.scroll, 0);
    }
}

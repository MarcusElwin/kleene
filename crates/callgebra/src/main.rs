//! The `callgebra` command.

#![forbid(unsafe_code)]

use clap::{Parser, Subcommand};
use std::io::{BufRead, Write};
use std::path::PathBuf;

/// Relational algebra for recursive model calls.
#[derive(Parser, Debug)]
#[command(name = "callgebra", version, about, long_about = None)]
struct Cli {
    /// Log filter, e.g. `info` or `callgebra=debug`.
    #[arg(long, default_value = "warn", global = true)]
    log: String,
    /// Database file for session tables, memo and trace. Default: `.callgebra/run.duckdb`.
    #[arg(long, global = true)]
    db: Option<PathBuf>,
    /// Workspace root for tools (`files`, `grep`, `shell`, ...). Default: current directory.
    #[arg(long, global = true)]
    workspace: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Run a task to completion: the model writes CallSQL turn by turn until
    /// FINAL. Prints each turn and the final relation.
    Run {
        /// Task text, or `@path` to read it from a file.
        task: String,
        /// A context file loaded as table `ctx(ordinal, text)`, one row per paragraph.
        #[arg(long)]
        context: Option<PathBuf>,
        /// Turn cap for the root session.
        #[arg(long, default_value_t = 30)]
        max_turns: u32,
        /// Deepest child session allowed (0 disables rlm/spawn).
        #[arg(long, default_value_t = 2)]
        max_depth: u32,
        /// Model-call budget for the whole run.
        #[arg(long)]
        budget_calls: Option<u64>,
        /// Dollar budget for the whole run.
        #[arg(long)]
        budget_dollars: Option<f64>,
        /// Print only the final relation.
        #[arg(long, short = 'q')]
        quiet: bool,
    },
    /// Continue a run whose root session stopped at its turn cap or was
    /// interrupted (`callgebra trace "SELECT run, outcome FROM trace_sessions"`).
    Resume {
        /// The run id.
        run: String,
        /// Extra turns to grant.
        #[arg(long, default_value_t = 30)]
        max_turns: u32,
        /// Print only the final relation.
        #[arg(long, short = 'q')]
        quiet: bool,
    },
    /// Interactive CallSQL REPL against the store. Reads statements from
    /// stdin (one per line, or terminated by `;`), prints rendered results.
    Repl {
        /// Execute this SQL instead of reading stdin.
        #[arg(short = 'c', long)]
        command: Option<String>,
    },
    /// Print the call plan for a statement without executing it.
    Explain {
        /// CallSQL text.
        sql: String,
    },
    /// Query the trace database directly with DuckDB SQL.
    Trace {
        /// SQL over the trace tables (`trace_runs`, `trace_sessions`,
        /// `trace_statements`, `trace_calls`, `trace_tool_calls`,
        /// `trace_rounds`, `trace_final`, `memo`).
        sql: String,
    },
    /// Run evaluation task packs and report (M7).
    Bench,
    /// Open the terminal UI over the daemon (starting a daemon in the
    /// background if none is listening).
    Tui {
        /// Start this task on connect (text, or `@path`).
        #[arg(long)]
        run: Option<String>,
        /// Context file for `--run`.
        #[arg(long)]
        context: Option<PathBuf>,
        /// Daemon socket. Default: `.callgebra/daemon.sock`.
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    /// Attach to the daemon and print every event as a JSON line (headless
    /// mode). With `--run`, exits when that run finishes.
    Attach {
        /// Start this task on connect (text, or `@path`).
        #[arg(long)]
        run: Option<String>,
        /// Context file for `--run`.
        #[arg(long)]
        context: Option<PathBuf>,
        /// Daemon socket. Default: `.callgebra/daemon.sock`.
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    /// Start the engine daemon in the foreground.
    Daemon {
        /// Socket path. Default: `.callgebra/daemon.sock`.
        #[arg(long)]
        socket: Option<PathBuf>,
    },
}

fn socket_path(cli: &Cli, socket: &Option<PathBuf>) -> PathBuf {
    socket.clone().unwrap_or_else(|| {
        db_path(cli)
            .parent()
            .map(|p| p.join("daemon.sock"))
            .unwrap_or_else(|| PathBuf::from(".callgebra/daemon.sock"))
    })
}

fn read_task(task: &str) -> anyhow::Result<String> {
    Ok(match task.strip_prefix('@') {
        Some(path) => std::fs::read_to_string(path)?,
        None => task.to_string(),
    })
}

fn start_request(
    run: &Option<String>,
    context: &Option<PathBuf>,
    workspace: &Option<PathBuf>,
) -> anyhow::Result<Option<callgebra_daemon::ClientRequest>> {
    let Some(task) = run else {
        return Ok(None);
    };
    let context = match context {
        Some(p) => Some(std::fs::read_to_string(p)?),
        None => None,
    };
    Ok(Some(callgebra_daemon::ClientRequest::StartRun {
        task: read_task(task)?,
        workspace: workspace
            .as_ref()
            .map(|w| w.display().to_string())
            .unwrap_or_default(),
        context,
        max_turns: None,
        max_depth: None,
        budget_calls: None,
    }))
}

async fn daemon_config(cli: &Cli) -> anyhow::Result<callgebra_harness::HarnessConfig> {
    let workspace = cli
        .workspace
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let provider = match callgebra_llm::provider_from_env() {
        Ok(p) => Some(p),
        Err(e) => {
            eprintln!("callgebra: no model provider ({e}); model calls will fail");
            None
        }
    };
    Ok(callgebra_harness::HarnessConfig {
        workspace,
        provider,
        ..callgebra_harness::HarnessConfig::default()
    })
}

/// Connect to the daemon, spawning one in the background if the socket is
/// not answering.
async fn ensure_daemon(cli: &Cli, socket: &PathBuf) -> anyhow::Result<()> {
    if tokio::net::UnixStream::connect(socket).await.is_ok() {
        return Ok(());
    }
    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--db")
        .arg(db_path(cli))
        .arg("daemon")
        .arg("--socket")
        .arg(socket)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if let Some(w) = &cli.workspace {
        cmd.arg("--workspace").arg(w);
    }
    cmd.spawn()?;
    for _ in 0..100 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        if tokio::net::UnixStream::connect(socket).await.is_ok() {
            return Ok(());
        }
    }
    anyhow::bail!("daemon did not start listening on {}", socket.display())
}

fn db_path(cli: &Cli) -> PathBuf {
    cli.db
        .clone()
        .unwrap_or_else(|| PathBuf::from(".callgebra").join("run.duckdb"))
}

fn open_store(cli: &Cli) -> anyhow::Result<callgebra_store::DuckDbStore> {
    let path = db_path(cli);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(callgebra_store::DuckDbStore::open(&path)?)
}

/// A REPL plus the trace writer to flush before exit.
struct Opened {
    repl: callgebra_harness::Repl,
    trace: callgebra_store::DuckDbTraceSink,
}

/// Prints turns as they happen.
struct Printer {
    quiet: bool,
}

impl callgebra_harness::Observer for Printer {
    fn session_started(&self, meta: &callgebra_harness::SessionMeta, task: &str) {
        if self.quiet {
            return;
        }
        let indent = "  ".repeat(meta.depth as usize);
        eprintln!(
            "{indent}▶ session {} depth {} role {}: {}",
            short(&meta.id.to_string()),
            meta.depth,
            meta.role.name,
            first_line(task)
        );
    }

    fn turn(&self, meta: &callgebra_harness::SessionMeta, turn: &callgebra_harness::Turn) {
        if self.quiet {
            return;
        }
        let indent = "  ".repeat(meta.depth as usize);
        eprintln!(
            "{indent}── turn {} ({}) ──",
            turn.n,
            short(&meta.id.to_string())
        );
        match &turn.sql {
            Some(sql) => {
                for l in sql.lines() {
                    eprintln!("{indent}> {l}");
                }
            }
            None => eprintln!("{indent}> (no SQL) {}", first_line(&turn.reply)),
        }
        for l in turn.feedback.lines() {
            eprintln!("{indent}  {l}");
        }
    }

    fn session_finished(
        &self,
        meta: &callgebra_harness::SessionMeta,
        outcome: &callgebra_harness::Outcome,
        usage: &callgebra_core::BudgetUsage,
    ) {
        if self.quiet {
            return;
        }
        let indent = "  ".repeat(meta.depth as usize);
        eprintln!(
            "{indent}■ session {} ended: {} after {} calls, {} tokens, ${:.4}, {:.1?}",
            short(&meta.id.to_string()),
            outcome.tag(),
            usage.calls,
            usage.tokens,
            usage.dollars,
            usage.wall
        );
    }
}

fn short(id: &str) -> &str {
    &id[..id.len().min(8)]
}

fn first_line(s: &str) -> String {
    let l = s.lines().next().unwrap_or_default();
    if l.chars().count() > 80 {
        format!("{}…", l.chars().take(80).collect::<String>())
    } else {
        l.to_string()
    }
}

async fn open_harness(
    cli: &Cli,
    max_turns: u32,
    max_depth: u32,
    budget: callgebra_core::Budget,
    quiet: bool,
) -> anyhow::Result<(
    std::sync::Arc<callgebra_harness::Harness>,
    callgebra_store::DuckDbTraceSink,
)> {
    let store = open_store(cli)?;
    let workspace = cli
        .workspace
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let provider = match callgebra_llm::provider_from_env() {
        Ok(p) => Some(p),
        Err(e) => {
            eprintln!("callgebra: no model provider ({e}); model calls will fail");
            None
        }
    };
    let trace = store.trace_sink();
    let tracer = Some(callgebra_trace::Tracer::new(std::sync::Arc::new(
        trace.clone(),
    )));
    let cfg = callgebra_harness::HarnessConfig {
        workspace,
        provider,
        tracer,
        max_depth,
        max_turns,
        budget,
        observer: Some(std::sync::Arc::new(Printer { quiet })),
        ..callgebra_harness::HarnessConfig::default()
    };
    let harness = callgebra_harness::Harness::new(store, cfg).await?;
    Ok((harness, trace))
}

fn print_report(report: &callgebra_harness::RunReport) -> i32 {
    println!("run {}", report.run);
    match &report.root.outcome {
        callgebra_harness::Outcome::Final { answer } => {
            print!("{}", answer.render_table(200));
            println!(
                "{} row{} after {} turns, {} calls, {} tokens, ${:.4}",
                answer.len(),
                if answer.len() == 1 { "" } else { "s" },
                report.root.turns,
                report.root.usage.calls,
                report.root.usage.tokens,
                report.root.usage.dollars
            );
            0
        }
        other => {
            println!(
                "no FINAL: {} after {} turns, {} calls, {} tokens, ${:.4}",
                match other {
                    callgebra_harness::Outcome::BudgetExhausted { detail } =>
                        format!("budget exhausted ({detail})"),
                    callgebra_harness::Outcome::TurnsExhausted => format!(
                        "turn cap reached; `callgebra resume {}` continues it",
                        report.run
                    ),
                    callgebra_harness::Outcome::Failed { error } => format!("failed: {error}"),
                    callgebra_harness::Outcome::Cancelled => "cancelled".into(),
                    callgebra_harness::Outcome::Final { .. } => unreachable!(),
                },
                report.root.turns,
                report.root.usage.calls,
                report.root.usage.tokens,
                report.root.usage.dollars
            );
            3
        }
    }
}

async fn open_repl(cli: &Cli) -> anyhow::Result<Opened> {
    let store = open_store(cli)?;
    let workspace = cli
        .workspace
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let provider = match callgebra_llm::provider_from_env() {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::info!("no model provider: {e}");
            None
        }
    };
    let trace = store.trace_sink();
    let tracer = Some(callgebra_trace::Tracer::new(std::sync::Arc::new(
        trace.clone(),
    )));
    let repl = callgebra_harness::Repl::with_config(
        store,
        callgebra_harness::ReplConfig {
            workspace,
            provider,
            tracer,
        },
    )
    .await?;
    Ok(Opened { repl, trace })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(&cli.log))
        .with_writer(std::io::stderr)
        .init();

    match &cli.command {
        None => {
            println!("callgebra {}", env!("CARGO_PKG_VERSION"));
            println!("Relational algebra for recursive model calls. Run `callgebra --help`.");
            Ok(())
        }
        Some(Command::Run {
            task,
            context,
            max_turns,
            max_depth,
            budget_calls,
            budget_dollars,
            quiet,
        }) => {
            let task_text = match task.strip_prefix('@') {
                Some(path) => std::fs::read_to_string(path)?,
                None => task.clone(),
            };
            let context = match context {
                Some(p) => Some(std::fs::read_to_string(p)?),
                None => None,
            };
            let budget = callgebra_core::Budget {
                calls: *budget_calls,
                dollars: *budget_dollars,
                ..callgebra_core::Budget::unbounded()
            };
            let (harness, trace) =
                open_harness(&cli, *max_turns, *max_depth, budget, *quiet).await?;
            let report = harness.run(&task_text, context).await?;
            trace.flush().await;
            let code = print_report(&report);
            if code != 0 {
                std::process::exit(code);
            }
            Ok(())
        }
        Some(Command::Resume {
            run,
            max_turns,
            quiet,
        }) => {
            let run_id: uuid::Uuid = run.parse()?;
            let (harness, trace) = open_harness(
                &cli,
                *max_turns,
                2,
                callgebra_core::Budget::unbounded(),
                *quiet,
            )
            .await?;
            let report = harness.resume(callgebra_core::RunId(run_id)).await?;
            trace.flush().await;
            let code = print_report(&report);
            if code != 0 {
                std::process::exit(code);
            }
            Ok(())
        }
        Some(Command::Repl { command }) => {
            let Opened { repl, trace } = open_repl(&cli).await?;
            let mut stdout = std::io::stdout();
            if let Some(sql) = command {
                let mut failed = false;
                for r in repl.submit(sql).await {
                    writeln!(stdout, "{}", r.text)?;
                    failed |= r.is_error;
                }
                trace.flush().await;
                if failed {
                    std::process::exit(1);
                }
                return Ok(());
            }
            let stdin = std::io::stdin();
            let interactive = atty_stdin();
            let mut buffer = String::new();
            if interactive {
                eprintln!("callgebra repl — CallSQL statements end with ';'. Ctrl-D to quit.");
            }
            loop {
                if interactive {
                    write!(
                        stdout,
                        "{}",
                        if buffer.is_empty() {
                            "callgebra> "
                        } else {
                            "        -> "
                        }
                    )?;
                    stdout.flush()?;
                }
                let mut line = String::new();
                if stdin.lock().read_line(&mut line)? == 0 {
                    break;
                }
                buffer.push_str(&line);
                if buffer.trim_end().ends_with(';') {
                    for r in repl.submit(&buffer).await {
                        writeln!(stdout, "{}", r.text)?;
                    }
                    buffer.clear();
                }
            }
            if !buffer.trim().is_empty() {
                for r in repl.submit(&buffer).await {
                    writeln!(stdout, "{}", r.text)?;
                }
            }
            trace.flush().await;
            Ok(())
        }
        Some(Command::Explain { sql }) => {
            let Opened { repl, trace } = open_repl(&cli).await?;
            let mut failed = false;
            for r in repl.submit(&format!("EXPLAIN {sql}")).await {
                println!("{}", r.text);
                failed |= r.is_error;
            }
            trace.flush().await;
            if failed {
                std::process::exit(1);
            }
            Ok(())
        }
        Some(Command::Trace { sql }) => {
            let store = open_store(&cli)?;
            let batch = store.query(sql).await?;
            print!("{}", batch.render_table(200));
            println!(
                "{} row{}",
                batch.len(),
                if batch.len() == 1 { "" } else { "s" }
            );
            Ok(())
        }
        Some(Command::Daemon { socket }) => {
            let socket = socket_path(&cli, socket);
            let store = open_store(&cli)?;
            let cfg = daemon_config(&cli).await?;
            let daemon = callgebra_daemon::server::Daemon::new(store, cfg).await?;
            eprintln!("callgebra daemon listening on {}", socket.display());
            daemon.serve(&socket).await?;
            Ok(())
        }
        Some(Command::Tui {
            run,
            context,
            socket,
        }) => {
            let socket = socket_path(&cli, socket);
            ensure_daemon(&cli, &socket).await?;
            let start = start_request(run, context, &cli.workspace)?;
            callgebra_tui::run(&socket, start).await?;
            Ok(())
        }
        Some(Command::Attach {
            run,
            context,
            socket,
        }) => {
            let socket = socket_path(&cli, socket);
            ensure_daemon(&cli, &socket).await?;
            let start = start_request(run, context, &cli.workspace)?;
            callgebra_tui::headless(&socket, start).await?;
            Ok(())
        }
        Some(Command::Bench) => {
            eprintln!("callgebra: not implemented yet; arrives in M7, see docs/PLAN.md");
            std::process::exit(2)
        }
    }
}

fn atty_stdin() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal()
}

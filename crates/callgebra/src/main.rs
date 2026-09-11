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
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Run a task to completion (M3).
    Run {
        /// Task text or path to a task file.
        task: String,
    },
    /// Interactive CallSQL REPL against the store. Reads statements from
    /// stdin (one per line, or terminated by `;`), prints rendered results.
    Repl {
        /// Execute this SQL instead of reading stdin.
        #[arg(short = 'c', long)]
        command: Option<String>,
    },
    /// Print the call plan for a statement without executing it (M2).
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
    /// Open the terminal UI (M4).
    Tui,
    /// Start the engine daemon (M4).
    Daemon,
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
        Some(Command::Repl { command }) => {
            let store = open_store(&cli)?;
            let repl = callgebra_harness::Repl::new(store).await?;
            let mut stdout = std::io::stdout();
            if let Some(sql) = command {
                let mut failed = false;
                for r in repl.submit(sql).await {
                    writeln!(stdout, "{}", r.text)?;
                    failed |= r.is_error;
                }
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
        Some(cmd) => {
            let milestone = match cmd {
                Command::Explain { .. } => "M2",
                Command::Run { .. } => "M3",
                Command::Tui | Command::Daemon => "M4",
                Command::Bench => "M7",
                Command::Repl { .. } | Command::Trace { .. } => unreachable!(),
            };
            eprintln!("callgebra: not implemented yet; arrives in {milestone}, see docs/PLAN.md");
            std::process::exit(2)
        }
    }
}

fn atty_stdin() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal()
}

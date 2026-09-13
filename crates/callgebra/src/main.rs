//! The `callgebra` command.

#![forbid(unsafe_code)]

use clap::{Parser, Subcommand};

/// Relational algebra for recursive model calls.
#[derive(Parser, Debug)]
#[command(name = "callgebra", version, about, long_about = None)]
struct Cli {
    /// Log filter, e.g. `info` or `callgebra=debug`.
    #[arg(long, default_value = "info", global = true)]
    log: String,
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
    /// Interactive CallSQL REPL against a workspace (M1).
    Repl,
    /// Print the call plan for a statement without executing it (M2).
    Explain {
        /// CallSQL text.
        sql: String,
    },
    /// Query the trace database (M1).
    Trace {
        /// SQL over the trace tables.
        sql: String,
    },
    /// Run evaluation task packs and report (M7).
    Bench,
    /// Open the terminal UI (M4).
    Tui,
    /// Start the engine daemon (M4).
    Daemon,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(&cli.log))
        .with_writer(std::io::stderr)
        .init();

    match cli.command {
        None => {
            println!("callgebra {}", env!("CARGO_PKG_VERSION"));
            println!("Relational algebra for recursive model calls. Run `callgebra --help`.");
            Ok(())
        }
        Some(cmd) => {
            let milestone = match cmd {
                Command::Repl | Command::Trace { .. } => "M1",
                Command::Explain { .. } => "M2",
                Command::Run { .. } => "M3",
                Command::Tui | Command::Daemon => "M4",
                Command::Bench => "M7",
            };
            eprintln!("callgebra: not implemented yet; arrives in {milestone}, see docs/PLAN.md");
            std::process::exit(2)
        }
    }
}

//! The `kleene` command.

#![forbid(unsafe_code)]

mod pretty;

use clap::{Parser, Subcommand};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::Arc;

/// Relational algebra for recursive model calls.
#[derive(Parser, Debug)]
#[command(name = "kleene", version, about, long_about = None)]
struct Cli {
    /// Log filter, e.g. `info` or `kleene=debug`.
    #[arg(long, default_value = "warn", global = true)]
    log: String,
    /// Database file for session tables, memo and trace. Default: `.kleene/run.duckdb`.
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
    /// interrupted (`kleene trace "SELECT run, outcome FROM trace_sessions"`).
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
    /// Benchmarks: run task packs under learning / frozen / plain modes,
    /// build and import packs, report accuracy, cost and learning curves.
    Bench {
        #[command(subcommand)]
        action: BenchAction,
    },
    /// Open the terminal UI over the daemon (starting a daemon in the
    /// background if none is listening).
    Tui {
        /// Start this task on connect (text, or `@path`).
        #[arg(long)]
        run: Option<String>,
        /// Context file for `--run`.
        #[arg(long)]
        context: Option<PathBuf>,
        /// Daemon socket. Default: `.kleene/daemon.sock`.
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
        /// Daemon socket. Default: `.kleene/daemon.sock`.
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    /// The continual, self-learning loop: generated and user tasks, code
    /// oracles, ratings, curriculum and a replay-gated playbook.
    Learn {
        #[command(subcommand)]
        action: LearnAction,
    },
    /// Configure model providers: a guided wizard in a terminal, or flags.
    /// Writes the config file (`kleene setup --show` prints where).
    Setup {
        /// Anthropic API key (an OAuth token works too).
        #[arg(long)]
        anthropic_key: Option<String>,
        /// Anthropic endpoint override.
        #[arg(long)]
        anthropic_base_url: Option<String>,
        /// OpenAI or compatible API key.
        #[arg(long)]
        openai_key: Option<String>,
        /// OpenAI-compatible endpoint; a local server needs only this.
        #[arg(long)]
        openai_base_url: Option<String>,
        /// The model every alias resolves to without a router.
        #[arg(long)]
        openai_model: Option<String>,
        /// Router TOML with aliases, failover and pricing.
        #[arg(long)]
        router: Option<PathBuf>,
        /// Print the effective settings with keys masked and stop.
        #[arg(long)]
        show: bool,
    },
    /// Start the engine daemon in the foreground.
    Daemon {
        /// Socket path. Default: `.kleene/daemon.sock`.
        #[arg(long)]
        socket: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
enum BenchAction {
    /// Run a pack directory (with pack.json) in one mode and print the results.
    Run {
        /// Pack directory, e.g. tasks/terminal.
        pack: PathBuf,
        /// learning (playbook on), frozen (control), or plain (tool-calling agent baseline).
        #[arg(long, default_value = "learning")]
        mode: String,
        /// Only the first N tasks.
        #[arg(long)]
        limit: Option<usize>,
        /// Record every model response as a fixture under this directory.
        #[arg(long)]
        record: Option<PathBuf>,
        /// Serve model responses from fixtures under this directory (offline).
        #[arg(long)]
        replay: Option<PathBuf>,
    },
    /// Freeze tasks from a generator into a pack directory.
    Build {
        /// Output directory, e.g. tasks/oolong-like.
        out: PathBuf,
        /// Generator: sat3, graph, puzzle, corpus, repo, statements, contracts.
        #[arg(long)]
        from: String,
        /// How many tasks.
        #[arg(long, default_value_t = 20)]
        count: usize,
        /// Hardness dial in [0, 1].
        #[arg(long, default_value_t = 0.5)]
        dial: f64,
        /// First seed.
        #[arg(long, default_value_t = 1)]
        seed: u64,
    },
    /// Write the built-in Terminal-Bench-style pack to a directory.
    Terminal {
        /// Output directory, e.g. tasks/terminal.
        out: PathBuf,
    },
    /// Import a Harvey LAB checkout into a pack (matter folders copied in).
    ImportLab {
        /// LAB root containing task directories with task.json.
        root: PathBuf,
        /// Output directory, e.g. tasks/harvey-lab.
        out: PathBuf,
    },
    /// Accuracy, calls and dollars per pack and mode over every recorded run.
    Report,
    /// The learning curve of one run as a sparkline and rolling mean.
    Curve {
        /// Run id (from `bench run` or the bench_runs table).
        run: String,
        /// Rolling window.
        #[arg(long, default_value_t = 5)]
        window: usize,
    },
    /// Every eval row as CSV on stdout.
    Csv,
}

async fn run_bench(cli: &Cli, action: &BenchAction) -> anyhow::Result<()> {
    use kleene_harness::learn::bench::{sparkline, Mode};
    use kleene_harness::learn::packs::{import_lab, terminal_pack, Pack};
    use kleene_harness::learn::{Learn, LearnConfig};
    match action {
        BenchAction::Build {
            out,
            from,
            count,
            dial,
            seed,
        } => {
            let ws = cli
                .workspace
                .clone()
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
            let name = out
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| from.clone());
            let pack = Pack::from_generator(&name, from, *count, *dial, *seed, &ws)?;
            pack.save(out)?;
            println!(
                "wrote {} tasks to {}",
                pack.tasks.len(),
                out.join("pack.json").display()
            );
            return Ok(());
        }
        BenchAction::Terminal { out } => {
            let pack = terminal_pack();
            pack.save(out)?;
            println!(
                "wrote {} tasks to {}",
                pack.tasks.len(),
                out.join("pack.json").display()
            );
            return Ok(());
        }
        BenchAction::ImportLab { root, out } => {
            let pack = import_lab(root, out)?;
            pack.save(out)?;
            println!(
                "imported {} LAB tasks to {}",
                pack.tasks.len(),
                out.join("pack.json").display()
            );
            return Ok(());
        }
        _ => {}
    }
    let store = open_store(cli)?;
    let mut cfg = daemon_config(cli).await?;
    if let BenchAction::Run { record, replay, .. } = action {
        if let Some(dir) = replay {
            cfg.provider = Some(Arc::new(kleene_llm::ReplayProvider::new(dir.clone())));
        } else if let Some(dir) = record {
            if let Some(inner) = cfg.provider.take() {
                cfg.provider = Some(Arc::new(kleene_llm::RecordingProvider::new(
                    inner,
                    dir.clone(),
                )));
            }
        }
    }
    let learn = Learn::new(store, cfg, LearnConfig::default()).await?;
    match action {
        BenchAction::Run {
            pack, mode, limit, ..
        } => {
            let mode = Mode::parse(mode)
                .ok_or_else(|| anyhow::anyhow!("mode must be learning, frozen or plain"))?;
            let p = Pack::load(pack)?;
            let report = learn.bench(pack, &p, mode, *limit).await?;
            for r in &report.rows {
                println!(
                    "{} {:>3} {:<24} {} calls ${:.4} {}",
                    if r.solved { "✓" } else { "✗" },
                    r.seq + 1,
                    r.task,
                    r.calls,
                    r.dollars,
                    r.detail.lines().next().unwrap_or("")
                );
            }
            println!(
                "run {} · {} · {} · {}/{} solved ({:.0}%) · {} calls · ${:.4}
curve {}",
                report.run,
                report.pack,
                mode.label(),
                report.rows.iter().filter(|r| r.solved).count(),
                report.rows.len(),
                report.accuracy() * 100.0,
                report.calls(),
                report.dollars(),
                sparkline(&report.curve(5))
            );
        }
        BenchAction::Report => print!("{}", learn.bench_summary().await?.render_table(200)),
        BenchAction::Curve { run, window } => {
            let points = learn.bench_curve(run).await?;
            if points.is_empty() {
                anyhow::bail!("no eval rows for run {run}");
            }
            let w = (*window).max(1);
            let curve: Vec<f64> = (0..points.len())
                .map(|i| {
                    let lo = i.saturating_sub(w - 1);
                    let slice = &points[lo..=i];
                    slice.iter().filter(|(_, s)| *s).count() as f64 / slice.len() as f64
                })
                .collect();
            println!("{}", sparkline(&curve));
            for (i, ((seq, solved), c)) in points.iter().zip(&curve).enumerate() {
                println!(
                    "{:>3} {} rolling {:.2}",
                    seq,
                    if *solved { "✓" } else { "✗" },
                    c
                );
                let _ = i;
            }
        }
        BenchAction::Csv => print!("{}", learn.bench_csv().await?),
        BenchAction::Build { .. }
        | BenchAction::Terminal { .. }
        | BenchAction::ImportLab { .. } => {}
    }
    Ok(())
}

#[derive(Subcommand, Debug)]
enum LearnAction {
    /// Run the loop unattended until a limit is hit (state is in the store,
    /// so running again resumes).
    Run {
        /// Stop after this many tasks.
        #[arg(long, default_value_t = 10)]
        tasks: usize,
        /// Stop after spending this much.
        #[arg(long)]
        budget_dollars: Option<f64>,
        /// Stop after this many minutes.
        #[arg(long)]
        minutes: Option<u64>,
        /// Generators to keep fed (comma separated; default all).
        #[arg(long, value_delimiter = ',')]
        generators: Vec<String>,
        /// Pending tasks to keep per generator.
        #[arg(long, default_value_t = 3)]
        queue: usize,
        /// Tasks of a kind replayed to gate a playbook candidate (0 adopts outright).
        #[arg(long, default_value_t = 3)]
        replay: usize,
    },
    /// Add a user task. `--expect` gives an exact expected answer (rows as
    /// `a|b;c|d`); without it the task waits for human review.
    Add {
        /// Task text, or `@path`.
        task: String,
        /// Task kind, for the playbook.
        #[arg(long, default_value = "user")]
        kind: String,
        /// Context file loaded as `ctx`.
        #[arg(long)]
        context: Option<PathBuf>,
        /// Expected rows, `cell|cell;cell|cell`.
        #[arg(long)]
        expect: Option<String>,
        /// A shell oracle: exit 0 on the answer rows (JSON on stdin) means pass.
        #[arg(long)]
        check: Option<String>,
    },
    /// Generate tasks from a generator at its current dial.
    Generate {
        /// sat3, graph, puzzle, corpus or repo.
        generator: String,
        /// How many.
        #[arg(long, default_value_t = 3)]
        count: usize,
    },
    /// Ask the model to propose a task with a rubric (judged by a separate judge call).
    Propose {
        /// Topic.
        topic: String,
    },
    /// Attempt one task now (by id, or the curriculum's pick).
    Step {
        /// Task id.
        id: Option<String>,
    },
    /// The board: counts per generator and status, and each dial.
    Board,
    /// The morning report: solve rate, calls and depth by generator and difficulty.
    Report,
    /// The playbook ledger.
    Playbook,
    /// Withdraw a playbook version.
    Revert {
        /// Version number.
        version: i64,
    },
}

fn parse_expect(spec: &str) -> Vec<Vec<String>> {
    spec.split(';')
        .filter(|r| !r.trim().is_empty())
        .map(|r| r.split('|').map(|c| c.trim().to_string()).collect())
        .collect()
}

async fn run_learn(cli: &Cli, action: &LearnAction) -> anyhow::Result<()> {
    use kleene_harness::learn::verify::Verify;
    use kleene_harness::learn::{Learn, LearnConfig, RunLimits};
    let store = open_store(cli)?;
    let cfg = daemon_config(cli).await?;
    let replay = match action {
        LearnAction::Run { replay, .. } => *replay,
        _ => 3,
    };
    let learn = Learn::new(
        store,
        cfg,
        LearnConfig {
            replay_sample: replay,
            ..LearnConfig::default()
        },
    )
    .await?;
    match action {
        LearnAction::Run {
            tasks,
            budget_dollars,
            minutes,
            generators,
            queue,
            ..
        } => {
            let limits = RunLimits {
                max_tasks: *tasks,
                max_dollars: *budget_dollars,
                max_wall: minutes.map(|m| std::time::Duration::from_secs(m * 60)),
                generators: generators.clone(),
                queue_depth: *queue,
            };
            let reports = learn.run(&limits).await?;
            for r in &reports {
                println!(
                    "{} {} [{}] {} calls ${:.4} {}{}",
                    if r.solved { "✓" } else { "✗" },
                    r.kind,
                    r.generator,
                    r.calls,
                    r.dollars,
                    r.detail,
                    match (r.playbook_candidate, r.adopted) {
                        (Some(v), Some(true)) => format!(" · playbook v{v} adopted"),
                        (Some(v), Some(false)) => format!(" · playbook v{v} rejected"),
                        _ => String::new(),
                    }
                );
            }
            let solved = reports.iter().filter(|r| r.solved).count();
            println!(
                "{} task(s), {} solved, ${:.4}",
                reports.len(),
                solved,
                reports.iter().map(|r| r.dollars).sum::<f64>()
            );
            print!("{}", learn.board().await?.render_table(50));
        }
        LearnAction::Add {
            task,
            kind,
            context,
            expect,
            check,
        } => {
            let context = match context {
                Some(p) => Some(std::fs::read_to_string(p)?),
                None => None,
            };
            let verify = match (expect, check) {
                (Some(e), _) => Some(Verify::Exact {
                    rows: parse_expect(e),
                }),
                (None, Some(c)) => Some(Verify::Shell { command: c.clone() }),
                (None, None) => None,
            };
            let id = learn
                .add_task(kind, &read_task(task)?, context, verify)
                .await?;
            println!("{id}");
        }
        LearnAction::Generate { generator, count } => {
            for id in learn.generate(generator, *count).await? {
                println!("{id}");
            }
        }
        LearnAction::Propose { topic } => println!("{}", learn.propose(topic).await?),
        LearnAction::Step { id } => match learn.step(id.as_deref(), &[]).await? {
            Some(r) => println!("{}", serde_json::to_string_pretty(&r)?),
            None => println!("nothing pending"),
        },
        LearnAction::Board => print!("{}", learn.board().await?.render_table(100)),
        LearnAction::Report => print!("{}", learn.report().await?.render_table(100)),
        LearnAction::Playbook => print!("{}", learn.playbook().await?.render_table(100)),
        LearnAction::Revert { version } => {
            learn.revert(*version).await?;
            println!("reverted playbook v{version}");
        }
    }
    Ok(())
}

fn socket_path(cli: &Cli, socket: &Option<PathBuf>) -> PathBuf {
    socket.clone().unwrap_or_else(|| {
        db_path(cli)
            .parent()
            .map(|p| p.join("daemon.sock"))
            .unwrap_or_else(|| PathBuf::from(".kleene/daemon.sock"))
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
) -> anyhow::Result<Option<kleene_daemon::ClientRequest>> {
    let Some(task) = run else {
        return Ok(None);
    };
    let context = match context {
        Some(p) => Some(std::fs::read_to_string(p)?),
        None => None,
    };
    Ok(Some(kleene_daemon::ClientRequest::StartRun {
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

async fn daemon_config(cli: &Cli) -> anyhow::Result<kleene_harness::HarnessConfig> {
    let workspace = cli
        .workspace
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let provider = match kleene_llm::provider_from_env() {
        Ok(p) => Some(p),
        Err(e) => {
            eprintln!("kleene: no model provider ({e}); model calls will fail");
            None
        }
    };
    Ok(kleene_harness::HarnessConfig {
        workspace,
        provider,
        ..kleene_harness::HarnessConfig::default()
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
        .unwrap_or_else(|| PathBuf::from(".kleene").join("run.duckdb"))
}

fn open_store(cli: &Cli) -> anyhow::Result<kleene_store::DuckDbStore> {
    let path = db_path(cli);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(kleene_store::DuckDbStore::open(&path)?)
}

/// A REPL plus the trace writer to flush before exit.
struct Opened {
    repl: kleene_harness::Repl,
    trace: kleene_store::DuckDbTraceSink,
}

async fn open_harness(
    cli: &Cli,
    max_turns: u32,
    max_depth: u32,
    budget: kleene_core::Budget,
    quiet: bool,
) -> anyhow::Result<(
    std::sync::Arc<kleene_harness::Harness>,
    kleene_store::DuckDbTraceSink,
)> {
    let store = open_store(cli)?;
    let workspace = cli
        .workspace
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    // A first `kleene run` in a terminal gets the wizard instead of a failure.
    onboard_if_needed().await?;
    let provider = match kleene_llm::provider_from_env() {
        Ok(p) => Some(p),
        Err(e) => {
            eprintln!("kleene: no model provider ({e}); model calls will fail");
            None
        }
    };
    let trace = store.trace_sink();
    let tracer = Some(kleene_trace::Tracer::new(std::sync::Arc::new(
        trace.clone(),
    )));
    let cfg = kleene_harness::HarnessConfig {
        workspace,
        provider,
        tracer,
        max_depth,
        max_turns,
        budget,
        observer: Some(std::sync::Arc::new(pretty::Printer::new(quiet))),
        ..kleene_harness::HarnessConfig::default()
    };
    let harness = kleene_harness::Harness::new(store, cfg).await?;
    Ok((harness, trace))
}

async fn open_repl(cli: &Cli) -> anyhow::Result<Opened> {
    let store = open_store(cli)?;
    let workspace = cli
        .workspace
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let provider = match kleene_llm::provider_from_env() {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::info!("no model provider: {e}");
            None
        }
    };
    let trace = store.trace_sink();
    let tracer = Some(kleene_trace::Tracer::new(std::sync::Arc::new(
        trace.clone(),
    )));
    let repl = kleene_harness::Repl::with_config(
        store,
        kleene_harness::ReplConfig {
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
        None if atty_stdin() => {
            // The unified experience: `kleene` alone opens the terminal UI
            // with the prompt bar focused (and the setup wizard first, when
            // no provider is configured).
            if !onboard_if_needed().await? {
                eprintln!(
                    "kleene: no model provider configured; runs will fail until `kleene setup`"
                );
            }
            let socket = socket_path(&cli, &None);
            ensure_daemon(&cli, &socket).await?;
            kleene_tui::run(&socket, None, workspace_string(&cli)).await?;
            Ok(())
        }
        None => {
            println!("kleene {}", env!("CARGO_PKG_VERSION"));
            println!("Relational algebra for recursive model calls. Run `kleene --help`.");
            Ok(())
        }
        Some(Command::Setup {
            anthropic_key,
            anthropic_base_url,
            openai_key,
            openai_base_url,
            openai_model,
            router,
            show,
        }) => {
            run_setup(SetupArgs {
                anthropic_key: anthropic_key.clone(),
                anthropic_base_url: anthropic_base_url.clone(),
                openai_key: openai_key.clone(),
                openai_base_url: openai_base_url.clone(),
                openai_model: openai_model.clone(),
                router: router.clone(),
                show: *show,
            })
            .await
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
            let budget = kleene_core::Budget {
                calls: *budget_calls,
                dollars: *budget_dollars,
                ..kleene_core::Budget::unbounded()
            };
            let (harness, trace) =
                open_harness(&cli, *max_turns, *max_depth, budget, *quiet).await?;
            let report = harness.run(&task_text, context).await?;
            trace.flush().await;
            let code = pretty::report(&report);
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
                kleene_core::Budget::unbounded(),
                *quiet,
            )
            .await?;
            let report = harness.resume(kleene_core::RunId(run_id)).await?;
            trace.flush().await;
            let code = pretty::report(&report);
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
                eprintln!("kleene repl — CallSQL statements end with ';'. Ctrl-D to quit.");
            }
            loop {
                if interactive {
                    write!(
                        stdout,
                        "{}",
                        if buffer.is_empty() {
                            "kleene> "
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
            let daemon = kleene_daemon::server::Daemon::new(store, cfg).await?;
            eprintln!("kleene daemon listening on {}", socket.display());
            daemon.serve(&socket).await?;
            Ok(())
        }
        Some(Command::Tui {
            run,
            context,
            socket,
        }) => {
            // First start with nothing configured: run the wizard before the
            // daemon comes up, so the daemon reads the keys it writes.
            if !onboard_if_needed().await? {
                eprintln!(
                    "kleene: no model provider configured; runs will fail until `kleene setup`"
                );
            }
            let socket = socket_path(&cli, socket);
            ensure_daemon(&cli, &socket).await?;
            let start = start_request(run, context, &cli.workspace)?;
            kleene_tui::run(&socket, start, workspace_string(&cli)).await?;
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
            kleene_tui::headless(&socket, start).await?;
            Ok(())
        }
        Some(Command::Learn { action }) => run_learn(&cli, action).await,
        Some(Command::Bench { action }) => run_bench(&cli, action).await,
    }
}

/// The workspace as the daemon protocol wants it: a path string, empty for
/// the daemon's default.
fn workspace_string(cli: &Cli) -> String {
    cli.workspace
        .as_ref()
        .map(|w| w.display().to_string())
        .unwrap_or_default()
}

fn atty_stdin() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal()
}

/// Flags of `kleene setup`.
struct SetupArgs {
    anthropic_key: Option<String>,
    anthropic_base_url: Option<String>,
    openai_key: Option<String>,
    openai_base_url: Option<String>,
    openai_model: Option<String>,
    router: Option<PathBuf>,
    show: bool,
}

impl SetupArgs {
    fn any_flag(&self) -> bool {
        self.anthropic_key.is_some()
            || self.anthropic_base_url.is_some()
            || self.openai_key.is_some()
            || self.openai_base_url.is_some()
            || self.openai_model.is_some()
            || self.router.is_some()
    }
}

/// What is configured, keys masked, and where each part came from.
fn describe_settings() -> anyhow::Result<String> {
    use kleene_llm::ProviderSettings;
    use kleene_tui::setup::mask;
    let path = ProviderSettings::path();
    let file = ProviderSettings::load()?;
    let env = ProviderSettings::from_env();
    let effective = file.clone().unwrap_or_default().overlaid(env.clone());
    let mut out = String::new();
    out.push_str(&format!(
        "config file  {}{}\n",
        path.display(),
        if file.is_some() { "" } else { " (absent)" }
    ));
    let source = |from_env: bool| if from_env { "environment" } else { "file" };
    match &effective.anthropic {
        Some(a) if a.is_configured() => {
            let cred = a
                .api_key
                .as_deref()
                .or(a.auth_token.as_deref())
                .unwrap_or("");
            let from_env = env
                .anthropic
                .as_ref()
                .is_some_and(|e| e.api_key.is_some() || e.auth_token.is_some());
            out.push_str(&format!(
                "anthropic    {} ({}){}\n",
                mask(cred),
                source(from_env),
                a.base_url
                    .as_deref()
                    .map(|u| format!("  {u}"))
                    .unwrap_or_default()
            ));
        }
        _ => out.push_str("anthropic    not configured\n"),
    }
    match &effective.openai_compat {
        Some(o) if o.is_configured() => {
            let from_env = env
                .openai_compat
                .as_ref()
                .is_some_and(|e| e.api_key.is_some() || e.base_url.is_some());
            out.push_str(&format!(
                "openai       {}  key {}  model {} ({})\n",
                o.base_url.as_deref().unwrap_or("https://api.openai.com/v1"),
                o.api_key
                    .as_deref()
                    .map(mask)
                    .unwrap_or_else(|| "none".into()),
                o.model
                    .as_deref()
                    .unwrap_or(kleene_llm::env::DEFAULT_OPENAI_MODEL),
                source(from_env)
            ));
        }
        _ => out.push_str("openai       not configured\n"),
    }
    match &effective.router {
        Some(r) => out.push_str(&format!(
            "router       {} ({})\n",
            r.display(),
            source(env.router.is_some())
        )),
        None => out.push_str("router       defaults for the configured provider\n"),
    }
    if !effective.is_configured() {
        out.push_str("\nNothing usable yet: run `kleene setup`.\n");
    }
    Ok(out)
}

async fn run_setup(args: SetupArgs) -> anyhow::Result<()> {
    use kleene_llm::ProviderSettings;
    if args.show {
        print!("{}", describe_settings()?);
        return Ok(());
    }
    let existing = ProviderSettings::load()?.unwrap_or_default();
    if args.any_flag() {
        let mut s = existing;
        if args.anthropic_key.is_some() || args.anthropic_base_url.is_some() {
            let mut a = s.anthropic.take().unwrap_or_default();
            if let Some(k) = args.anthropic_key {
                if k.starts_with("sk-") {
                    a.api_key = Some(k);
                    a.auth_token = None;
                } else {
                    a.auth_token = Some(k);
                    a.api_key = None;
                }
            }
            if let Some(u) = args.anthropic_base_url {
                a.base_url = Some(u);
            }
            s.anthropic = Some(a);
        }
        if args.openai_key.is_some()
            || args.openai_base_url.is_some()
            || args.openai_model.is_some()
        {
            let mut o = s.openai_compat.take().unwrap_or_default();
            if let Some(k) = args.openai_key {
                o.api_key = Some(k);
            }
            if let Some(u) = args.openai_base_url {
                o.base_url = Some(u);
            }
            if let Some(m) = args.openai_model {
                o.model = Some(m);
            }
            s.openai_compat = Some(o);
        }
        if let Some(r) = args.router {
            s.router = Some(r);
        }
        let path = s.save()?;
        eprintln!("kleene: wrote {}", path.display());
        print!("{}", describe_settings()?);
        return Ok(());
    }
    if !atty_stdin() {
        anyhow::bail!(
            "kleene setup needs a terminal for the wizard; pass flags instead (see `kleene setup --help`)"
        );
    }
    let path = ProviderSettings::path();
    match kleene_tui::setup::run(existing, path).await? {
        Some(s) => {
            let path = s.save()?;
            eprintln!("kleene: wrote {}", path.display());
            print!("{}", describe_settings()?);
            Ok(())
        }
        None => {
            eprintln!("kleene: setup cancelled, nothing written");
            Ok(())
        }
    }
}

/// Offer the wizard when nothing is configured and we are in a terminal.
/// Returns whether a provider is configured afterwards.
async fn onboard_if_needed() -> anyhow::Result<bool> {
    use kleene_llm::ProviderSettings;
    let effective = ProviderSettings::effective()?;
    if effective.is_configured() {
        return Ok(true);
    }
    if !atty_stdin() {
        return Ok(false);
    }
    let existing = ProviderSettings::load()?.unwrap_or_default();
    match kleene_tui::setup::run(existing, ProviderSettings::path()).await? {
        Some(s) => {
            let path = s.save()?;
            eprintln!("kleene: wrote {}", path.display());
            Ok(true)
        }
        None => Ok(false),
    }
}

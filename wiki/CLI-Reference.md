# CLI reference

One binary, `kleene`. `kleene --help` and `kleene <command> --help` print the flags as compiled; the long reference with every flag is [`docs/CLI.md`](https://github.com/MarcusElwin/kleene/blob/main/docs/CLI.md).

## Global flags

| Flag | Meaning | Default |
|---|---|---|
| `--db <path>` | database file for session tables, memo and trace | `.kleene/run.duckdb` |
| `--workspace <dir>` | root for the workspace tools (`files`, `grep`, `shell`, ...) | current directory |
| `--log <filter>` | log filter, e.g. `info` or `kleene=debug` | `warn` |

## Commands

| Command | Does |
|---|---|
| `kleene` | the terminal UI: one stream, a prompt, slash commands; type a task to run it |
| `kleene setup` | pick providers and store their keys; `--show` prints what is configured |
| `kleene run <task>` | drive a model through the SQL turn loop to `FINAL`, streamed to the terminal. `@path` reads the task from a file; `--context <file>` loads `ctx`; `--max-turns`, `--max-depth`, `--budget-calls`, `--budget-dollars` bound it; `--check <cmd>` refuses `FINAL` while the command fails |
| `kleene resume <run-id>` | continue a run that hit its turn cap or was interrupted |
| `kleene repl [-c SQL]` | CallSQL against the store, interactive or scripted (`kleene repl < file.sql`) |
| `kleene explain <sql>` | the call plan and its cost, without executing; needs no provider |
| `kleene trace <sql>` | DuckDB SQL over the trace, memo and session tables |
| `kleene tui [--run <task>]` | the same UI over the engine daemon, optionally starting a run on connect |
| `kleene attach [--run <task>]` | the same, headless: every event as a JSON line |
| `kleene daemon` | the engine as a server on a Unix socket (`.kleene/daemon.sock`) |
| `kleene learn run\|board\|report\|playbook\|refine\|revert` | the continual loop: tasks, oracles, ratings, curriculum, replay-gated playbook |
| `kleene bench run\|report\|results\|plot\|build\|import-oolong\|import-lab` | task packs under learning, frozen and plain-agent modes |
| `kleene mcp [add\|remove]` | MCP servers: list them with their tools, add or remove one in `mcp.json` |
| `kleene skills [name]` | the skills sessions can read, built in, per user and per project |

## The TUI

`kleene` with no arguments connects to the daemon (starting one if none listens) and shows one scrolling stream: each turn with its calls and cost, the reply as it streams, every statement's result, child sessions nested one level in, and the answer as a `FINAL` block.

| Command | Does |
|---|---|
| `/sql <statement>` | run one CallSQL statement yourself in an interactive session |
| `/trace <sql>` | query the store: `trace_*`, `memo`, `tasks`, `evals` |
| `/board` | the continual loop's task board |
| `/runs`, `/follow <run>`, `/cancel` | live runs on this daemon; follow one by (the tail of) its id; cancel the one being followed |
| `/plans` | show or hide `EXPLAIN` plans under statements (also `Ctrl-P`) |
| `/theme [flavour]` | next Catppuccin flavour, or `mocha`, `macchiato`, `frappé`, `latte` (also `Ctrl-T`) |
| `/setup` | add or change API keys; the daemon reloads them |
| `/mcp [add <name> <cmd> ...]` | the MCP servers and their tools; add or remove one |
| `/skills [name]` | the loaded skills, or one in full |
| `/clear`, `/quit` | clear the stream (`Ctrl-L`); detach, the daemon and its runs keep going (`Ctrl-C`) |

`Tab` completes a command, `Up`/`Down` walk the input history, `PageUp`/`PageDown` scroll the stream, `Ctrl-X` cancels the run being followed. Runs started against the same daemon from another client appear in the stream too.

## Files it writes

| Path | Holds |
|---|---|
| `.kleene/run.duckdb` | every table, the memo, the trace, sessions, learning state and bench results for this directory |
| `.kleene/daemon.sock` | the daemon's socket |
| `~/.config/kleene/config.toml` | provider and web search keys written by `kleene setup` |
| `mcp.json` | MCP servers added with `kleene mcp add` |

## Troubleshooting

- **`no model provider configured`**: run `kleene setup`, or set `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, `OPENAI_API_KEY` or `OPENAI_BASE_URL`. `repl`, `explain` and `trace` work without one as long as the statement makes no calls.
- **Estimates and dollars are all zero**: the model is not in the pricing table. Only the Anthropic defaults come priced; add a `[pricing."model"]` section to your router file.
- **A statement is refused with its plan**: the estimate exceeded the remaining call or dollar budget. Raise `--budget-*` or narrow the query; `EXPLAIN` shows where the calls go.
- **The TUI shows nothing**: a stale socket from a killed daemon. Remove `.kleene/daemon.sock` or pass a fresh `--socket`.
- **Building takes forever or fills the disk**: DuckDB compiling from source, once per build profile. Keep `target/` between runs.

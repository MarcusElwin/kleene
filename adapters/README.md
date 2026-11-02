# Adapters: running kleene inside other benchmark harnesses

The packs under `tasks/` and `kleene bench` measure kleene against itself:
learning against frozen against plain, one model against another. An
adapter runs kleene as one more agent inside somebody else's harness, so a
public leaderboard can grade it on their tasks with their tests.

## Harbor and Terminal-Bench 2.0

[Harbor](https://github.com/laude-institute/harbor) runs
[Terminal-Bench 2.0](https://www.tbench.ai): 89 tasks, each a Docker image
with an instruction and a hidden test script, graded by what the agent
leaves on the machine. An "installed agent" is a CLI that Harbor installs in
the task's container and runs once with the instruction;
`kleene_harbor.py` makes `kleene` one.

### Requirements

- Docker (Harbor builds and runs each task's image; `-e daytona`, `-e modal`
  and the other cloud sandboxes work too and need no local daemon).
- `uv tool install harbor` (Harbor 0.25 or later; the adapter is tested
  against 0.25.0).
- A model key in the environment: `ANTHROPIC_API_KEY` for
  `-m anthropic/<model>`, `OPENAI_API_KEY` for `-m openai/<model>`.
  kleene speaks these two wire formats; Harbor's other providers are refused
  with a message.

### Run

From a checkout of this repository (the adapter is imported by module path,
so `PYTHONPATH` must include the checkout):

```bash
export ANTHROPIC_API_KEY=sk-ant-...
PYTHONPATH=. harbor run -d terminal-bench@2.0 \
  -a adapters.kleene_harbor:KleeneAgent \
  -m anthropic/claude-haiku-4-5 \
  --ak budget_dollars=1 \
  -n 4 -o jobs
```

One task, for a smoke test:

```bash
PYTHONPATH=. harbor run -d terminal-bench@2.0 -t regex-log \
  -a adapters.kleene_harbor:KleeneAgent -m anthropic/claude-haiku-4-5
```

Harbor writes `jobs/<job>/` with one directory per trial; the agent's files
are under `agent/`:

| File | What |
|---|---|
| `kleene.txt` | kleene's stderr: the run summary line (turns, calls, tokens, dollars) and log lines at the `--ak log=` level; the turn-by-turn transcript is in the store, `kleene --db agent/kleene.duckdb trace "SELECT sql, rows, error FROM trace_statements ORDER BY started_at"` |
| `result.json` | stdout of `kleene run --json`: the `FINAL` rows, or `{"run", "outcome", "detail"}` when the session ended without one |
| `exit_code` | `0` after a `FINAL`, `3` without one (turns or budget exhausted), anything else is a kleene failure |
| `kleene.duckdb` | the store: `trace_*` tables, memo and the model's own tables, queryable with `kleene --db ... trace "<sql>"` |
| `usage.json` | the row `kleene trace --json` printed over `trace_calls`; Harbor reads tokens and dollars from it |
| `router.toml`, `instruction.txt` | what the run was given |

Harbor's own `result.json` at the trial root carries the verifier's reward
and the agent's `n_input_tokens`, `n_cache_tokens`, `n_output_tokens` and
`cost_usd`; `harbor view` browses them.

### Options

`--ak key=value`, repeatable. `harbor agent schema adapters.kleene_harbor:KleeneAgent`
prints the same list.

| Option | Default | Meaning |
|---|---|---|
| `max_turns` | `60` | `kleene run --max-turns`, the root session's turn cap |
| `max_depth` | `2` | `kleene run --max-depth`; `0` disables `rlm` and `spawn` |
| `budget_dollars` | none | `kleene run --budget-dollars` for the whole run, children included; a statement whose estimate exceeds what is left is refused |
| `budget_calls` | none | `kleene run --budget-calls` |
| `workspace` | `/app` | the directory the tools are confined to and `shell` runs in; every Terminal-Bench 2.0 task but a handful uses `/app` |
| `check` | none | `kleene run --check`, a command that must exit 0 before `FINAL` is accepted |
| `version` | latest release | the release tag `install.sh` installs, e.g. `v0.5.0` |
| `binary` | none | a local `kleene` binary to upload instead of installing a release (test an unreleased build) |
| `log` | `info` | `kleene --log` filter for `kleene.txt` |
| `reasoning_effort` | none | Harbor's `--effort`; written as `effort` into every alias |

### What the adapter does

1. **Install** (`install()`): `curl ... install.sh | sh` as root with
   `KLEENE_INSTALL=/usr/local/bin`, or `upload_file` of `--ak binary=...`.
   The released Linux binary is built on Ubuntu 22.04 and links glibc 2.35,
   so it runs on every Terminal-Bench 2.0 image but the two Debian bullseye
   ones (`qemu-alpine-ssh`, `qemu-startup`), which fail at install and
   count as unsolved.
2. **Route**: a router TOML that resolves `root`, `worker`, `proxy` and
   `judge` to the `-m` model, so a run advertised as one model is that model
   throughout, child sessions included. `KLEENE_ROUTER_TOML` points at it and
   `KLEENE_CONFIG_DIR` at an empty directory, so nothing in the image's
   `~/.config/kleene` leaks in.
3. **Run**: the instruction plus a short suffix (how a kleene session ends:
   do the work with `shell`, `write_file` and `patch`, verify, then `FINAL`
   over one row `done = true, note`) is written to a file and
   `kleene run @file --json` runs in the workspace with the options above.
   A Jinja template given with `--ak prompt_template_path=...` replaces the
   suffix. Exit code 3 (no `FINAL`) is a result for the verifier, not an
   agent error; any other non-zero exit is.
4. **Account**: `kleene trace --json` sums `trace_calls` into `usage.json`,
   and `populate_context_post_run` copies it into Harbor's `AgentContext`
   (memo hits and the `FINAL` rows go into `metadata`).

### Tests

```bash
uv run --no-project --python "$(uv tool dir)/harbor/bin/python" \
  --with pytest --with pytest-asyncio -m pytest adapters -q
```

A fake environment records the commands the adapter would run; no Docker
and no model. The end-to-end check is the one-task run above.

# Quickstart

## 1. Point it at a model

The wizard asks which providers to use and stores the keys owner-readable under `~/.config/kleene/`. Environment variables win over the file when both are set.

```bash
kleene setup                          # Anthropic, OpenAI, or a compatible endpoint (Ollama, vLLM, a gateway);
                                      # optionally Brave, Tavily, Exa or Linkup for web_search
export ANTHROPIC_API_KEY=sk-ant-...   # or just the environment
export OPENAI_API_KEY=sk-...          # or any OpenAI-compatible endpoint (OPENAI_BASE_URL, OPENAI_MODEL)
```

Details and routing: [[Configuration]].

## 2. Run it

`kleene` alone opens the terminal UI: one scrolling stream with a prompt. Type a task, press Enter, and watch the model's reply stream in, its SQL run and the answer arrive. The first thing worth typing is `/setup` if no key is configured yet.

```text
$ kleene
> Which project consumed the most hours in total? The notes are in demos/oolong/corpus.txt
```

Or drive it from the shell:

```bash
kleene run @demos/oolong/task.txt --context demos/oolong/corpus.txt --budget-calls 60
kleene trace "SELECT depth, role, outcome, turns, calls, dollars FROM trace_sessions ORDER BY started_at"
```

`kleene run` streams the reply as it is written, highlights the SQL, shows a spinner while statements execute and prints the results and the final relation as tables.

## 3. What happened

The model receives the context as a table `ctx(ordinal, text)`, one row per paragraph, and writes CallSQL turn by turn: it can `SELECT` over the context, define prompt functions, call tools with `CALL`, delegate with `rlm(...)` and `spawn(...)`, and finish with `FINAL`. Every model call is memoised, so running the same task again is free. Everything lands in `.kleene/run.duckdb` under the current directory.

From the TUI prompt, the same engine answers questions about the run:

```text
> /trace SELECT depth, role, outcome, turns, calls, dollars FROM trace_sessions
> /sql   SELECT COUNT(*) FROM hours
> /plans
```

## More to try

| Demo | Command |
|---|---|
| Long context: partition and map `rlm` over sixty meeting notes | `kleene run @demos/oolong/task.txt --context demos/oolong/corpus.txt --budget-calls 60` |
| Repository question with read-only reviewer agents | `kleene run @demos/repo-review/task.txt --workspace . --max-depth 1` |
| The planner without spending anything | `kleene explain "SELECT c FROM candidates WHERE llm_bool('Is ' \|\| c \|\| ' a real place?')"` |
| Join ordering, a proxy cascade and a beam | `kleene repl < demos/planner/three_way.sql`, `kleene repl < demos/planner/cascade_and_beam.sql` |
| Watch a run live | `kleene tui --run @demos/oolong/task.txt --context demos/oolong/corpus.txt` |
| Resume a run that hit its turn cap | `kleene resume <run-id> --max-turns 10` |
| Let it learn overnight and measure it | `kleene learn run --tasks 20 --generators puzzle,corpus`, then `kleene learn report` |

The demos are described in [`demos/README.md`](https://github.com/MarcusElwin/kleene/blob/main/demos/README.md). Next: [[CallSQL Cheat Sheet]].

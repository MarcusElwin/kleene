# FAQ

**Why SQL?** Because the things an agent loop does badly by hand (map a prompt over a thousand rows, dedupe identical calls, stop at the first counterexample, pick the cheap check before the expensive one, bound a search) are what a relational planner does for a living. Writing the plan as SQL makes the calls visible to an optimizer and the cost visible to you before anything is spent.

**What is CallSQL?** The PostgreSQL-flavoured subset Kleene executes, plus model calls (`llm`, `llm_bool`, `llm_json`, prompt-defined functions), tools (`CALL`), delegation (`rlm`, `spawn`, `CREATE AGENT`), budgets (`SET budget.*`) and `FINAL`. See [[CallSQL Cheat Sheet]].

**What is an RLM?** A recursive language model: a session that can open child sessions with a slice of its budget, each the same loop one level deeper. In Kleene that is `rlm(question, context)` in a `CROSS JOIN LATERAL`, one child per input row, and `spawn('role', task)` for a declared agent. Kleene uses recursion where other harnesses compact the context.

**Which models work?** Anything that speaks the Anthropic Messages API or the OpenAI-compatible chat API: Anthropic, OpenAI, Ollama, vLLM, LM Studio, gateways. There are no SDKs; the adapters speak the wire format over HTTPS. See [[Configuration]].

**Does it need an API key to try?** Not for the engine. `kleene repl`, `kleene explain` and `kleene trace` work with no provider as long as the statement makes no model calls, and `EXPLAIN` prices a plan without running it. A task run needs one key.

**Why does a run that failed cost nothing to re-run?** Every model call is memoised in DuckDB by model and prompt fingerprint, across sessions and runs. `SET memo = off` bypasses it.

**What does "refused with its plan" mean?** The planner estimated more calls or dollars than the remaining budget allows, so the statement did not run. The model sees the plan and the estimate and writes a narrower one. Raise `--budget-calls` or `--budget-dollars` if the estimate is what you meant to spend.

**Why are my dollar figures zero?** Only the Anthropic defaults come priced. On an OpenAI-compatible endpoint, add a `[pricing."model"]` section to a router file.

**Where does everything live?** In one DuckDB file, `.kleene/run.duckdb` under the current directory: tables, memo, trace, sessions, learning state and benchmark results. Query it with `kleene trace` or `/trace` in the TUI.

**Can it edit code?** Yes. The workspace tools (`read`, `lines`, `grep`, `search`, `patch`, `write_file`, `shell`) are jailed to `--workspace`, `--check <cmd>` refuses `FINAL` while the tests fail, and the `coding` benchmark pack measures it. It is early: see the [[Benchmarks]] page for how it does.

**Does it learn?** Without weight updates. `kleene learn` keeps a task stream with code oracles, rates tasks and solver configurations with Bradley-Terry, and keeps SQL that solved a task as a playbook entry only after a replay eval shows it wins on solves and cost. Playbook entries are rows you can inspect and revert.

**Why "Kleene"?** For Stephen Cole Kleene: the Kleene star (closure under repetition, what a recursive CTE computes), the fixed-point theorem (semi-naive evaluation) and the recursion theorem (a program that refers to itself, which is what `rlm` does). The long version is in the [README](https://github.com/MarcusElwin/kleene#why-kleene).

**How is it pronounced?** "KLAY-nee", as Kleene himself said it.

**Is it production ready?** No. It is pre-release and the benchmarks are honest about the gaps; see [`docs/WRITEUP.md`, "Honest gaps"](https://github.com/MarcusElwin/kleene/blob/main/docs/WRITEUP.md#5-honest-gaps).

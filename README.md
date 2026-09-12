# callgebra

**Callgebra — relational algebra for recursive model calls.**

Write declarative SQL. Compile joins, recursion, predicates and aggregation
into an execution graph of language-model calls, recursive sub-sessions and
tool calls. Plan and cost that graph, run it, and query the trace with the
same SQL. Watch it happen in a terminal UI.

```sql
SELECT candidate
FROM possibilities
WHERE VERIFY(candidate)
  AND NOT EXISTS (
    SELECT 1 FROM counterexamples ce
    WHERE REFUTE(candidate, ce.text)
  );
```

`VERIFY` is one model call per distinct candidate. The `NOT EXISTS` is an
anti-semi-join that stops on the first refuting counterexample. `EXPLAIN`
tells you how many calls that is before you spend them.

Status: **M7 benchmarks**. The CallSQL frontend, the executor, the
DuckDB store and the call algebra work; model functions, prompt-defined
functions, `CALL` tools, memo, budgets and `EXPLAIN` are in; `callgebra run`
drives a model through the SQL turn loop to `FINAL`, spawning child sessions
for `rlm(...)` and `spawn(...)`; `callgebra tui` watches it all live over
the engine daemon (call tree, transcript, plan, trace explorer, cancel,
detach); the planner orders joins around call predicates, cascades
oracles through declared proxies, costs beam-limited recursion and refuses
statements the remaining budget cannot pay for; and `callgebra learn` runs
the continual loop: generated and user tasks with code oracles, ratings and
a curriculum, and a playbook of winning SQL adopted only after it wins a
replay eval; and `callgebra bench` runs task packs under learning, frozen and
plain-agent modes and records every task in `evals`. Try it without a model:

```bash
cargo run -- repl -c "WITH RECURSIVE r(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM r WHERE n < 5) SELECT SUM(n) FROM r"
cargo run -- explain "SELECT c FROM candidates WHERE llm_bool('Is ' || c || ' a real place?')"
cargo run -- trace "SELECT * FROM trace_statements"
```

and with one (`ANTHROPIC_API_KEY` or `OPENAI_API_KEY`):

```bash
cargo run -- run @demos/oolong/task.txt --context demos/oolong/corpus.txt --budget-calls 60
cargo run -- tui --run @demos/oolong/task.txt --context demos/oolong/corpus.txt
cargo run -- trace "SELECT depth, role, outcome, turns, calls FROM trace_sessions"
cargo run -- learn run --tasks 20 --generators puzzle,corpus   # overnight, resumable
cargo run -- learn report                                       # the morning query
cargo run -- bench run tasks/terminal --mode frozen             # a pack, one mode
cargo run -- bench report                                       # accuracy and cost per pack and mode
```

See [`demos/`](demos/README.md), [`demos/planner/`](demos/planner/README.md),
the continual loop in [`demos/README.md`](demos/README.md#continual-loop),
the task packs in [`tasks/`](tasks/README.md), the dialect reference in
[`docs/DIALECT.md`](docs/DIALECT.md) and the write-up in
[`docs/WRITEUP.md`](docs/WRITEUP.md). Read [`docs/PLAN.md`](docs/PLAN.md) for the
architecture, dialect, planner rules, harness design, TUI layout and
milestones, and [`docs/RESEARCH.md`](docs/RESEARCH.md) for the sources.

Written in Rust. Model-agnostic with no SDK and no gateway required: two
thin adapters (Anthropic native, OpenAI-compatible) behind one `Provider`
trait, an alias router with failover and pricing, and an optional Open
Responses gateway adapter for gateways such as
[Aura](https://github.com/UmaiTech/aura-llm-gateway). Sub-work is delegated to declared agent roles
with `CREATE AGENT` and `SPAWN`. Everything the model does is SQL: shell,
file edits, web and memory are `CALL` statements with volatility classes the
planner respects. Session tables, memo and trace live in one embedded DuckDB
file.

The name is also used by an unrelated small JavaScript library
(`fluture-js/callgebra`).

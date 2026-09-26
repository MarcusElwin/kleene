# Jev: typed decisions in the harness

TypeSafe's Jev is a *decision model*, not a text model. It takes a JSON
state and named questions, and returns typed answers with probabilities:

| question | answer |
|---|---|
| noul, a yes/no question | probability in `[0, 1]` that the answer is yes |
| choice, pick one of named labels | the winning label, a confidence, a probability per label |
| score, rate on an ordered rubric | the expected level (fractional), a confidence, a probability per level |

It is priced on input tokens only and answers in well under a second, which
makes it a cheap typed judge. Kleene uses it where a text call would be slow
and expensive and where a probability is more useful than a sentence: inside
filters, `ORDER BY`, classification joins, and as the proxy of a cascade.

## Configuration

```bash
export TYPESAFE_API_KEY=ts-...            # required
export TYPESAFE_BASE_URL=https://...      # optional, default https://api.typesafe.ai
export TYPESAFE_DEFAULT_MODEL=jev-latest  # optional
```

or in `~/.config/kleene/config.toml`:

```toml
[typesafe]
api_key = "ts-..."
model = "jev-latest"
```

`kleene config` shows whether it is configured. Jev is optional: without a
key the `jev_*` functions fail with a message that names the variable, and
nothing else changes. A decision provider alone cannot run a session; a
text provider is still needed for the REPL loop.

## SQL surface

Four built-ins, in every catalog. `state` is the content to decide about
(text, or JSON text which is sent structured); labels and levels are a JSON
array or a comma-separated list.

```sql
SELECT jev_noul(ticket, 'Is this about billing?') AS p FROM tickets;
SELECT jev_choice(ticket, 'What is the tone?', 'calm, angry, confused') AS tone FROM tickets;
SELECT jev_score(ticket, 'How severe?', '["minor", "major", "critical"]') AS severity FROM tickets;
SELECT t.id, c.label, c.probability
FROM tickets t CROSS JOIN LATERAL jev_choices(t.ticket, 'Which team?', 'billing, infra, sales') AS c;
```

| function | signature | answer |
|---|---|---|
| `jev_noul` | `(state TEXT, question TEXT) -> DOUBLE` | yes probability |
| `jev_choice` | `(state TEXT, question TEXT, labels TEXT) -> TEXT` | the most probable label |
| `jev_score` | `(state TEXT, question TEXT, levels TEXT) -> DOUBLE` | expected level, `0` to `n - 1` |
| `jev_choices` | `(state TEXT, question TEXT, labels TEXT) -> TABLE(label TEXT, probability DOUBLE)` | every label, most probable first |

Typical shapes:

- **Beam search.** `ORDER BY jev_score(item, 'How promising?', 'dead end, maybe, promising') DESC LIMIT 5`
  in the recursive term of an `EXPAND` search keeps the frontier small without
  a text call per candidate.
- **Cheap filter before an expensive one.** `WHERE jev_noul(x, '...') > 0.9 AND verify(x)`:
  the planner evaluates the cheap conjunct first.
- **Routing a row.** `jev_choice(task, 'Which role should do this?', 'mapper, verifier, worker')`
  feeds `spawn(...)`.

## Prompt-defined functions on the `jev` alias

`MODEL 'jev'` on a `CREATE FUNCTION ... AS PROMPT` sends the function to Jev
instead of a text model. The template is the question and the arguments are
the state, as a JSON object keyed by argument name:

```sql
CREATE FUNCTION plausible(c TEXT) RETURNS DOUBLE  AS PROMPT 'Is {c} a plausible idea?' MODEL 'jev';
CREATE FUNCTION likely(c TEXT)    RETURNS BOOLEAN AS PROMPT 'Is {c} a plausible idea?' MODEL 'jev';
```

`RETURNS DOUBLE` is the yes probability and `RETURNS BOOLEAN` is the same
at the 0.5 threshold. Other return types are refused; use `jev_choice` for
labels.

The `DOUBLE` form is exactly what a cascade proxy needs, so a Jev function
can stand in front of any boolean oracle:

```sql
CREATE FUNCTION good(c TEXT) RETURNS BOOLEAN AS PROMPT 'Is {c} good?'
  PROXY plausible THRESHOLDS (0.2, 0.8);
SELECT idea FROM ideas WHERE good(idea);
```

Jev scores every row; rows at or above `0.8` pass and rows below `0.2` are
dropped without the text model, which sees only the band in between. The
cost model prices the `jev` alias at a hundredth of a text call, so
`EXPLAIN` shows the cascade whenever the oracle is on a text tier.

## Accounting

A decision is a call. It takes one slot from `budget.calls`, is charged its
reported cost (input tokens at the list price, output free), lands in
`trace_calls` under alias `jev`, and is memoised on its fingerprint (state,
model, questions) like any other call, so the same question about the same
row is free the second time. `kleene bench run --record` writes decision
fixtures next to completions and `--replay` serves both.

## Where it lives

- `kleene-core`: `ModelAlias::jev` and the `jev_*` catalog entries (`decision_functions`).
- `kleene-llm`: the `DecisionProvider` trait and its types (`decision.rs`), the
  TypeSafe adapter (`adapters/typesafe.rs`, `POST /v1/systemone` over
  `reqwest`, no SDK), replay and recording of decisions, `[typesafe]` settings.
- `kleene-algebra`: the `jev` alias cost factor.
- `kleene-harness`: `LiveSink::decide` (memo, budget, trace), the `jev_*`
  implementations, `MODEL 'jev'` on prompt-defined functions, the prompt
  section that shows the model what it can ask, and `ScriptedDecisions` for tests.

## Not yet

The Jev design notes put decisions at every high-frequency point of an
agent loop. Three of those fit Kleene and are separate changes: a Jev
difficulty prior for user tasks in the learn loop, routing subtasks by a
Jev choice, and an allow/ask/deny policy on `shell`.

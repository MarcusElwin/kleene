/**
 * Reads the committed benchmark rows (`kleene bench csv` output under ../plots)
 * at build time and aggregates them per pack, mode and model. Adding a run is
 * adding its evals.csv to RUNS.
 */
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const repo = dirname(dirname(dirname(dirname(fileURLToPath(import.meta.url)))));

/** The committed runs: a CSV (plus any later CSVs of the same model under
 * `csvs`), the model label, and when and how it ran. */
export const RUNS: { csv: string; csvs?: string[]; model: string; label: string; date: string; plots: string; note: string }[] = [
  { csv: "plots/evals.csv", model: "claude-opus-5-5", label: "Claude Opus 5.5", date: "27 Sep 2026", plots: "plots", note: "root on Opus 5.5 at high effort, worker and judge on Sonnet 5, proxy on Haiku 4.5" },
  { csv: "plots/haiku-2026-10-01/evals.csv", csvs: ["plots/haiku-2026-10-06/evals.csv"], model: "claude-haiku-4-5-20251001", label: "Claude Haiku 4.5", date: "1 and 6 Oct 2026", plots: "plots/haiku-2026-10-01", note: "every solver alias on Haiku 4.5, judge on Sonnet 5.5; coding and memo-rubric on 1 October, the four original packs, coding plain and logbook-hard (capped at ten tasks) on 6 October with the conversation cached" },
  { csv: "plots/luna-2026-10-05/evals.csv", model: "gpt-6-luna", label: "GPT-6 Luna", date: "5 Oct 2026", plots: "plots/luna-2026-10-05", note: "every solver alias on GPT-6 Luna over OpenAI chat completions, judge on Sonnet 5.5, all seven packs; logbook-hard capped at ten tasks and its learning run stopped after three" },
];

export const MODES = ["learning", "frozen", "plain"] as const;
export type Mode = (typeof MODES)[number];

export interface Row { pack: string; mode: Mode; model: string; task: string; solved: boolean; calls: number; tokens: number; dollars: number; turns: number; wall_ms: number }
export interface Cell { pack: string; mode: Mode; model: string; label: string; tasks: number; solved: number; pass: number; dollars: number; dollarsPerTask: number; callsPerTask: number; tokensPerTask: number; secondsPerTask: number }

function parseCsv(text: string): Record<string, string>[] {
  const [head, ...lines] = text.trim().split("\n");
  const cols = head.split(",");
  return lines.filter(Boolean).map((l) => Object.fromEntries(l.split(",").map((v, i) => [cols[i], v])));
}

export function loadRows(): Row[] {
  const rows: Row[] = [];
  for (const run of RUNS) {
    for (const csv of [run.csv, ...(run.csvs ?? [])]) for (const r of parseCsv(readFileSync(join(repo, csv), "utf8"))) {
      rows.push({ pack: r.pack, mode: r.mode as Mode, model: r.model ?? run.model, task: r.task, solved: r.solved === "true", calls: +r.calls, tokens: +r.tokens, dollars: +r.dollars, turns: +r.turns, wall_ms: +r.wall_ms });
    }
  }
  return rows;
}

export function aggregate(rows: Row[]): Cell[] {
  const groups = new Map<string, Row[]>();
  for (const r of rows) {
    const k = `${r.pack}|${r.mode}|${r.model}`;
    (groups.get(k) ?? groups.set(k, []).get(k)!).push(r);
  }
  const cells: Cell[] = [];
  for (const [k, g] of groups) {
    const [pack, mode, model] = k.split("|");
    const n = g.length;
    const sum = (f: (r: Row) => number) => g.reduce((a, r) => a + f(r), 0);
    const solved = g.filter((r) => r.solved).length;
    const dollars = sum((r) => r.dollars);
    cells.push({ pack, mode: mode as Mode, model, label: RUNS.find((x) => x.model === model)?.label ?? model, tasks: n, solved, pass: solved / n, dollars, dollarsPerTask: dollars / n, callsPerTask: sum((r) => r.calls) / n, tokensPerTask: sum((r) => r.tokens) / n, secondsPerTask: sum((r) => r.wall_ms) / n / 1000 });
  }
  const modeIx = (m: Mode) => MODES.indexOf(m);
  return cells.sort((a, b) => a.pack.localeCompare(b.pack) || modeIx(a.mode) - modeIx(b.mode) || a.label.localeCompare(b.label));
}

/** Pack order on the page: the first four Opus packs, then the harder ones. */
export const PACK_ORDER = ["terminal", "oolong-like", "finance-synthetic", "legal-synthetic", "coding", "memo-rubric", "logbook-hard"];

export const fmt = {
  money: (x: number) => (x >= 1 ? `$${x.toFixed(2)}` : `$${x.toFixed(3)}`),
  num: (x: number, d = 1) => x.toFixed(d),
  int: (x: number) => Math.round(x).toLocaleString("en-US"),
  pct: (x: number) => `${Math.round(x * 100)}%`,
};

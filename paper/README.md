# Kleene technical report

An arXiv-style write-up of the design and the benchmark results that
`docs/WRITEUP.md` reports: the call algebra, the planner and budgets, the
replay-gated playbook, the benchmark harness, the Opus 5.5, Haiku 4.5 and
GPT-6 Luna runs across the eleven packs and the three recursion-depth
packs, related work and limitations. Version 3 (November 2026) adds the
hard packs, the Opus logbook-hard run, the recursion-depth packs with the
four harness fixes they forced, conversation memory, capability profiles
and Windows.

- `main.tex`, `references.bib`: the paper. Every arXiv identifier in the
  bibliography was checked against the arXiv API on 2026-10-03.
- `figures/`: every figure is drawn from the evals CSVs and plots under
  `plots/` by `figures/make_figures.py` (matplotlib); `make figures` runs it.
- `Makefile`: `make` builds `main.pdf` with latexmk.

The numbers are copied from `docs/WRITEUP.md` sections 4 to 4.3,
`docs/BENCHMARKS.md` (the hard packs and recursion depth),
`plots/results/results.md` and `plots/*/report.txt`; when a new run lands,
update them and the paper together. Set `KLEENE_FIG_PNG=<dir>` when running
`make figures` to also get a PNG of every figure.

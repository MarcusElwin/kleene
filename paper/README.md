# Kleene technical report

An arXiv-style write-up of the design and the benchmark results that
`docs/WRITEUP.md` reports: the call algebra, the planner and budgets, the
replay-gated playbook, the benchmark harness, the Opus 5.5 and Haiku 4.5
runs, related work and limitations.

- `main.tex`, `references.bib`: the paper. Every arXiv identifier in the
  bibliography was checked against the arXiv API on 2026-10-03.
- `figures/`: PDFs converted from the SVGs under `plots/` (`make figures`).
- `Makefile`: `make` builds `main.pdf` with latexmk.

The numbers are copied from `docs/WRITEUP.md` sections 4 and 4.1 and from
`plots/*/report.txt`; when a new run lands, update both.

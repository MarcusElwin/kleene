#!/usr/bin/env python3
"""Draw the paper's benchmark figures from the committed eval rows.

Reads the evals CSVs under ../../plots and writes PDFs next to this script:

  opus_cost.pdf        Opus 5.5 on the four original packs: dollars per task by mode
  haiku_curves.pdf     Haiku 4.5, 1 October: rolling-5 accuracy on memo-rubric and coding
  luna_curves.pdf      GPT-6 Luna: rolling-5 accuracy, one panel per pack
  pareto.pdf           pass rate against dollars per task, three packs, every model and mode
  estimate.pdf         planner estimate against actual calls per statement (Haiku, 1 October),
                       read back from the points of plots/haiku-2026-10-01/estimate_accuracy.svg
                       because the trace store of that run is not checked in
  hard_pareto.pdf      the four hard packs (October 2026): pass rate against dollars per task
  recursion.pdf        the recursion-depth packs: tasks solved per tree depth and the deepest
                       child session each task opened, one series per run

Set KLEENE_FIG_PNG to a directory to also write a PNG of every figure there.

Run from anywhere: `python3 paper/figures/make_figures.py`. Needs matplotlib.
"""

from __future__ import annotations

import csv
import math
import os
from collections import defaultdict
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402
from matplotlib.lines import Line2D  # noqa: E402

HERE = Path(__file__).resolve().parent
PLOTS = HERE.parent.parent / "plots"

RUNS = {
    "opus-09": PLOTS / "evals.csv",
    "haiku-10-01": PLOTS / "haiku-2026-10-01" / "evals.csv",
    "haiku-10-06": PLOTS / "haiku-2026-10-06" / "evals.csv",
    "luna": PLOTS / "luna-2026-10-05" / "evals.csv",
    "opus-10-06": PLOTS / "opus-2026-10-06" / "evals.csv",
    "haiku-10-13": PLOTS / "haiku-2026-10-13" / "evals.csv",
    "luna-10-13": PLOTS / "luna-2026-10-13" / "evals.csv",
    "opus-10-26": PLOTS / "opus-2026-10-26" / "evals.csv",
}

# The recursion-depth runs, all frozen mode, in the order the paper reports them.
RECURSION_RUNS = [
    ("Haiku 4.5, plain pack", PLOTS / "haiku-recursion-2026-10-29" / "evals.csv", "#eb6834", "o"),
    ("Luna, plain pack", PLOTS / "luna-recursion-2026-10-31" / "rerun" / "evals.csv", "#1baf7a", "o"),
    ("Luna, guided pack", PLOTS / "luna-recursion-2026-10-31" / "guided" / "evals.csv", "#1baf7a", "s"),
    ("Luna, long pack", PLOTS / "luna-recursion-2026-10-31" / "long" / "evals.csv", "#1baf7a", "^"),
    ("Opus 5.5, plain pack", PLOTS / "opus-recursion-2026-11-02" / "evals.csv", "#2a78d6", "o"),
]

MODES = ["learning", "frozen", "plain"]
MODE_COLOR = {"learning": "#2a78d6", "frozen": "#eb6834", "plain": "#1baf7a"}
MODEL_NAME = {
    "claude-opus-5-5": "Claude Opus 5.5",
    "claude-haiku-4-5-20251001": "Claude Haiku 4.5",
    "gpt-6-luna": "GPT-6 Luna",
}
MODEL_COLOR = {
    "Claude Opus 5.5": "#2a78d6",
    "Claude Haiku 4.5": "#eb6834",
    "GPT-6 Luna": "#1baf7a",
}
MODE_MARKER = {"learning": "o", "frozen": "s", "plain": "^"}
PACK_ORDER = [
    "terminal",
    "oolong-like",
    "finance-synthetic",
    "legal-synthetic",
    "coding",
    "memo-rubric",
    "logbook-hard",
]
TEXT = "#0b0b0b"
MUTED = "#52514e"
GRID = "#e6e5e2"

plt.rcParams.update(
    {
        "font.family": "DejaVu Sans",
        "font.size": 8,
        "axes.titlesize": 8.5,
        "axes.labelsize": 8,
        "axes.edgecolor": MUTED,
        "axes.linewidth": 0.5,
        "axes.spines.top": False,
        "axes.spines.right": False,
        "xtick.color": MUTED,
        "ytick.color": MUTED,
        "xtick.labelsize": 7,
        "ytick.labelsize": 7,
        "xtick.major.width": 0.5,
        "ytick.major.width": 0.5,
        "grid.color": GRID,
        "grid.linewidth": 0.5,
        "legend.fontsize": 7.5,
        "legend.frameon": False,
        "text.color": TEXT,
        "axes.labelcolor": TEXT,
        "pdf.fonttype": 42,
    }
)


def load(path: Path) -> list[dict]:
    rows = list(csv.DictReader(open(path)))
    for r in rows:
        r["seq"] = int(r["seq"])
        r["solved"] = r["solved"] == "true"
        r["dollars"] = float(r["dollars"])
        r["calls"] = float(r["calls"])
        if "depth" in r:
            r["depth"] = int(float(r["depth"]))
    return rows


def save(fig, name: str):
    fig.savefig(HERE / f"{name}.pdf")
    png_dir = os.environ.get("KLEENE_FIG_PNG")
    if png_dir:
        fig.savefig(Path(png_dir) / f"{name}.png", dpi=200)


def by_run(rows: list[dict]) -> dict[tuple[str, str], list[dict]]:
    out: dict[tuple[str, str], list[dict]] = defaultdict(list)
    for r in rows:
        out[(r["pack"], r["mode"])].append(r)
    for v in out.values():
        v.sort(key=lambda r: r["seq"])
    return out


def rolling(rows: list[dict], window: int = 5) -> tuple[list[int], list[float]]:
    xs, ys = [], []
    solved = [1.0 if r["solved"] else 0.0 for r in rows]
    for i in range(len(solved)):
        lo = max(0, i - window + 1)
        xs.append(i + 1)
        ys.append(sum(solved[lo : i + 1]) / (i + 1 - lo))
    return xs, ys


def mode_legend(ax, modes=MODES, loc="upper right", **kw):
    handles = [
        Line2D([0], [0], color=MODE_COLOR[m], lw=1.6, marker=MODE_MARKER[m], ms=3.5, label=m)
        for m in modes
    ]
    ax.legend(handles=handles, loc=loc, handlelength=1.6, **kw)


def tidy(ax):
    ax.grid(axis="y", zorder=0)
    ax.set_axisbelow(True)
    ax.tick_params(length=2)


# ---------------------------------------------------------------- opus cost


def fig_opus_cost():
    rows = load(RUNS["opus-09"])
    runs = by_run(rows)
    packs = ["terminal", "oolong-like", "finance-synthetic", "legal-synthetic"]
    fig, axes = plt.subplots(1, 4, figsize=(6.5, 2.0), sharey=False)
    for ax, pack in zip(axes, packs):
        vals = []
        for i, m in enumerate(MODES):
            rs = runs[(pack, m)]
            cents = 100 * sum(r["dollars"] for r in rs) / len(rs)
            vals.append(cents)
            ax.bar(i, cents, width=0.62, color=MODE_COLOR[m], zorder=3, linewidth=0)
            ax.text(i, cents, f"{cents:.1f}¢", ha="center", va="bottom", fontsize=6.5, color=TEXT)
        ax.set_xticks(range(3), MODES, fontsize=6.5)
        ax.set_title(f"{pack} ({len(runs[(pack, 'frozen')])} tasks)")
        ax.set_ylim(0, max(vals) * 1.25)
        ax.set_yticks([])
        ax.spines["left"].set_visible(False)
        tidy(ax)
        ax.grid(False)
    axes[0].set_ylabel("cents per task", fontsize=7)
    fig.tight_layout(w_pad=1.0)
    save(fig, "opus_cost")
    plt.close(fig)


# ---------------------------------------------------------------- curves


def curve_panel(ax, runs, pack, modes=MODES, label_end=True):
    for m in modes:
        if (pack, m) not in runs:
            continue
        xs, ys = rolling(runs[(pack, m)])
        ax.plot(xs, ys, color=MODE_COLOR[m], lw=1.6, marker=MODE_MARKER[m], ms=2.6, zorder=3, clip_on=False)
    ax.set_ylim(-0.02, 1.02)
    ax.set_yticks([0, 0.5, 1.0], ["0", ".5", "1"])
    n = max(len(runs[(pack, m)]) for m in modes if (pack, m) in runs)
    ax.set_xlim(1, max(n, 2))
    ax.set_xticks([1, n] if n > 1 else [1])
    tidy(ax)


def fig_haiku_curves():
    runs = by_run(load(RUNS["haiku-10-01"]))
    fig, axes = plt.subplots(1, 2, figsize=(6.5, 2.1))
    curve_panel(axes[0], runs, "memo-rubric")
    axes[0].set_title("memo-rubric (20 tasks)")
    curve_panel(axes[1], runs, "coding", modes=["learning", "frozen"])
    axes[1].set_title("coding (12 steps)")
    for ax in axes:
        ax.set_xlabel("task number")
    axes[0].set_ylabel("accuracy, rolling 5")
    mode_legend(axes[0], loc="upper right")
    mode_legend(axes[1], modes=["learning", "frozen"], loc="upper right")
    fig.tight_layout(w_pad=1.5)
    save(fig, "haiku_curves")
    plt.close(fig)


def fig_luna_curves():
    runs = by_run(load(RUNS["luna"]))
    fig, axes = plt.subplots(2, 4, figsize=(6.5, 3.4))
    axes = axes.ravel()
    for ax, pack in zip(axes, PACK_ORDER):
        curve_panel(ax, runs, pack)
        n = len(runs[(pack, "frozen")])
        ax.set_title(f"{pack} ({n})")
    axes[-1].axis("off")
    mode_legend(axes[-1], loc="center left")
    for ax in axes[4:7]:
        ax.set_xlabel("task number")
    for ax in (axes[0], axes[4]):
        ax.set_ylabel("accuracy, rolling 5")
    fig.tight_layout(w_pad=1.2, h_pad=1.4)
    save(fig, "luna_curves")
    plt.close(fig)


# ---------------------------------------------------------------- pareto


def fig_pareto():
    pareto_figure(["memo-rubric", "coding", "logbook-hard"], "pareto")


def fig_hard_pareto():
    pareto_figure(["oolong-hard", "finance-hard", "legal-hard", "terminal-hard"], "hard_pareto")


def pareto_figure(packs: list[str], name: str):
    cells: dict[tuple[str, str, str], list[dict]] = defaultdict(list)
    for path in RUNS.values():
        for r in load(path):
            cells[(r["pack"], r["mode"], MODEL_NAME[r["model"]])].append(r)
    fig, axes = plt.subplots(1, len(packs), figsize=(6.5, 2.5))
    for ax, pack in zip(axes, packs):
        pts = []
        for (p, mode, model), rs in cells.items():
            if p != pack:
                continue
            cost = sum(r["dollars"] for r in rs) / len(rs)
            acc = sum(1 for r in rs if r["solved"]) / len(rs)
            pts.append((cost, acc, mode, model))
            ax.scatter(
                cost,
                acc,
                s=26,
                color=MODEL_COLOR[model],
                marker=MODE_MARKER[mode],
                edgecolors="white",
                linewidths=0.8,
                zorder=4,
            )
        # Pareto frontier: cheapest point for each accuracy level, walking up
        front = []
        best = -1.0
        for cost, acc, _, _ in sorted(pts):
            if acc > best:
                front.append((cost, acc))
                best = acc
        if len(front) > 1:
            ax.plot([c for c, _ in front], [a for _, a in front], color=MUTED, lw=0.8, ls=(0, (3, 2)), zorder=2)
        ax.set_xscale("log")
        ax.set_ylim(-0.03, 1.05)
        ax.set_yticks([0, 0.25, 0.5, 0.75, 1.0], ["0", "25%", "50%", "75%", "100%"])
        ax.set_title(pack)
        ax.set_xlabel("dollars per task (log)")
        tidy(ax)
        ax.grid(axis="x", zorder=0)
    axes[0].set_ylabel("pass rate")
    model_handles = [
        Line2D([0], [0], marker="o", color="none", markerfacecolor=c, ms=5, label=m)
        for m, c in MODEL_COLOR.items()
    ]
    mode_handles = [
        Line2D([0], [0], marker=MODE_MARKER[m], color="none", markerfacecolor=MUTED, ms=5, label=m)
        for m in MODES
    ]
    fig.legend(
        handles=model_handles + mode_handles,
        loc="lower center",
        ncol=6,
        bbox_to_anchor=(0.5, -0.02),
        columnspacing=1.2,
        handletextpad=0.3,
    )
    fig.tight_layout(rect=(0, 0.08, 1, 1), w_pad=1.2)
    save(fig, name)
    plt.close(fig)


# ---------------------------------------------------------------- recursion


def fig_recursion():
    levels = [3, 4, 5, 6]
    runs = [(label, load(path), color, marker) for label, path, color, marker in RECURSION_RUNS]
    fig, (ax_solved, ax_depth) = plt.subplots(1, 2, figsize=(6.5, 2.5), gridspec_kw={"width_ratios": [1.15, 1]})

    # Left: tasks solved out of five at each tree depth, one bar per run.
    n = len(runs)
    width = 0.8 / n
    for i, (label, rows, color, marker) in enumerate(runs):
        xs, ys = [], []
        for j, k in enumerate(levels):
            solved = sum(1 for r in rows if r["task"].startswith(f"k{k}-") and r["solved"])
            xs.append(j + (i - (n - 1) / 2) * width)
            ys.append(solved)
        hatch = {"o": None, "s": "////", "^": "...."}[marker]
        ax_solved.bar(xs, ys, width=width * 0.92, color=color, hatch=hatch, edgecolor="white",
                      linewidth=0.4, zorder=3, label=label)
    ax_solved.set_xticks(range(len(levels)), [f"{k} levels" for k in levels])
    ax_solved.set_ylim(0, 5.3)
    ax_solved.set_yticks(range(0, 6))
    ax_solved.set_ylabel("tasks solved (of 5)")
    ax_solved.set_title("accuracy by tree depth")
    tidy(ax_solved)

    # Right: the deepest child session each task opened, against the depth the task was built for.
    rng_offsets = [(-0.3, 0.0), (-0.15, 0.0), (0.0, 0.0), (0.15, 0.0), (0.3, 0.0)]
    ax_depth.plot([2.6, 6.4], [2.6, 6.4], color=MUTED, lw=0.6, ls=(0, (3, 2)), zorder=1)
    ax_depth.text(6.35, 6.1, "depth the task\nwas built for", ha="right", va="top", fontsize=6.3, color=MUTED)
    for (label, rows, color, marker), (dx, _) in zip(runs, rng_offsets):
        counts: dict[tuple[int, int], int] = defaultdict(int)
        for r in rows:
            k = int(r["task"][1])
            counts[(k, r["depth"])] += 1
        for (k, d), c in counts.items():
            ax_depth.scatter(k + dx, d, s=14 + 9 * c, color=color, marker=marker, edgecolors="white",
                             linewidths=0.6, zorder=4, alpha=0.9)
    ax_depth.set_xlim(2.5, 6.5)
    ax_depth.set_ylim(-0.3, 6.5)
    ax_depth.set_xticks(levels, [f"k{k}" for k in levels])
    ax_depth.set_yticks(range(0, 7))
    ax_depth.set_xlabel("tree depth of the task")
    ax_depth.set_ylabel("deepest child session")
    ax_depth.set_title("depth the model used")
    tidy(ax_depth)
    ax_depth.text(0.03, 0.96, "area is the number of tasks at a point", transform=ax_depth.transAxes,
                  ha="left", va="top", fontsize=6.3, color=MUTED)
    fig.legend(loc="lower center", ncol=5, bbox_to_anchor=(0.5, -0.02), fontsize=6.5,
               columnspacing=1.0, handlelength=1.2, handletextpad=0.4)
    fig.tight_layout(rect=(0, 0.08, 1, 1), w_pad=1.6)
    save(fig, "recursion")
    plt.close(fig)


# ---------------------------------------------------------------- estimates


def fig_estimate():
    import re

    svg = (PLOTS / "haiku-2026-10-01" / "estimate_accuracy.svg").read_text()
    pts = re.findall(r'<circle cx="([\d.]+)" cy="([\d.]+)"', svg)
    # axis geometry of bench plot's SVG: x 64..696 -> 0..300, y 368..40 -> 0..300
    counts: dict[tuple[int, int], int] = defaultdict(int)
    for cx, cy in pts:
        est = round((float(cx) - 64) * 300 / 632)
        act = round((368 - float(cy)) * 300 / 328)
        counts[(est, act)] += 1
    fig, ax = plt.subplots(figsize=(4.2, 3.2))
    ax.plot([0, 320], [0, 320], color=MUTED, lw=0.6, ls=(0, (3, 2)), zorder=1)
    for (est, act), n in sorted(counts.items(), key=lambda kv: -kv[1]):
        area = 14 + 2.2 * math.sqrt(n) * 6
        ax.scatter(est, act, s=area, color=MODE_COLOR["frozen"], alpha=0.75,
                   edgecolors="white", linewidths=0.6, zorder=3)
        if n >= 50:
            r = math.sqrt(area) / 2  # bubble radius in points
            if act < est:            # below the diagonal: label straight right
                off, ha, va = (r + 3, 0), "left", "center"
            elif est == 0:           # the origin: label floats above, with a leader
                off, ha, va = (-2, r + 26), "left", "bottom"
            else:                    # on the diagonal: label up and to the right
                off, ha, va = (r + 2, r + 2), "left", "center"
            arrow = dict(arrowstyle="-", color=MUTED, lw=0.5, shrinkB=r) if est == 0 else None
            ax.annotate(f"{n} statements", (est, act), xytext=off, textcoords="offset points",
                        fontsize=6.5, color=TEXT, ha=ha, va=va, arrowprops=arrow)
    ax.set_xscale("symlog", linthresh=5)
    ax.set_yscale("symlog", linthresh=5)
    ax.set_xlim(-0.5, 350)
    ax.set_ylim(-0.5, 350)
    ticks = [0, 1, 2, 5, 10, 30, 100, 300]
    ax.set_xticks(ticks, [str(t) for t in ticks])
    ax.set_yticks(ticks, [str(t) for t in ticks])
    ax.set_xlabel("estimated calls (symlog)")
    ax.set_ylabel("actual calls (symlog)")
    ax.grid(axis="both", zorder=0)
    ax.set_axisbelow(True)
    ax.tick_params(length=2)
    ax.text(0.97, 0.05, f"{len(pts)} statements; area is the count at a point",
            transform=ax.transAxes, ha="right", va="bottom", fontsize=6.5, color=MUTED)
    fig.tight_layout()
    save(fig, "estimate")
    plt.close(fig)


if __name__ == "__main__":
    fig_estimate()
    fig_opus_cost()
    fig_haiku_curves()
    fig_luna_curves()
    fig_pareto()
    fig_hard_pareto()
    fig_recursion()
    print("wrote estimate, opus_cost, haiku_curves, luna_curves, pareto, hard_pareto, recursion PDFs to", HERE)

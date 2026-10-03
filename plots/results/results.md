| Pack | Mode | Claude Opus 5.5 pass | Claude Opus 5.5 $/task | Claude Opus 5.5 calls | Claude Haiku 4.5 pass | Claude Haiku 4.5 $/task | Claude Haiku 4.5 calls |
|---|---|---:|---:|---:|---:|---:|---:|
| `coding` | learning | | | | 2/12 (17%) | $0.039 | 2.7 |
| `coding` | frozen | | | | 2/12 (17%) | $0.072 | 4.6 |
| `finance-synthetic` | learning | 20/20 (100%) | $0.031 | 2.4 | | | |
| `finance-synthetic` | frozen | 20/20 (100%) | $0.042 | 3.5 | | | |
| `finance-synthetic` | plain | 20/20 (100%) | $0.004 | 1.0 | | | |
| `legal-synthetic` | learning | 20/20 (100%) | $0.130 | 10.6 | | | |
| `legal-synthetic` | frozen | 20/20 (100%) | $0.085 | 12.7 | | | |
| `legal-synthetic` | plain | 20/20 (100%) | $0.005 | 1.0 | | | |
| `memo-rubric` | learning | | | | 11/20 (55%) | $0.018 | 3.8 |
| `memo-rubric` | frozen | | | | 8/20 (40%) | $0.065 | 19.9 |
| `memo-rubric` | plain | | | | 8/20 (40%) | $0.018 | 3.1 |
| `oolong-like` | learning | 20/20 (100%) | $0.096 | 3.9 | | | |
| `oolong-like` | frozen | 20/20 (100%) | $0.076 | 4.9 | | | |
| `oolong-like` | plain | 20/20 (100%) | $0.020 | 1.0 | | | |
| `terminal` | learning | 6/6 (100%) | $0.007 | 1.3 | | | |
| `terminal` | frozen | 6/6 (100%) | $0.012 | 2.2 | | | |
| `terminal` | plain | 6/6 (100%) | $0.006 | 2.5 | | | |

Models: Claude Opus 5.5 is `claude-opus-5-5`, Claude Haiku 4.5 is `claude-haiku-4-5-20251001`.

<details>
<summary><code>coding</code>: pass rate against cost per task, and every metric per model and mode</summary>

![coding: pass rate against cost](plots/results/coding-pareto.svg)

| Model | Mode | Tasks | Pass | $/task | Total $ | Calls/task | Tokens/task | Depth | Seconds/task |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Claude Haiku 4.5 | learning | 12 | 2/12 (17%) | $0.039 | $0.473 | 2.7 | 29,910 | 0.00 | 26.5 |
| Claude Haiku 4.5 | frozen | 12 | 2/12 (17%) | $0.072 | $0.868 | 4.6 | 48,375 | 0.00 | 43.9 |

</details>

<details>
<summary><code>finance-synthetic</code>: every metric per model and mode</summary>

| Model | Mode | Tasks | Pass | $/task | Total $ | Calls/task | Tokens/task | Depth | Seconds/task |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Claude Opus 5.5 | learning | 20 | 20/20 (100%) | $0.031 | $0.628 | 2.4 | 29,999 | 0.00 | 10.2 |
| Claude Opus 5.5 | frozen | 20 | 20/20 (100%) | $0.042 | $0.831 | 3.5 | 42,177 | 0.00 | 12.2 |
| Claude Opus 5.5 | plain | 20 | 20/20 (100%) | $0.004 | $0.072 | 1.0 | 1,244 | 0.00 | 3.0 |

</details>

<details>
<summary><code>legal-synthetic</code>: every metric per model and mode</summary>

| Model | Mode | Tasks | Pass | $/task | Total $ | Calls/task | Tokens/task | Depth | Seconds/task |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Claude Opus 5.5 | learning | 20 | 20/20 (100%) | $0.130 | $2.61 | 10.6 | 58,315 | 0.00 | 25.1 |
| Claude Opus 5.5 | frozen | 20 | 20/20 (100%) | $0.085 | $1.69 | 12.7 | 62,452 | 0.00 | 26.8 |
| Claude Opus 5.5 | plain | 20 | 20/20 (100%) | $0.005 | $0.107 | 1.0 | 1,475 | 0.00 | 3.3 |

</details>

<details>
<summary><code>memo-rubric</code>: pass rate against cost per task, and every metric per model and mode</summary>

![memo-rubric: pass rate against cost](plots/results/memo-rubric-pareto.svg)

| Model | Mode | Tasks | Pass | $/task | Total $ | Calls/task | Tokens/task | Depth | Seconds/task |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Claude Haiku 4.5 | learning | 20 | 11/20 (55%) | $0.018 | $0.365 | 3.8 | 23,041 | 0.00 | 11.9 |
| Claude Haiku 4.5 | frozen | 20 | 8/20 (40%) | $0.065 | $1.30 | 19.9 | 49,043 | 0.05 | 32.2 |
| Claude Haiku 4.5 | plain | 20 | 8/20 (40%) | $0.018 | $0.354 | 3.1 | 10,764 | 0.00 | 13.0 |

</details>

<details>
<summary><code>oolong-like</code>: every metric per model and mode</summary>

| Model | Mode | Tasks | Pass | $/task | Total $ | Calls/task | Tokens/task | Depth | Seconds/task |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Claude Opus 5.5 | learning | 20 | 20/20 (100%) | $0.096 | $1.92 | 3.9 | 41,991 | 0.00 | 16.8 |
| Claude Opus 5.5 | frozen | 20 | 20/20 (100%) | $0.076 | $1.51 | 4.9 | 30,778 | 0.00 | 17.5 |
| Claude Opus 5.5 | plain | 20 | 20/20 (100%) | $0.020 | $0.394 | 1.0 | 3,084 | 0.00 | 7.5 |

</details>

<details>
<summary><code>terminal</code>: every metric per model and mode</summary>

| Model | Mode | Tasks | Pass | $/task | Total $ | Calls/task | Tokens/task | Depth | Seconds/task |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Claude Opus 5.5 | learning | 6 | 6/6 (100%) | $0.007 | $0.043 | 1.3 | 6,742 | 0.00 | 4.1 |
| Claude Opus 5.5 | frozen | 6 | 6/6 (100%) | $0.012 | $0.074 | 2.2 | 10,551 | 0.00 | 5.1 |
| Claude Opus 5.5 | plain | 6 | 6/6 (100%) | $0.006 | $0.033 | 2.5 | 2,395 | 0.00 | 8.0 |

</details>


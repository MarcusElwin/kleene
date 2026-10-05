| Pack | Mode | Model | Tasks | Pass | $/task | Total $ | Calls/task | Tokens/task | Seconds/task |
|---|---|---|---:|---:|---:|---:|---:|---:|---:|
| `coding` | learning | Claude Opus 5.5 | | | | | | | |
| `coding` | learning | Claude Haiku 4.5 | 12 | 2/12 (17%) | $0.039 | $0.473 | 2.7 | 29,910 | 26.5 |
| `coding` | frozen | Claude Opus 5.5 | | | | | | | |
| `coding` | frozen | Claude Haiku 4.5 | 12 | 2/12 (17%) | $0.072 | $0.868 | 4.6 | 48,375 | 43.9 |
| `finance-synthetic` | learning | Claude Opus 5.5 | 20 | 20/20 (100%) | $0.031 | $0.628 | 2.4 | 29,999 | 10.2 |
| `finance-synthetic` | learning | Claude Haiku 4.5 | | | | | | | |
| `finance-synthetic` | frozen | Claude Opus 5.5 | 20 | 20/20 (100%) | $0.042 | $0.831 | 3.5 | 42,177 | 12.2 |
| `finance-synthetic` | frozen | Claude Haiku 4.5 | | | | | | | |
| `finance-synthetic` | plain | Claude Opus 5.5 | 20 | 20/20 (100%) | $0.004 | $0.072 | 1.0 | 1,244 | 3.0 |
| `finance-synthetic` | plain | Claude Haiku 4.5 | | | | | | | |
| `legal-synthetic` | learning | Claude Opus 5.5 | 20 | 20/20 (100%) | $0.130 | $2.61 | 10.6 | 58,315 | 25.1 |
| `legal-synthetic` | learning | Claude Haiku 4.5 | | | | | | | |
| `legal-synthetic` | frozen | Claude Opus 5.5 | 20 | 20/20 (100%) | $0.085 | $1.69 | 12.7 | 62,452 | 26.8 |
| `legal-synthetic` | frozen | Claude Haiku 4.5 | | | | | | | |
| `legal-synthetic` | plain | Claude Opus 5.5 | 20 | 20/20 (100%) | $0.005 | $0.107 | 1.0 | 1,475 | 3.3 |
| `legal-synthetic` | plain | Claude Haiku 4.5 | | | | | | | |
| `memo-rubric` | learning | Claude Opus 5.5 | | | | | | | |
| `memo-rubric` | learning | Claude Haiku 4.5 | 20 | 11/20 (55%) | $0.018 | $0.365 | 3.8 | 23,041 | 11.9 |
| `memo-rubric` | frozen | Claude Opus 5.5 | | | | | | | |
| `memo-rubric` | frozen | Claude Haiku 4.5 | 20 | 8/20 (40%) | $0.065 | $1.30 | 19.9 | 49,043 | 32.2 |
| `memo-rubric` | plain | Claude Opus 5.5 | | | | | | | |
| `memo-rubric` | plain | Claude Haiku 4.5 | 20 | 8/20 (40%) | $0.018 | $0.354 | 3.1 | 10,764 | 13.0 |
| `oolong-like` | learning | Claude Opus 5.5 | 20 | 20/20 (100%) | $0.096 | $1.92 | 3.9 | 41,991 | 16.8 |
| `oolong-like` | learning | Claude Haiku 4.5 | | | | | | | |
| `oolong-like` | frozen | Claude Opus 5.5 | 20 | 20/20 (100%) | $0.076 | $1.51 | 4.9 | 30,778 | 17.5 |
| `oolong-like` | frozen | Claude Haiku 4.5 | | | | | | | |
| `oolong-like` | plain | Claude Opus 5.5 | 20 | 20/20 (100%) | $0.020 | $0.394 | 1.0 | 3,084 | 7.5 |
| `oolong-like` | plain | Claude Haiku 4.5 | | | | | | | |
| `terminal` | learning | Claude Opus 5.5 | 6 | 6/6 (100%) | $0.007 | $0.043 | 1.3 | 6,742 | 4.1 |
| `terminal` | learning | Claude Haiku 4.5 | | | | | | | |
| `terminal` | frozen | Claude Opus 5.5 | 6 | 6/6 (100%) | $0.012 | $0.074 | 2.2 | 10,551 | 5.1 |
| `terminal` | frozen | Claude Haiku 4.5 | | | | | | | |
| `terminal` | plain | Claude Opus 5.5 | 6 | 6/6 (100%) | $0.006 | $0.033 | 2.5 | 2,395 | 8.0 |
| `terminal` | plain | Claude Haiku 4.5 | | | | | | | |

Models: Claude Opus 5.5 is `claude-opus-5-5`, Claude Haiku 4.5 is `claude-haiku-4-5-20251001`.

<details>
<summary><code>coding</code>: pass rate against cost per task, every model and mode</summary>

![coding: pass rate against cost](plots/results/coding-pareto.svg)

</details>

<details>
<summary><code>memo-rubric</code>: pass rate against cost per task, every model and mode</summary>

![memo-rubric: pass rate against cost](plots/results/memo-rubric-pareto.svg)

</details>


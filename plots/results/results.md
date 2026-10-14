**Pass rate**, one column per model; each cell reads `learning / frozen / plain`, the best mode in bold, `–` where that mode has not run.

| Pack | Tasks | Claude Opus 5.5 | Claude Haiku 4.5 | GPT-6 Luna |
|---|---:|---:|---:|---:|
| `coding` | 12 | 100% / 100% / 100% | 17% / 17% / **33%** | 83% / **92%** / 75% |
| `finance-hard` | 20 | not run | 75% / 65% / **95%** | **40%** / **40%** / 30% |
| `finance-synthetic` | 20 | 100% / 100% / 100% | 95% / **100%** / **100%** | 75% / 70% / **100%** |
| `legal-hard` | 20 | not run | 35% / 50% / **55%** | **35%** / 15% / 20% |
| `legal-synthetic` | 20 | 100% / 100% / 100% | **85%** / 70% / 75% | 75% / 90% / **100%** |
| `logbook-hard` | 10 | **100%** / **100%** / 50% | 10% / 10% / **20%** | 33% / **70%** / 30% |
| `memo-rubric` | 20 | 20% / **50%** / **50%** | **55%** / 40% / 40% | **90%** / 40% / 55% |
| `oolong-hard` | 20 | not run | **35%** / 30% / 30% | 35% / **50%** / 0% |
| `oolong-like` | 20 | 100% / 100% / 100% | 95% / 70% / **100%** | **100%** / 75% / 70% |
| `terminal` | 6 | 100% / 100% / 100% | 100% / 100% / 100% | 83% / **100%** / **100%** |
| `terminal-hard` | 8 | not run | **88%** / 62% / **88%** | 50% / 50% / **62%** |

Models: Claude Opus 5.5 is `claude-opus-5-5`, Claude Haiku 4.5 is `claude-haiku-4-5-20251001`, GPT-6 Luna is `gpt-6-luna`.

<details>
<summary><b>Cost per task</b>, same layout: mean dollars of the solver's own calls, the cheapest mode in bold</summary>

| Pack | Tasks | Claude Opus 5.5 | Claude Haiku 4.5 | GPT-6 Luna |
|---|---:|---:|---:|---:|
| `coding` | 12 | $0.142 / **$0.109** / $0.110 | **$0.039** / $0.072 / $0.065 | **$0.0017** / $0.0019 / $0.0023 |
| `finance-hard` | 20 | not run | **$0.077** / $0.144 / $0.196 | $0.011 / $0.041 / **$0.0021** |
| `finance-synthetic` | 20 | $0.031 / $0.042 / **$0.0036** | $0.015 / $0.022 / **$0.0064** | $0.0004 / $0.0005 / **$0.0001** |
| `legal-hard` | 20 | not run | $0.160 / $0.156 / **$0.063** | $0.0029 / $0.0024 / **$0.0011** |
| `legal-synthetic` | 20 | $0.130 / $0.085 / **$0.0054** | $0.015 / $0.036 / **$0.0080** | $0.0007 / $0.0016 / **$0.0001** |
| `logbook-hard` | 10 | **$0.143** / $0.145 / $1.48 | $0.595 / $0.513 / **$0.458** | $0.062 / $0.023 / **$0.0074** |
| `memo-rubric` | 20 | $0.062 / $0.063 / **$0.028** | $0.018 / $0.065 / **$0.018** | $0.0010 / $0.0015 / **$0.0003** |
| `oolong-hard` | 20 | not run | $0.143 / **$0.122** / $0.368 | $0.132 / $0.212 / **$0.0033** |
| `oolong-like` | 20 | $0.096 / $0.076 / **$0.020** | **$0.010** / $0.038 / $0.025 | **$0.0002** / $0.0019 / $0.0004 |
| `terminal` | 6 | $0.0072 / $0.012 / **$0.0056** | $0.0087 / $0.012 / **$0.0082** | $0.0002 / **$0.0002** / $0.0002 |
| `terminal-hard` | 8 | not run | $0.060 / $0.054 / **$0.045** | $0.060 / $0.0064 / **$0.0006** |

</details>

<details>
<summary><b>Every cell</b>: one row per pack, mode and model with tasks, pass, dollars, calls, tokens and seconds</summary>

| Pack | Mode | Model | Tasks | Pass | $/task | Total $ | Calls/task | Tokens/task | Seconds/task |
|---|---|---|---:|---:|---:|---:|---:|---:|---:|
| `coding` | learning | Claude Opus 5.5 | 12 | 12/12 (100%) | $0.142 | $1.70 | 8.0 | 88,984 | 35.2 |
| `coding` | learning | Claude Haiku 4.5 | 12 | 2/12 (17%) | $0.039 | $0.473 | 2.7 | 29,910 | 26.5 |
| `coding` | learning | GPT-6 Luna | 12 | 10/12 (83%) | $0.0017 | $0.020 | 8.2 | 42,094 | 26.2 |
| `coding` | frozen | Claude Opus 5.5 | 12 | 12/12 (100%) | $0.109 | $1.31 | 8.6 | 76,533 | 36.9 |
| `coding` | frozen | Claude Haiku 4.5 | 12 | 2/12 (17%) | $0.072 | $0.868 | 4.6 | 48,375 | 43.9 |
| `coding` | frozen | GPT-6 Luna | 12 | 11/12 (92%) | $0.0019 | $0.023 | 12.1 | 55,286 | 32.6 |
| `coding` | plain | Claude Opus 5.5 | 12 | 12/12 (100%) | $0.110 | $1.32 | 9.4 | 68,156 | 32.9 |
| `coding` | plain | Claude Haiku 4.5 | 12 | 4/12 (33%) | $0.065 | $0.776 | 28.6 | 218,019 | 52.6 |
| `coding` | plain | GPT-6 Luna | 12 | 9/12 (75%) | $0.0023 | $0.028 | 10.2 | 27,860 | 51.7 |
| `finance-hard` | learning | Claude Opus 5.5 | | | | | | | |
| `finance-hard` | learning | Claude Haiku 4.5 | 20 | 15/20 (75%) | $0.077 | $1.55 | 9.1 | 134,760 | 73.9 |
| `finance-hard` | learning | GPT-6 Luna | 20 | 8/20 (40%) | $0.011 | $0.223 | 88.3 | 231,533 | 65.2 |
| `finance-hard` | frozen | Claude Opus 5.5 | | | | | | | |
| `finance-hard` | frozen | Claude Haiku 4.5 | 20 | 13/20 (65%) | $0.144 | $2.87 | 14.7 | 264,156 | 124.5 |
| `finance-hard` | frozen | GPT-6 Luna | 20 | 8/20 (40%) | $0.041 | $0.815 | 447.2 | 1,485,152 | 249.2 |
| `finance-hard` | plain | Claude Opus 5.5 | | | | | | | |
| `finance-hard` | plain | Claude Haiku 4.5 | 20 | 19/20 (95%) | $0.196 | $3.93 | 15.7 | 559,397 | 116.9 |
| `finance-hard` | plain | GPT-6 Luna | 20 | 6/20 (30%) | $0.0021 | $0.041 | 1.1 | 24,120 | 2.3 |
| `finance-synthetic` | learning | Claude Opus 5.5 | 20 | 20/20 (100%) | $0.031 | $0.628 | 2.4 | 29,999 | 10.2 |
| `finance-synthetic` | learning | Claude Haiku 4.5 | 20 | 19/20 (95%) | $0.015 | $0.295 | 7.6 | 18,963 | 8.4 |
| `finance-synthetic` | learning | GPT-6 Luna | 20 | 15/20 (75%) | $0.0004 | $0.0078 | 3.5 | 12,081 | 7.0 |
| `finance-synthetic` | frozen | Claude Opus 5.5 | 20 | 20/20 (100%) | $0.042 | $0.831 | 3.5 | 42,177 | 12.2 |
| `finance-synthetic` | frozen | Claude Haiku 4.5 | 20 | 20/20 (100%) | $0.022 | $0.431 | 7.2 | 30,274 | 16.8 |
| `finance-synthetic` | frozen | GPT-6 Luna | 20 | 14/20 (70%) | $0.0005 | $0.0090 | 4.2 | 11,789 | 9.1 |
| `finance-synthetic` | plain | Claude Opus 5.5 | 20 | 20/20 (100%) | $0.0036 | $0.072 | 1.0 | 1,244 | 3.0 |
| `finance-synthetic` | plain | Claude Haiku 4.5 | 20 | 20/20 (100%) | $0.0064 | $0.128 | 1.6 | 5,375 | 2.3 |
| `finance-synthetic` | plain | GPT-6 Luna | 20 | 20/20 (100%) | $0.0001 | $0.0026 | 1.0 | 950 | 1.6 |
| `legal-hard` | learning | Claude Opus 5.5 | | | | | | | |
| `legal-hard` | learning | Claude Haiku 4.5 | 20 | 7/20 (35%) | $0.160 | $3.20 | 29.1 | 187,058 | 89.9 |
| `legal-hard` | learning | GPT-6 Luna | 20 | 7/20 (35%) | $0.0029 | $0.058 | 16.4 | 64,728 | 17.7 |
| `legal-hard` | frozen | Claude Opus 5.5 | | | | | | | |
| `legal-hard` | frozen | Claude Haiku 4.5 | 20 | 10/20 (50%) | $0.156 | $3.13 | 26.9 | 231,769 | 183.7 |
| `legal-hard` | frozen | GPT-6 Luna | 20 | 3/20 (15%) | $0.0024 | $0.048 | 16.1 | 90,304 | 22.2 |
| `legal-hard` | plain | Claude Opus 5.5 | | | | | | | |
| `legal-hard` | plain | Claude Haiku 4.5 | 20 | 11/20 (55%) | $0.063 | $1.26 | 4.6 | 70,256 | 46.6 |
| `legal-hard` | plain | GPT-6 Luna | 20 | 4/20 (20%) | $0.0011 | $0.022 | 1.5 | 16,878 | 2.1 |
| `legal-synthetic` | learning | Claude Opus 5.5 | 20 | 20/20 (100%) | $0.130 | $2.61 | 10.6 | 58,315 | 25.1 |
| `legal-synthetic` | learning | Claude Haiku 4.5 | 20 | 17/20 (85%) | $0.015 | $0.294 | 9.9 | 16,541 | 8.2 |
| `legal-synthetic` | learning | GPT-6 Luna | 20 | 15/20 (75%) | $0.0007 | $0.014 | 4.2 | 15,893 | 13.3 |
| `legal-synthetic` | frozen | Claude Opus 5.5 | 20 | 20/20 (100%) | $0.085 | $1.69 | 12.7 | 62,452 | 26.8 |
| `legal-synthetic` | frozen | Claude Haiku 4.5 | 20 | 14/20 (70%) | $0.036 | $0.717 | 43.5 | 48,594 | 32.3 |
| `legal-synthetic` | frozen | GPT-6 Luna | 20 | 18/20 (90%) | $0.0016 | $0.033 | 15.7 | 23,973 | 33.9 |
| `legal-synthetic` | plain | Claude Opus 5.5 | 20 | 20/20 (100%) | $0.0054 | $0.107 | 1.0 | 1,475 | 3.3 |
| `legal-synthetic` | plain | Claude Haiku 4.5 | 20 | 15/20 (75%) | $0.0080 | $0.161 | 1.9 | 6,227 | 5.2 |
| `legal-synthetic` | plain | GPT-6 Luna | 20 | 20/20 (100%) | $0.0001 | $0.0027 | 1.0 | 1,004 | 1.6 |
| `logbook-hard` | learning | Claude Opus 5.5 | 10 | 10/10 (100%) | $0.143 | $1.43 | 6.7 | 66,997 | 44.5 |
| `logbook-hard` | learning | Claude Haiku 4.5 | 10 | 1/10 (10%) | $0.595 | $5.95 | 1694.4 | 591,443 | 320.4 |
| `logbook-hard` | learning | GPT-6 Luna | 3 | 1/3 (33%) | $0.062 | $0.187 | 686.7 | 746,259 | 760.8 |
| `logbook-hard` | frozen | Claude Opus 5.5 | 10 | 10/10 (100%) | $0.145 | $1.45 | 8.5 | 79,296 | 51.0 |
| `logbook-hard` | frozen | Claude Haiku 4.5 | 10 | 1/10 (10%) | $0.513 | $5.13 | 1702.1 | 476,213 | 287.6 |
| `logbook-hard` | frozen | GPT-6 Luna | 10 | 7/10 (70%) | $0.023 | $0.235 | 247.3 | 180,736 | 209.6 |
| `logbook-hard` | plain | Claude Opus 5.5 | 10 | 5/10 (50%) | $1.48 | $14.82 | 11.9 | 801,803 | 298.7 |
| `logbook-hard` | plain | Claude Haiku 4.5 | 10 | 2/10 (20%) | $0.458 | $4.58 | 22.2 | 1,365,343 | 287.1 |
| `logbook-hard` | plain | GPT-6 Luna | 10 | 3/10 (30%) | $0.0074 | $0.074 | 2.3 | 97,000 | 64.2 |
| `memo-rubric` | learning | Claude Opus 5.5 | 20 | 4/20 (20%) | $0.062 | $1.24 | 5.3 | 44,064 | 19.2 |
| `memo-rubric` | learning | Claude Haiku 4.5 | 20 | 11/20 (55%) | $0.018 | $0.365 | 3.8 | 23,041 | 11.9 |
| `memo-rubric` | learning | GPT-6 Luna | 20 | 18/20 (90%) | $0.0010 | $0.019 | 6.1 | 27,683 | 14.5 |
| `memo-rubric` | frozen | Claude Opus 5.5 | 20 | 10/20 (50%) | $0.063 | $1.26 | 5.3 | 40,086 | 21.6 |
| `memo-rubric` | frozen | Claude Haiku 4.5 | 20 | 8/20 (40%) | $0.065 | $1.30 | 19.9 | 49,043 | 32.2 |
| `memo-rubric` | frozen | GPT-6 Luna | 20 | 8/20 (40%) | $0.0015 | $0.030 | 13.6 | 36,571 | 23.8 |
| `memo-rubric` | plain | Claude Opus 5.5 | 20 | 10/20 (50%) | $0.028 | $0.554 | 1.0 | 5,763 | 9.5 |
| `memo-rubric` | plain | Claude Haiku 4.5 | 20 | 8/20 (40%) | $0.018 | $0.354 | 3.1 | 10,764 | 13.0 |
| `memo-rubric` | plain | GPT-6 Luna | 20 | 11/20 (55%) | $0.0003 | $0.0058 | 1.0 | 1,978 | 4.4 |
| `oolong-hard` | learning | Claude Opus 5.5 | | | | | | | |
| `oolong-hard` | learning | Claude Haiku 4.5 | 20 | 7/20 (35%) | $0.143 | $2.85 | 138.1 | 143,092 | 94.9 |
| `oolong-hard` | learning | GPT-6 Luna | 20 | 7/20 (35%) | $0.132 | $2.64 | 1156.0 | 2,202,176 | 527.2 |
| `oolong-hard` | frozen | Claude Opus 5.5 | | | | | | | |
| `oolong-hard` | frozen | Claude Haiku 4.5 | 20 | 6/20 (30%) | $0.122 | $2.45 | 20.9 | 173,324 | 91.1 |
| `oolong-hard` | frozen | GPT-6 Luna | 14 | 7/14 (50%) | $0.212 | $2.97 | 2032.2 | 5,367,039 | 994.2 |
| `oolong-hard` | plain | Claude Opus 5.5 | | | | | | | |
| `oolong-hard` | plain | Claude Haiku 4.5 | 20 | 6/20 (30%) | $0.368 | $7.36 | 20.8 | 1,047,281 | 281.2 |
| `oolong-hard` | plain | GPT-6 Luna | 20 | 0/20 (0%) | $0.0033 | $0.067 | 1.4 | 46,279 | 2.2 |
| `oolong-like` | learning | Claude Opus 5.5 | 20 | 20/20 (100%) | $0.096 | $1.92 | 3.9 | 41,991 | 16.8 |
| `oolong-like` | learning | Claude Haiku 4.5 | 20 | 19/20 (95%) | $0.010 | $0.200 | 1.9 | 9,115 | 5.1 |
| `oolong-like` | learning | GPT-6 Luna | 20 | 20/20 (100%) | $0.0002 | $0.0049 | 1.3 | 4,516 | 3.5 |
| `oolong-like` | frozen | Claude Opus 5.5 | 20 | 20/20 (100%) | $0.076 | $1.51 | 4.9 | 30,778 | 17.5 |
| `oolong-like` | frozen | Claude Haiku 4.5 | 20 | 14/20 (70%) | $0.038 | $0.753 | 50.4 | 53,217 | 36.4 |
| `oolong-like` | frozen | GPT-6 Luna | 20 | 15/20 (75%) | $0.0019 | $0.038 | 34.0 | 19,230 | 30.5 |
| `oolong-like` | plain | Claude Opus 5.5 | 20 | 20/20 (100%) | $0.020 | $0.394 | 1.0 | 3,084 | 7.5 |
| `oolong-like` | plain | Claude Haiku 4.5 | 20 | 20/20 (100%) | $0.025 | $0.491 | 4.2 | 24,932 | 16.6 |
| `oolong-like` | plain | GPT-6 Luna | 20 | 14/20 (70%) | $0.0004 | $0.0071 | 1.0 | 2,031 | 3.9 |
| `terminal` | learning | Claude Opus 5.5 | 6 | 6/6 (100%) | $0.0072 | $0.043 | 1.3 | 6,742 | 4.1 |
| `terminal` | learning | Claude Haiku 4.5 | 6 | 6/6 (100%) | $0.0087 | $0.052 | 2.2 | 8,159 | 2.0 |
| `terminal` | learning | GPT-6 Luna | 6 | 5/6 (83%) | $0.0002 | $0.0014 | 1.5 | 4,219 | 2.6 |
| `terminal` | frozen | Claude Opus 5.5 | 6 | 6/6 (100%) | $0.012 | $0.074 | 2.2 | 10,551 | 5.1 |
| `terminal` | frozen | Claude Haiku 4.5 | 6 | 6/6 (100%) | $0.012 | $0.070 | 3.0 | 11,190 | 2.9 |
| `terminal` | frozen | GPT-6 Luna | 6 | 6/6 (100%) | $0.0002 | $0.0010 | 1.5 | 4,128 | 2.7 |
| `terminal` | plain | Claude Opus 5.5 | 6 | 6/6 (100%) | $0.0056 | $0.033 | 2.5 | 2,395 | 8.0 |
| `terminal` | plain | Claude Haiku 4.5 | 6 | 6/6 (100%) | $0.0082 | $0.049 | 2.5 | 7,286 | 2.8 |
| `terminal` | plain | GPT-6 Luna | 6 | 6/6 (100%) | $0.0002 | $0.0012 | 2.3 | 1,584 | 2.8 |
| `terminal-hard` | learning | Claude Opus 5.5 | | | | | | | |
| `terminal-hard` | learning | Claude Haiku 4.5 | 8 | 7/8 (88%) | $0.060 | $0.480 | 58.0 | 53,659 | 42.4 |
| `terminal-hard` | learning | GPT-6 Luna | 8 | 4/8 (50%) | $0.060 | $0.477 | 1877.1 | 329,651 | 364.1 |
| `terminal-hard` | frozen | Claude Opus 5.5 | | | | | | | |
| `terminal-hard` | frozen | Claude Haiku 4.5 | 8 | 5/8 (62%) | $0.054 | $0.430 | 25.2 | 59,899 | 44.8 |
| `terminal-hard` | frozen | GPT-6 Luna | 8 | 4/8 (50%) | $0.0064 | $0.051 | 3.2 | 72,152 | 7.9 |
| `terminal-hard` | plain | Claude Opus 5.5 | | | | | | | |
| `terminal-hard` | plain | Claude Haiku 4.5 | 8 | 7/8 (88%) | $0.045 | $0.360 | 16.9 | 83,295 | 25.6 |
| `terminal-hard` | plain | GPT-6 Luna | 8 | 5/8 (62%) | $0.0006 | $0.0049 | 5.9 | 19,196 | 10.4 |

</details>

<details>
<summary><code>coding</code>: pass rate against cost per task, every model and mode</summary>

![coding: pass rate against cost](plots/results/coding-pareto.svg)

</details>

<details>
<summary><code>logbook-hard</code>: pass rate against cost per task, every model and mode</summary>

![logbook-hard: pass rate against cost](plots/results/logbook-hard-pareto.svg)

</details>

<details>
<summary><code>memo-rubric</code>: pass rate against cost per task, every model and mode</summary>

![memo-rubric: pass rate against cost](plots/results/memo-rubric-pareto.svg)

</details>


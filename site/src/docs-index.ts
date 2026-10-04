/** Order and one-line blurbs for the documentation index; slugs are the file names in docs/. */
export const DOCS: { slug: string; title: string; blurb: string }[] = [
  { slug: "cli", title: "The kleene CLI", blurb: "Install, provider setup, environment variables, every command and flag, troubleshooting." },
  { slug: "dialect", title: "CallSQL dialect", blurb: "The language: relational core, sessions, model calls, tools, delegation, planner rules." },
  { slug: "architecture", title: "Architecture", blurb: "The crates, the path of one statement, sessions, the store and trace tables, the daemon protocol, the planner." },
  { slug: "plan", title: "Implementation plan", blurb: "The design and its rationale, milestone by milestone." },
  { slug: "benchmarks", title: "Benchmarks", blurb: "The packs, the learning / frozen / plain modes, the oracles, what bench report and the plots measure." },
  { slug: "writeup", title: "Write-up", blurb: "The claim, the algebra, what has been measured and how to reproduce it." },
  { slug: "research", title: "Research digest", blurb: "Recursive language models, SQL as a model interface, complexity results, benchmarks." },
];

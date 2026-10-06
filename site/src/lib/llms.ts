import { getCollection } from "astro:content";
import { DOCS } from "../docs-index";

const SITE = "https://kleene.sh";

/** The pages of the site that are not rendered from docs/, for llms.txt. */
export const PAGES = [
  { path: "/", title: "Home", blurb: "What Kleene is, a typed example session, recursive language models, why it exists, how to install." },
  { path: "/overview/", title: "Technical overview", blurb: "The algebra of call kinds, EXPLAIN, the execution graph, beams and recursion, the crates." },
  { path: "/benchmarks/", title: "Benchmarks", blurb: "Cost, calls and pass rate per pack and mode for the model runs, with the Pareto plots." },
  { path: "/about/", title: "About", blurb: "Stephen Cole Kleene, the theorems the name points at, references." },
];

/** The docs collection in the order of the documentation index. */
export async function orderedDocs() {
  const entries = await getCollection("docs");
  const bySlug = new Map(entries.map((e) => [e.id.toLowerCase(), e]));
  return DOCS.flatMap((d) => {
    const entry = bySlug.get(d.slug);
    return entry ? [{ ...d, entry }] : [];
  });
}

/** llms.txt: an index of the site in the llmstxt.org shape. */
export async function llmsIndex(): Promise<string> {
  const docs = await orderedDocs();
  const lines = [
    "# Kleene",
    "",
    "> Relational algebra for recursive model calls. Write declarative SQL (CallSQL); Kleene compiles joins, recursion, predicates and aggregation into a planned, budgeted execution graph of language-model calls, recursive sub-sessions and tool calls, runs it, and lets you query the trace with the same SQL. A Rust workspace with DuckDB inside, a `kleene` CLI and a terminal UI. MIT licensed, by Marcus Elwin.",
    "",
    "Source: https://github.com/MarcusElwin/kleene. The documentation pages below are the repository's own docs/*.md; each is also served as raw Markdown at the .md URL.",
    "",
    "## Documentation",
    "",
    ...docs.map((d) => `- [${d.title}](${SITE}/docs/${d.slug}.md): ${d.blurb}`),
    "",
    "## Site",
    "",
    ...PAGES.map((p) => `- [${p.title}](${SITE}${p.path}): ${p.blurb}`),
    "",
    "## Optional",
    "",
    `- [Everything in one file](${SITE}/llms-full.txt): all documentation pages concatenated.`,
    `- [README](https://raw.githubusercontent.com/MarcusElwin/kleene/main/README.md): the repository README with install, examples and status.`,
    "",
  ];
  return lines.join("\n");
}

/** llms-full.txt: every documentation page, in index order, as Markdown. */
export async function llmsFull(): Promise<string> {
  const docs = await orderedDocs();
  const parts = [
    "# Kleene documentation",
    "",
    `Relational algebra for recursive model calls. Site: ${SITE}. Source: https://github.com/MarcusElwin/kleene.`,
    "",
  ];
  for (const d of docs) {
    parts.push(`<!-- ${SITE}/docs/${d.slug}.md -->`, "", (d.entry.body ?? "").trim(), "", "");
  }
  return parts.join("\n");
}

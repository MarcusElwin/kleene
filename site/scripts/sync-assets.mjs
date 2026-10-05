// Copies the repository's screenshots and plots into public/ so the pages and
// the rendered docs can reference them. Runs before `astro dev` and `astro build`.
import { cpSync, existsSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const site = dirname(dirname(fileURLToPath(import.meta.url)));
const repo = dirname(site);
const pairs = [
  [join(repo, "docs", "screenshots"), join(site, "public", "screenshots")],
  [join(repo, "plots"), join(site, "public", "plots")],
];
for (const [from, to] of pairs) {
  if (!existsSync(from)) continue;
  mkdirSync(dirname(to), { recursive: true });
  cpSync(from, to, { recursive: true });
}

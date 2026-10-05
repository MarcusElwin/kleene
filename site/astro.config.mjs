import { defineConfig } from "astro/config";
import { visit } from "unist-util-visit";

// Docs are rendered straight from ../docs/*.md. Their links are written for
// GitHub, so rewrite them: another doc becomes /docs/<slug>, a screenshot
// becomes /screenshots/<file>, anything else in the repository points at GitHub.
const REPO = "https://github.com/MarcusElwin/kleene/blob/main";
function rewriteDocLinks() {
  return (tree) => {
    visit(tree, "element", (node) => {
      const key = node.tagName === "a" ? "href" : node.tagName === "img" ? "src" : null;
      if (!key) return;
      const url = node.properties?.[key];
      if (typeof url !== "string" || /^(https?:|mailto:|#|\/)/.test(url)) return;
      const m = url.match(/^(?:\.\/)?([A-Z]+)\.md(#.*)?$/);
      if (m) {
        node.properties[key] = `/docs/${m[1].toLowerCase()}${m[2] ?? ""}`;
      } else if (url.startsWith("screenshots/")) {
        node.properties[key] = `/${url}`;
      } else if (url.startsWith("../README.md")) {
        node.properties[key] = `${REPO}/README.md${url.slice("../README.md".length)}`;
      } else {
        node.properties[key] = `${REPO}/${url.replace(/^\.\.\//, "").replace(/^\.\//, "docs/")}`;
      }
    });
  };
}

// Shiki has no "mermaid" grammar and would mark the fence plaintext, losing the
// signal the client script keys on. Emit the fence as raw HTML instead.
function mermaidFences() {
  const esc = (t) => t.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  return (tree) => {
    visit(tree, "code", (node, index, parent) => {
      if (node.lang !== "mermaid" || !parent || index == null) return;
      parent.children[index] = { type: "html", value: `<pre data-language="mermaid"><code class="language-mermaid">${esc(node.value)}</code></pre>` };
    });
  };
}

export default defineConfig({
  site: "https://kleene.dev",
  markdown: {
    remarkPlugins: [mermaidFences],
    rehypePlugins: [rewriteDocLinks],
    shikiConfig: { theme: "vitesse-black" },
  },
});

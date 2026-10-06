import type { APIRoute } from "astro";
import { orderedDocs } from "../../lib/llms";

// Each documentation page as the raw Markdown it is rendered from, for agents and llms.txt.
export async function getStaticPaths() {
  const docs = await orderedDocs();
  return docs.map((d) => ({ params: { slug: d.slug }, props: { body: d.entry.body ?? "" } }));
}

export const GET: APIRoute = ({ props }) =>
  new Response(props.body, { headers: { "Content-Type": "text/markdown; charset=utf-8" } });

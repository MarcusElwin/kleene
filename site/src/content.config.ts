import { defineCollection, z } from "astro:content";
import { glob } from "astro/loaders";

// The documentation pages are the repository's own docs/*.md, read in place.
export const docs = defineCollection({
  loader: glob({ pattern: "*.md", base: "../docs" }),
  schema: z.object({}).passthrough(),
});

export const collections = { docs };

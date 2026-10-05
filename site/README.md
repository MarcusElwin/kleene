# kleene.dev

The project website: landing page, technical overview, about page and the
documentation rendered from [`../docs`](../docs). An [Astro](https://astro.build)
site; the documentation pages are a content collection whose loader reads
`../docs/*.md` in place, so a doc change on `main` is a site change.

```bash
npm install
npm run dev      # http://localhost:4321
npm run build    # static output in dist/
```

`scripts/sync-assets.mjs` runs before `dev` and `build` and copies
`../docs/screenshots` and `../plots` into `public/`, so the pages and the
rendered docs can show them. Those copies are git-ignored.

## Deploying on Vercel

Import the repository and set **Root Directory** to `site`. Vercel detects
Astro and needs no other setting: build command `npm run build`, output
`dist`. The `site` in `astro.config.mjs` is the canonical URL used for
`og:` tags and should be changed once a domain is attached.

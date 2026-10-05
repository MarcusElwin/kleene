# kleene.sh

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

The site is served at [kleene.sh](https://kleene.sh) from Vercel.

1. In Vercel, **Add New > Project**, import `MarcusElwin/kleene`, and set
   **Root Directory** to `site`. Vercel detects Astro: build command
   `npm run build`, output `dist`, install `npm install`. Deploy.
2. **Settings > Domains**, add `kleene.sh` and `www.kleene.sh`, with `www`
   redirecting to the apex. Vercel shows the records to create.
3. At the registrar, point the apex at Vercel with an `A` record to
   `76.76.21.21` and `www` with a `CNAME` to `cname.vercel-dns.com`, or move
   the nameservers to `ns1.vercel-dns.com` and `ns2.vercel-dns.com` and let
   Vercel manage the zone. Vercel issues the TLS certificate once the
   records resolve.
4. Every push to `main` that touches `site/` redeploys production; every
   pull request gets a preview URL. To skip deploys for commits that touch
   nothing under `site/`, set **Settings > Git > Ignored Build Step** to
   `git diff --quiet HEAD^ HEAD -- .` (it runs inside the root directory).

`site` in `astro.config.mjs` is the canonical origin: it fills the
`canonical` link, the `og:url` tags and `sitemap-index.xml`.

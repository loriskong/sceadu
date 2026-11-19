# Website

The Sceadu promotional site is a static site in `site/`, deployed to GitHub
Pages by `.github/workflows/pages.yml`. There is no build step, no framework, and
no JavaScript runtime dependency. `site/` is uploaded as the published artifact.

Live URL: <https://loriskong.github.io/sceadu/>

## Why GitHub Pages is enough

The site is HTML, CSS, two WOFF2 fonts, an SVG mark, a 700 KB demo GIF, two short
video files, and a poster (about 1.3 MB of media total). It has no server-side rendering, no
database, and no API. GitHub Pages serves all of it over HTTPS with range
requests for the video, so the 4-core Tencent Cloud host is not needed.

Pages limits that matter here:

- published site: 1 GB (this site is about 2 MB);
- file size: 100 MB (largest file is the GIF at about 700 KB);
- soft bandwidth: 100 GB per month — far above what a pre-alpha project page uses;
- build frequency: 10 per hour (a workflow run only stages static files).

## First deployment

1. Push to `main` with changes under `site/` or run the `pages` workflow manually.
2. If the job fails at `Configure Pages`, open
   **Settings -> Pages -> Build and deployment** and set **Source** to
   **GitHub Actions**, then re-run the workflow.

The workflow requests `pages: write` and `id-token: write`. Enablement may require
repository admin rights; a maintainer may need to click the Pages source once.

## Local preview

From the repository root:

```sh
python3 -m http.server 8000 --directory site
```

Open <http://127.0.0.1:8000/>. The site uses relative asset paths, so it works
both at `/` locally and at `/sceadu/` on GitHub Pages.

`site/404.html` uses `<base href="/sceadu/">` because it can be served at any
unknown path. If the repository is renamed or moved to a custom domain, update
that base, the canonical and Open Graph URLs, `site/sitemap.xml`, and
`site/robots.txt`.

## Content and assets

- `site/index.html` — landing page.
- `site/install/index.html` — human-readable install guide.
- `site/install.md` — canonical agent-readable guide, served at `/install.md`.
- `site/llms.txt` — short machine-readable index.
- `site/assets/css/site.css` — design tokens and landing page styles.
- `site/assets/css/install.css` — install guide layout.
- `site/assets/js/site.js` — navigation, copy buttons, reveal, hero mesh, video.
- `site/assets/fonts/` — self-hosted Space Grotesk and JetBrains Mono (SIL OFL);
  the license files are next to the fonts.
- `site/assets/media/` — `demo.gif`, `demo.mp4`, `demo.webm`, `demo-poster.png`.
  The page uses the videos; the GIF is the shareable fallback and is linked from
  the caption.

The demo is an illustrative terminal session built from the flow in
[`docs/demo-tennis.md`](demo-tennis.md). Identifiers and addresses are shortened.
It is not a recording of a live public network, and the page says so.

When replacing the demo, keep the 1000x700 aspect ratio, update all four files,
and keep the poster as the first frame users see. The page prefers WebM, then
MP4, and falls back to the GIF.

## Accessibility and motion

- All decorative canvas, grain, and scanlines are `aria-hidden`.
- The demo video is muted, looping, and has a visible pause control; it pauses
  automatically when `prefers-reduced-motion: reduce` is set.
- The terminal transcript is available as text under the demo.
- The site is keyboard navigable and has a skip link.

## Custom domain

Not configured. To add one later:

1. Create `site/CNAME` with the domain.
2. Point the DNS records at GitHub Pages (see GitHub's custom domain docs).
3. Update the canonical, Open Graph, sitemap, robots, and 404 base URLs from
   `https://loriskong.github.io/sceadu/` to the new origin.
4. Enable **Enforce HTTPS** in Settings -> Pages.

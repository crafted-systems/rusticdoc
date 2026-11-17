# rusticdoc.com

The marketing site and documentation, built with [Astro](https://astro.build) and [Starlight](https://starlight.astro.build). The output in `dist/` is static.

```bash
npm ci
npm run dev        # http://localhost:4327
npm run build
```

## Where things live

| Path | Holds |
|---|---|
| `src/pages/` | Marketing pages |
| `src/content/docs/docs/` | Documentation, served under `/docs/` |
| `src/content/docs/docs/reference/` | `api/` and `cli.md` are generated, so do not edit them |
| `src/site.ts` | The version and image name the marketing pages use |
| `samples/` | The invoice shown across the site, and how to rebuild it |

## Writing docs

- Say only what the code does and what has been run. Anything unproven goes in `docs/project/known-limits.md`, not in a promise.
- Plain words, sentence case, active voice. No superlatives, and no comparisons that were not measured.
- Each page has `title`, a one-sentence `description` and `sidebar: { order: n }` in its frontmatter.
- Add `test` to the opening fence of a shell example, as in ` ```bash test `, and `npm run test:examples` runs it against a real service.

## Tested examples

`npm run test:examples` starts the image (`IMAGE`, default `rusticdoc:site`) on port 9027 with a file store and no credentials, and runs each `test` block in a new directory holding these files:

| File | What it is |
|---|---|
| `invoice.docx`, `invoice.json` | The sample template and its data |
| `letter.txt`, `letter.json` | `Dear {{ customer.name }},` and a total, and data for it |
| `report.docx`, `deck.pptx`, `sheet.xlsx` | Ordinary documents with no tags |
| `a.pdf`, `b.pdf` | One-page PDFs |
| `form.pdf` | A PDF with text fields `name` and `date` and a checkbox `agree` |
| `brand-fonts.zip`, `font.ttf` | A ZIP holding one TrueType font, and that font |
| `logo.png` | A small PNG |

Blocks run under `bash -euo pipefail`, one block per shell, with `curl` set to fail on an HTTP error, so a block has to make its own state. `KEY` is set; the service ignores it because it has no credentials configured. Add `test=fail` to a block that must fail, to show an error response.

## Stale code block styles

Astro caches rendered docs in `.astro/` and `node_modules/.astro/`, including the address of the code block stylesheet. After changing `expressiveCode` in `astro.config.mjs`, delete both and rebuild, or code blocks lose their styling and `npm run check:links` reports the old stylesheet as missing.

## Social image

`public/og.png` is rendered from `scripts/og.html`. After changing it, run this from `site/` with Chrome installed (the path is for macOS):

```bash
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless=new --hide-scrollbars --allow-file-access-from-files --force-device-scale-factor=1 --window-size=1200,630 --screenshot="$PWD/public/og.png" "file://$PWD/scripts/og.html"
```

## Blog

Posts are Markdown files in `src/content/blog/`. A post's address is the year of its `date` and its file name: `introducing-rusticdoc.md` dated 2026 is `/blog/2026/introducing-rusticdoc/`, and `/blog/2026/` lists that year's posts. The frontmatter needs `title`, `description`, `date`, `cover` and `coverAlt`, and takes an `author`, which is otherwise the rusticdoc team, and a `tag`, the small label above the title such as Announcement. `draft: true` keeps a post out of the built site.

A post's cover is shown on its card, at the top of the post and as its share image. Each one is rendered from an HTML file in `scripts/blog-covers/`, at twice its 1200 by 630 size so it stays sharp. For `introducing-rusticdoc`, run this from `site/`:

```bash
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless=new --hide-scrollbars --allow-file-access-from-files --force-device-scale-factor=2 --window-size=1200,630 --screenshot="$PWD/src/assets/blog/introducing-rusticdoc.png" "file://$PWD/scripts/blog-covers/introducing-rusticdoc.html"
```

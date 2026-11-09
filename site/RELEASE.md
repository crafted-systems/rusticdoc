# Before rusticdoc.com goes live

The site builds and is ready to host, but the things below can't be done from this repository. They are in the order that matters.

## Must happen first, or the site's instructions fail

- [ ] **Register `rusticdoc.com`.** A WHOIS lookup on 6 November 2026 found it unregistered.
- [ ] **Make `github.com/vmosoti/rusticdoc` public.** The repository is private, so every GitHub link on the site returns 404 to visitors, and GitHub Pages needs a paid plan while it stays private.
- [x] **Publish the image as `ghcr.io/vmosoti/rusticdoc:0.1.0`.** `.github/workflows/release.yml` builds amd64 and arm64 on a `v0.*` tag and pushes both under the version. The package stays private while the repository is.
- [x] **Commit the storage and API work.**
- [x] **Tag `v0.1.0` and date the changelog** in `src/content/docs/docs/project/changelog.md`. Versions follow ZeroVer.
- [x] **Remove the "not published yet" notes.** The docs now say the image is private while the repository is; change that when it is made public.

## Fix or accept these product bugs

Writing and testing the docs turned these up. The docs tell readers about those that affect them.

Fixed, with tests: `to=txt` filling `/tmp`, values not escaped in text templates, and `-F data=@file` being ignored.

Not fixed. Reproduced twice, once by the docs author and once separately:

- **`openapi.json` names `user_password` and `owner_password`; the service reads `userPassword` and `ownerPassword`.** It also marks `template` and `files` as required on `/v1/render`, and lists only some of the steps. `generate-reference.mjs` corrects the names and required fields for the site, but generated SDKs will be wrong.

Reported by the docs authors with a reproduction, and not re-run separately:

- A `413` from a template answers `"error":"reported"`.
- The access log's `bytes_out` is always 0.
- Unparseable sizes and durations, such as `RUSTICDOC_RENDER_MEMORY=2g`, are ignored without a warning, and `RUSTICDOC_METRICS_PUBLIC=TRUE` reads as off.
- A private `downloadFrom` error names the webhook variable.
- `steps=watermark,watermark` with two sources draws four.
- A bookmark for page 99 of a one-page PDF lands on page 1.
- A `501 not_supported` hides its reason.
- `{% raw %}` in an Office template prints stray `{# #}`.
- An ODS loop across sheets duplicates sheets with `200`.
- Validation stops at the first undefined value in an expression group, and reports `ok:false` for PDFs.
- Generated TypeScript and Go SDKs don't compile.
- `deploy/kubernetes.yaml` uses `:latest`, and the `deploy/compose.yaml` memory limit leaves out the 2 GiB body budget and `/tmp`.

README corrections the docs already reflect: webhooks make 3 attempts in total rather than 3 retries; `date`, `t` and `{% settings %}` work only in Office templates; not every error on `/v1/*` is JSON; and several `RUSTICDOC_*` variables are missing from its table.

## Host the site

Output is static. Pick one:

- **Cloudflare Pages or Netlify (recommended):** root `site`, build `npm ci && npm run build`, output `dist`. Both apply `public/_headers`, which carries the Content-Security-Policy and other security headers.
- **GitHub Pages:** set the repository variable `SITE_DEPLOY` to `github-pages` and the Pages source to GitHub Actions. Until `rusticdoc.com` is live the workflow builds the site under `/rusticdoc` for `https://vmosoti.github.io/rusticdoc/` (`scripts/rebase.mjs`) and leaves out `public/CNAME`. Pages needs the repository public, or a plan that allows Pages for private repositories. Pages ignores `public/_headers`, so those headers will be missing.

Then point the domain's DNS at the host and turn on HTTPS.

## Decide before announcing

- [ ] **Check the claims on `/status/` are still true**, and update it as Office and PDF viewer checks finish.
- [ ] **Check the name "rusticdoc" for conflicts.**
- [ ] **Add a security contact.** `/.well-known/security.txt` and a contact on `/security/` need a real address, so none was invented.
- [ ] **Decide on analytics.** The site has none, and `_headers` would need changing to add a third-party one.

## After each release

```bash
npm run reference            # needs the release image, as IMAGE
npm run test:examples        # needs the release image, as IMAGE
npm run build && npm run check:links && npm run check:config
```

Update `VERSION` in `src/site.ts`, and the image tag in the docs.

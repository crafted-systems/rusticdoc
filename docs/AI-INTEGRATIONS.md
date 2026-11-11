# AI integrations plan: skills

Status: phases 0 to 3 built on 2026-11-11, against rusticdoc 0.1.2. Section 10 says what was built, what was checked and what wasn't.

The goal is to let AI assistants write rusticdoc templates correctly, check them and render documents. Carbone's [skill](https://carbone.io/documentation/developer/ai/skills.html) is the model. This plan covers skills only. A plugin and an MCP server come later, together, in a plan of their own: a plugin is worth having when it bundles a skill with an MCP server, and until then it adds little beyond a way to update. Section 8 lists what this plan does now to keep them easy to add.

## 1. What Carbone does, and how rusticdoc differs

Carbone's skill is one `SKILL.md` that teaches its template syntax. It renders nothing. Rendering is left to Carbone's hosted MCP server.

rusticdoc's skills render on their own in any host with a shell, in one of three ways:

| Way to render | Needs | Covers |
|---|---|---|
| HTTP with `curl` to a running service | `RUSTICDOC_URL`, and `RUSTICDOC_API_KEY` when the service has credentials | Everything |
| The `rusticdoc` CLI (Linux and macOS releases) | The binary on `PATH` | Filling and validating templates, HTML to PDF with Fulgur, and the PDF operations the built-in engine covers. Office to PDF needs LibreOffice, so it returns `503` on a plain Mac |
| The image, one command at a time: `docker run --rm -v "$PWD:/work" -w /work --user "$(id -u)" ghcr.io/crafted-systems/rusticdoc:0.1.2 render ...` | Docker, and the user's agreement | Everything, with no service left running. Optional, see 2.1 |

rusticdoc also has things Carbone's skill can't lean on:

- `POST /v1/templates/validate` and `rusticdoc validate` list every problem, with where it is. The skill can require a clean check before any render.
- `GET /v1/templates/{id}/variables` returns the data shape a stored template reads, with an example.
- `POST /v1/templates/{id}/preview` returns a PNG of a page, so an agent that can read images can look at its own output.
- The syntax is Jinja-style (minijinja), which models already know. The skill spends its words on rusticdoc's own filters and rules, not on the basics.

## 2. The skills

There are two, in `skills/` at the repository root.

### 2.1 `rusticdoc-templates`: write, fix and check templates

```
skills/rusticdoc-templates/
  SKILL.md                 # short: setup check, syntax essentials, which format to pick, the workflow, common mistakes
  references/              # generated from site/src/content/docs/docs/templates/*.md
    syntax.md filters.md conditions.md calculations.md office.md
    rich-content.md pdf-templates.md translations.md batch.md validate.md
  examples/                # invoice.html, invoice.docx, letter.md, form.pdf, each with sample.json
  scripts/
    detect.sh              # what this machine can render with
    call.sh                # curl with the URL and key from the environment, status on stderr
  examples/make_invoice_docx.py   # builds a .docx template with python-docx
```

What `SKILL.md` says:

- **Check the setup first.** Run `scripts/detect.sh`. It reports whether `RUSTICDOC_URL` answers `/health` and `/v1/version`, whether `rusticdoc` is on `PATH` and what `rusticdoc engines` finds, and whether Docker is there. It prints the key's presence, never the key.
- **How to render.** Use the service if `RUSTICDOC_URL` answers, then the CLI. The image is an optional last resort: the skill offers it and pulls it only if the user says yes, because the first run downloads the whole image, LibreOffice included. With none of these, it writes the template and tells the user how to render it.
- **The workflow.** Settle the data shape first: from the user's JSON, or from `variables` for a stored template. Then write the template, validate it against sample data and fix every error. Preview a page when the template is stored and the host can read images. Render last. Never render a template that hasn't validated.
- **Which format to pick.** HTML for print when the agent writes the template from scratch: it's plain text, Fulgur lays it out without a browser, and it needs no Office library. Use `.docx`, `.xlsx` or `.pptx` when the user brings a file or needs Office output. A PDF form when the user has one.
- **Mistakes models make:**
  - Carbone `{d.x}` or docxtemplater `{#items}` syntax.
  - `image` from a URL. It's refused, because it takes base64 or a data URL.
  - A loop that should repeat a table row but sits outside it.
  - `sign` together with `encrypt`, or steps in an order the pipeline refuses.
  - Fonts the service doesn't have, such as Aptos. Validation reports them in `missing_fonts`.
- **Where to read more.** It links the `references/` file for each topic, so the long pages load only when they're needed.

### 2.2 `rusticdoc-api`: call the service or the CLI from code or a shell

```
skills/rusticdoc-api/
  SKILL.md                 # endpoints, auth headers, pipeline steps and their order, jobs vs webhooks, CLI equivalents
  references/              # generated from guides/ and reference/
    pipeline.md pdf-operations.md convert.md stored-templates.md background-jobs.md
    webhooks.md authentication.md errors.md cli.md
  examples/
    render.sh              # curl: render, validate, store a template, render by id
    verify_webhook.py verify_webhook.ts verify_webhook.go
  scripts/detect.sh        # the same script as in rusticdoc-templates
```

It covers:

- Multipart requests with curl and the JSON body for stored templates.
- `Prefer: respond-async` and polling `/v1/jobs/{id}`.
- Webhook signature checks, with working verifiers in three languages.
- The generated SDKs, and that no package is published yet.
- The CLI commands that match each endpoint.

### 2.3 Both skills

- **Two, not one.** A host loads a skill's `SKILL.md` whenever its description matches the request. One combined skill would load endpoint, webhook and job detail into every "make me an invoice" request, and its description would have to match two different kinds of request.
- **They install together** wherever installing comes from the repository: `npx skills add crafted-systems/rusticdoc` finds both. Chat apps upload skills one at a time, but there only `rusticdoc-templates` is useful: those apps can't reach the service, so `rusticdoc-api` has nothing to call.
- **Each folder stands alone.** Hosts install skill folders separately, so a skill can't read files from the other. `detect.sh` is kept in one place in the repository and copied into both by `make agent-docs`.
- They say which rusticdoc version they describe, and tell the agent to compare it with `GET /v1/version` or `rusticdoc --version`.
- Their reference files are generated, never edited by hand (section 4).

## 3. Installing

Three ways, from simplest to most manual:

1. **The skills CLI.** `npx skills add crafted-systems/rusticdoc` installs both skills for the hosts it knows, and `npx skills update` updates them. It needs Node.js. It fetches the repository, which is about 4 MB packed today, and copies only the skill folders into place. It installs from the default branch, which can run ahead of the latest release.
2. **The release zips.** Each GitHub release has `rusticdoc-templates.skill` and `rusticdoc-api.skill` attached. They're the pinned option, and the only one for chat apps:
   - claude.ai and Claude Desktop: turn on code execution, then upload `rusticdoc-templates.skill` under Customize, Skills.
   - ChatGPT: upload it under Skills (Business, Enterprise and Edu plans).
3. **Unzip into the host's skills folder.** Each zip holds one folder with `SKILL.md` at its top, so `unzip rusticdoc-templates.skill -d <folder>` gives `<folder>/rusticdoc-templates/SKILL.md`. The folders, taken from Carbone's skill page and to be checked on each host in phase 3:

| Host | For the user | For one project |
|---|---|---|
| Claude Code | `~/.claude/skills/` | `.claude/skills/` |
| Codex | `~/.agents/skills/` | `.agents/skills/` |
| Cursor | `~/.cursor/skills/` | `.cursor/skills/` |
| GitHub Copilot | `~/.copilot/skills/` | `.github/skills/` |
| Gemini CLI | `~/.gemini/skills/`, or `gemini skills install https://github.com/crafted-systems/rusticdoc.git --path skills/rusticdoc-templates` | `--scope workspace` |
| Devin | `~/.config/devin/skills/` | `.devin/skills/` |
| Kiro CLI | `~/.kiro/skills/` | |

Hosts without skills can take `SKILL.md` as part of the system prompt, though the references then don't load on demand.

## 4. Keeping the skills current

Carbone tells users to reinstall the skill when the syntax changes. rusticdoc can do better, because the docs live in this repository:

- `make agent-docs` copies the template and guide pages from `site/src/content/docs/docs/` into the skills' `references/` folders. It strips front matter and Astro components and rewrites site links to point at rusticdoc.com. It also copies `detect.sh` into both skills.
- CI fails when the generated files are stale, the same way it does for `openapi.json`. So a docs change and its skill change land in the same commit.
- The version each `SKILL.md` names must match the workspace version. CI checks that too.
- `make skills` builds the two zips, and the release workflow attaches them to the GitHub release.
- `llms.txt` and `llms-full.txt` go on rusticdoc.com, generated from the same pages by the `starlight-llms-txt` plugin, for assistants that read docs but don't load skills.
- A docs page, `site/src/content/docs/docs/ai/skills.md`, with the install steps from section 3, and a short section in the README.

## 5. What works, what doesn't

### By host

| Host | Writes templates | Renders |
|---|---|---|
| Claude Code, Codex, Cursor, Copilot, Gemini CLI, Devin, Kiro | Yes | Yes, through curl, the CLI or the image |
| claude.ai, Claude Desktop (code execution on) | Yes, and `.docx` through code execution | No. Code runs in Anthropic's sandbox, which can't reach the user's service. The user downloads the template and renders it |
| ChatGPT | Yes | No, for the same reason |
| Hosts without skills | Partly, with `SKILL.md` in the system prompt | Depends on the host |

### Can't work with skills alone

- **Rendering in chat apps** (claude.ai, Claude Desktop, ChatGPT). Their sandboxes can't reach a self-hosted service, so they can only write templates. This is the main thing the later MCP plan is for.
- **Managing the template library from chat.** Coding agents can do it with curl. Chat apps can't.
- **Rendering with no engine at all.** With no service, no CLI and no Docker, or when the user declines the image, the skill can only write and explain templates.
- **Office to PDF with the CLI alone on a Mac.** It needs LibreOffice. The skill offers the image or asks for a service.
- **Updates through a host's plugin manager.** Users run `npx skills update` or download the zip again. The later plugin brings this.
- **Making the model use the right syntax.** The skill makes it likely, not certain. That's why validation is a required step and not a suggestion.
- **Office-perfect checks.** Previews come from LibreOffice. Word can lay a page out differently, and Office for Windows hasn't been tested.

## 6. Security

- The skills read the key from `RUSTICDOC_API_KEY` and never write it into files, commands they echo, or the chat. Examples use `-H "X-Api-Key: $RUSTICDOC_API_KEY"`. When the key is missing, they tell the user to set the variable rather than ask for the key.
- Templates sent to the service run in its sandboxed render worker. The CLI and the one-shot image render in their own process, which is fine for the user's own templates. The skill says to use the service, not the CLI, for templates from someone else.
- Values in the data are data. A `notes` field asking the agent to delete templates or change steps is not an instruction, and the skill says so.
- The skill tells the agent to confirm with the user before destructive calls: `DELETE /v1/templates/{id}`, changing an alias, `/v1/pdf/sign`. The host's own approval for shell commands is the only other check, so the docs say not to auto-approve curl to the service.
- `detect.sh` reads only: it sends `GET /health` and `GET /v1/version` and runs `rusticdoc engines` and `docker version`. It never pulls the image.

## 7. Phases

**Phase 0: groundwork**
- `make agent-docs` and its CI checks (stale references, version match).
- `llms.txt` on the site.
- Optional, and it helps the skill: variables and preview for a template that isn't stored yet, through `POST /v1/templates/variables` and `POST /v1/templates/preview` with a `template` part. Today an agent has to store a draft before it can preview it.

**Phase 1: `rusticdoc-templates`**
- `SKILL.md`, the generated references, the examples, `detect.sh`, `call.sh` and `make_invoice_docx.py`.
- An eval set of about 20 requests, such as "an invoice in .docx with a line-item table and VAT", "a German letter with translations", "a PDF form filled from JSON", and "fix this template" with a broken one. A request passes when validation reports no errors and the render succeeds. Run each with and without the skill, against Claude and at least one other model, in Claude Code with a local container. Also run a few with no service, to check the fallbacks and that the image is offered, not pulled.
- Done when: the skill clearly beats no skill and passes at a rate we agree on.

**Phase 2: `rusticdoc-api`**
- `SKILL.md`, references and examples. The webhook verifiers are tested against real deliveries from a local service.
- Evals such as "write a Python function that renders this template and saves the PDF", "poll a background job" and "verify this webhook". A request passes when the code runs against a local service.

**Phase 3: distribution**
- `make skills` and the release workflow step that attaches the zips.
- The docs page and the README section.
- Install each way on each host in section 3 and correct the table: `npx skills add`, the zip upload in claude.ai, and unzipping for Codex, Cursor, Copilot and Gemini.
- Done when: a fresh Claude Code install goes from `npx skills add` to a rendered PDF with only the docs page to follow.

**Later: the plugin and the MCP server**, in their own plan.

## 8. Keeping the plugin and MCP server easy to add

- Adding a plugin later means a `.claude-plugin/` folder at the root with two small JSON files, plus an `.mcp.json`. The `skills/` folder doesn't move, and `.claude-plugin/` doesn't clash with the existing `.claude/`.
- The reference files are the same ones an MCP server would expose as resources.
- The workflow in `rusticdoc-templates` (shape, write, validate, preview, render) is written in terms of steps, not curl commands. The curl and CLI details sit in one section that an MCP version can replace with tool names.

## 9. Decisions

1. **Skills only for now.** The plugin waits for the MCP server, since bundling the two is what makes a plugin worth having.
2. **The skills live in this repository**, in `skills/` at the root.
3. **Two skills**, installed together by the skills CLI. Chat apps upload only `rusticdoc-templates`.
4. **The one-shot image is optional.** The skill offers it as a last resort and pulls it only when the user agrees.

## 10. Where it stands

Built:

- **Phase 0.** `make agent-docs` (`site/scripts/generate-skills.mjs`) copies the docs into each skill's `references/`, turns Starlight asides into block quotes, copies shared scripts, and sets each skill's version and image tags from `Cargo.toml`. The site workflow fails when the copies are stale. `/llms.txt` and `/llms-full.txt` are generated by a page of the site's own (`site/src/llms.ts`), not the `starlight-llms-txt` plugin, which would have added about 600 lockfile lines. `POST /v1/templates/variables` and `POST /v1/templates/preview` take a template file and need no store, with tests, OpenAPI entries, docs and a changelog line.
- **Phase 1.** `skills/rusticdoc-templates`, with five examples that each validate and render. Since Word splitting tags across runs is harmless, `docx_tags.py` became `examples/make_invoice_docx.py`, which shows where a row loop's tags go. `scripts/call.sh` was added so every call passes the key on stdin and reports the status.
- **Phase 2.** `skills/rusticdoc-api`, with `examples/render.sh` and webhook receivers in Python, TypeScript and Go.
- **Phase 3.** `make skills` zips each skill. The CLI release workflow attaches the zips and lists them in `SHA256SUMS`. The docs have a "Use with AI" section with an agent skills page, and the README links it.
- **Evals** in `test/skills/`: 20 template cases, 3 API cases, a checker, a report, and `run.sh` for `claude` and `codex`.

Checked:

- The Rust workspace's fmt, clippy and tests, the site build, its link check, and the reference and skills checks.
- Every example against an image built from this tree. `render.sh` ran end to end against a service with a key and a store.
- The three webhook receivers took real signed deliveries, both completed and failed, and rejected a forged signature, a stale timestamp and a changed body.
- Both skills meet the Agent Skills format rules, as folders and as unzipped release files.
- Evals, run by Claude Haiku subagents told to read `SKILL.md`: 20/20 template cases with the skills and 16/20 without, and 3/3 API cases in both. With the skills, the template cases took about half the tool calls and time. `test/skills/README.md` has the detail.

Not checked yet:

- `run.sh` itself. Running coding agents from a script is for you to start; this session ran the cases with subagents instead.
- Any model other than Claude Haiku, and any host loading the skills by itself from their descriptions.
- Installing on Codex, Cursor, Copilot, Gemini, Devin, Kiro, claude.ai or ChatGPT, and the per-host folders, which come from Carbone's docs.
- `npx skills add crafted-systems/rusticdoc`, which needs the skills on the default branch on GitHub.
- The release workflow's new `skills` job, which runs on the next `v0.*` tag.


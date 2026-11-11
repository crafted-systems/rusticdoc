---
title: Agent skills
description: Install the rusticdoc skills so AI assistants write correct templates, check them and render documents, in Claude Code, Codex, Cursor, Copilot, Gemini CLI, claude.ai and ChatGPT.
sidebar:
  order: 1
---

The rusticdoc skills teach AI assistants to write templates in rusticdoc's syntax, check them against sample data, and render documents with your service. A skill is a folder with a `SKILL.md` and reference files that an assistant loads when a request needs it. They're in [`skills/`](https://github.com/crafted-systems/rusticdoc/tree/main/skills) in the repository.

| Skill | For |
|---|---|
| `rusticdoc-templates` | Writing, fixing and checking templates: Word, Excel, PowerPoint, OpenDocument, HTML, Markdown, text and PDF forms. It sticks to a workflow (settle the data, write, validate, preview, render), steers to HTML when it writes a document from scratch, and knows the mistakes models make, like Carbone or docxtemplater syntax and images from URLs |
| `rusticdoc-api` | Calling the service or the CLI from code or a shell: endpoints, steps, stored templates, background jobs, webhooks and their signatures, errors and the SDKs. Its examples include working webhook receivers in Python, TypeScript and Go |

Each skill's reference files are the pages of these docs, copied when the docs change, so the skills describe the release they ship with.

## How the skills render

In an assistant with a shell, the skills render by themselves. They run `scripts/detect.sh` first, which only looks and changes nothing, then use what it finds, in this order:

1. **Your service**, at `RUSTICDOC_URL` (default `http://localhost:9027`), with `RUSTICDOC_API_KEY` if it has [credentials](/docs/guides/authentication/).
2. **The CLI**, if `rusticdoc` is on `PATH` ([install](/docs/start/install/)). Without LibreOffice it can't turn Office files into PDF, so the skill says so and moves on.
3. **The image**, only if you agree. The skill asks first, because the first run downloads the whole image, LibreOffice included.

With none of these, the skill writes the template and sample data and tells you how to render them.

Set the variables in the environment the assistant runs in, never in the chat:

```bash
export RUSTICDOC_URL=https://docs.example.com
export RUSTICDOC_API_KEY=...   # only if the service has credentials
```

## Install

### With the skills CLI

```bash
npx skills add crafted-systems/rusticdoc
```

This installs both skills for the assistants it finds on your machine, and `npx skills update` updates them. It needs Node.js, and it installs from the default branch, which can run ahead of the latest release.

### From a release

Every [release](https://github.com/crafted-systems/rusticdoc/releases) from 0.1.4 on has `rusticdoc-templates.skill` and `rusticdoc-api.skill` attached, listed in its `SHA256SUMS`. Each is a zip with one folder, so unzipping it gives `<folder>/rusticdoc-templates/SKILL.md`:

```bash
unzip rusticdoc-templates.skill -d ~/.claude/skills/
unzip rusticdoc-api.skill -d ~/.claude/skills/
```

| Assistant | For you | For one project |
|---|---|---|
| Claude Code | `~/.claude/skills/` | `.claude/skills/` |
| Codex | `~/.agents/skills/` | `.agents/skills/` |
| Cursor | `~/.cursor/skills/` | `.cursor/skills/` |
| GitHub Copilot | `~/.copilot/skills/` | `.github/skills/` |
| Gemini CLI | `~/.gemini/skills/` | `gemini skills install https://github.com/crafted-systems/rusticdoc.git --path skills/rusticdoc-templates --scope workspace` |
| Devin | `~/.config/devin/skills/` | `.devin/skills/` |
| Kiro CLI | `~/.kiro/skills/` | |

Assistants change where they look more often than rusticdoc releases, so check your assistant's own docs if a skill doesn't show up.

### claude.ai, Claude Desktop and ChatGPT

Upload `rusticdoc-templates.skill`. In Claude, turn on code execution, then add it under Customize, Skills. In ChatGPT, add it under Skills, on the plans that have them.

These apps run code in their own cloud sandbox, which can't reach a service on your network. There the skill writes templates and sample data for you to download and render yourself, and `rusticdoc-api` has nothing to call, so there's no need to upload it.

## Check that it works

Ask your assistant something like:

```text
Check what can render rusticdoc templates here, then make an invoice template
from invoice.json and render it to PDF.
```

It should run `detect.sh`, write the template, validate it until it has no errors, and render it. If the assistant doesn't pick up the skill, name it: "Use the rusticdoc-templates skill to …".

## How well they work

The repository has an eval set of 20 template requests in [`test/skills/`](https://github.com/crafted-systems/rusticdoc/tree/main/test/skills): invoices in Word and HTML, an Excel report, a slide deck, a QR certificate, a PDF form, translations, templates ported from Carbone and Mustache, and broken templates to fix. A checker validates and renders what the assistant left behind and reads the result. There are three more requests for `rusticdoc-api`, such as a webhook verifier that's run against real and altered deliveries. `test/skills/README.md` has the latest results and how to run them yourself.

## What they can't do

- **Render in chat apps.** claude.ai, Claude Desktop and ChatGPT can't reach a self-hosted service.
- **Guarantee the syntax.** A skill makes the right syntax likely, not certain. That's why the skill treats validation as a required step.
- **Check Word's layout.** Previews come from LibreOffice, and Word can lay a page out slightly differently.
- **Keep themselves current through a plugin manager.** Update with `npx skills update` or a newer release's zips.

## Security

- The skills read the key from `RUSTICDOC_API_KEY` and never print it, write it to a file or ask for it in the chat. `scripts/call.sh` passes it to curl on standard input, so it stays out of the process list.
- They treat values in data as data, and ask before deleting stored templates, changing aliases or signing.
- Templates from someone you don't trust should go to the service, which renders in a sandboxed worker, not to the CLI. The skill says so.
- An assistant runs its commands with your permissions. Keep your assistant's approval prompts on for commands that call the service, as you would for any script.

## For language models

The docs are also at [`/llms.txt`](/llms.txt), an index, and [`/llms-full.txt`](/llms-full.txt), every page in one Markdown file, for assistants that read docs but don't load skills.

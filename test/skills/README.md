# Skill evals

These check that the agent skills in [`skills/`](../../skills) help. Each case is a request a developer might make, given to a coding agent in an empty folder with the inputs it needs. The agent runs once with the skills installed and once without. `check.py` then judges what it left behind against the service: the template validates against each data file, it renders to PDF, and the output says what the case expects.

- `cases.json`: 20 requests for `rusticdoc-templates`, such as invoices in Word and HTML, an Excel report with a total row, a slide per product, a QR certificate, a logo from data, a flattened PDF form, money and dates, a JSON payload, a multi-page statement, a CSV, a letter in two languages, templates ported from Carbone and Mustache, and a broken template to fix.
- `cases-api.json`: 3 requests for `rusticdoc-api`. They ask for a Python render function that raises on errors, a background-job script, and a webhook verifier. The checker runs the code: the function on a good and a broken template, and the verifier on two real deliveries and on altered copies of them.
- `fixtures/`: the inputs. `fixtures/deliveries/` holds two deliveries captured from a service, and `signing.hex` is that test service's signing key, which signs nothing else.
- `check.py CASES ID FOLDER`: judges one folder and prints a JSON line.
- `report.py RESULTS.jsonl...`: puts runs side by side.
- `run.sh AGENT ARM [CASE...]`: runs cases with `claude` or `codex` headless and checks them (see below).

## Run them

Start a service. The template cases need LibreOffice, so use the image. The API cases need a key and a store:

```bash
docker run -d --rm --name rd-evals -p 127.0.0.1:9027:9027 \
  -e RUSTICDOC_API_KEYS=test-key-0123456789abcdef -e RUSTICDOC_STORE=file:///tmp/store \
  ghcr.io/crafted-systems/rusticdoc:0.1.2
export RUSTICDOC_URL=http://localhost:9027 RUSTICDOC_API_KEY=test-key-0123456789abcdef
```

Then run an agent with and without the skills, and compare:

```bash
test/skills/run.sh claude skill
test/skills/run.sh claude none
MODEL=haiku test/skills/run.sh claude skill
CASES=test/skills/cases-api.json test/skills/run.sh codex skill
python3 test/skills/report.py test/skills/out/*/results.jsonl
```

`run.sh` copies each case's inputs into a fresh folder outside the repository, so an agent without the skills can't find them by looking around. With `ARM=skill` it installs both skills where that agent looks: `.claude/skills/` for Claude Code, `.agents/skills/` for Codex. The agents keep their permission checks, but that isn't a sandbox:

- Claude may use its file tools and run `curl`, the skills' two scripts, `python3`, `rusticdoc`, `ls`, `cat`, `unzip` and `jq` without asking, and other commands are refused. `python3` is allowed because the Word cases build files with python-docx, and it can run any code with your permissions.
- Codex runs in its workspace-write sandbox, which keeps writes to the case folder, with network access turned on so it can reach the service.

Run the evals on a machine or VM you don't mind an agent using.

Read the cases before you run them. Each case's folder, with the agent's transcript, ends up in `test/skills/out/`, which git ignores.

`check.py` needs `curl`, `pdftotext` and `pdfinfo` (poppler).

## Last run

11 November 2026, against an image built from this tree (rusticdoc 0.1.2 plus the draft variables and preview routes). Each case ran in a Claude Haiku subagent of a Claude Code session, with the skill folder in the case folder and the agent told to read its `SKILL.md` first, or with neither. That's close to how a host loads a skill, but it isn't `run.sh`, which hasn't been run yet.

| Run | Template cases | API cases |
|---|---|---|
| Haiku with the skills | 20/20 | 3/3 |
| Haiku without | 16/20 | 3/3 |

The four cases the baseline failed:

| Case | What went wrong without the skill |
|---|---|
| `invoice-docx` | Amounts printed as plain numbers (`28800`), not as money |
| `line-totals` | Amounts as `KES 4,800`, with `number` and the code typed in, instead of `currency` |
| `logo-image` | The logo was pasted into the document once, not printed from the data with `image` |
| `translated` | French wording needed a `translations` file sent with each request, so `lang=fr` alone gave English |

Without the skills, the agents found their way by reading `/openapi.json` and trying things against the service. That's why the baseline is high: these cases had a running service to experiment with. With the skills, the same cases took fewer steps. The template cases averaged 8.4 tool calls and 36 seconds with the skills, against 14 tool calls and 68 seconds without. The API cases took 7 to 8 tool calls with the skill and 7 to 14 without. A case with no service to try things on, as in a chat app, depends on the skill alone.

What these results don't cover:

- Other models. `run.sh` with `codex` is there for it, but it hasn't been run.
- Hosts that load skills by themselves. The subagents were told to read `SKILL.md`, not left to pick the skill from its description.
- How a document looks. The checks read text, page counts and JSON. They don't compare layouts.

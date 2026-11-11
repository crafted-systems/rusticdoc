#!/usr/bin/env bash
# Runs the skill evals: each case's request goes to a coding agent in a fresh
# folder, with the skills installed (ARM=skill) or not (ARM=none), and
# check.py judges what it left behind.
#
#   test/skills/run.sh AGENT ARM [CASE...]
#
#   AGENT  claude or codex, run headless with your own login
#   ARM    skill or none
#   CASE   ids from CASES (default: every case)
#
# The agents keep their permission checks, but this is not a sandbox. Claude may
# use its file tools and run curl, the skills' two scripts, python3, rusticdoc,
# ls, cat, unzip and jq without asking; other commands are refused. python3 is
# there because the Word cases build files with python-docx, and it can run any
# code with your permissions. Codex runs in its workspace-write sandbox, which
# keeps writes to the case folder, with network access turned on so curl can
# reach the service. Run the evals on a machine or VM you don't mind an agent
# using, and read what the cases ask first.
#
# Environment:
#   RUSTICDOC_URL      the service the agents and the checks call (default http://localhost:9027)
#   RUSTICDOC_API_KEY  its key, if it has credentials
#   CASES              the case file (default test/skills/cases.json)
#   MODEL              the agent's model, if not its default
#   JOBS               cases run at once (default 3)
#   TIMEOUT            seconds per case (default 900)
#
# Each case runs in a folder outside the repository, so an agent without the
# skills can't find them by looking around. The folders are copied to
# test/skills/out/<agent>-<arm>[-<model>]/ with the agent's transcript, and
# results.jsonl there has one line per case.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
agent="${1:?usage: run.sh claude|codex skill|none [CASE...]}"
arm="${2:?usage: run.sh claude|codex skill|none [CASE...]}"
shift 2
cases_file="${CASES:-$here/cases.json}"
export RUSTICDOC_URL="${RUSTICDOC_URL:-http://localhost:9027}"
jobs="${JOBS:-3}"
timeout="${TIMEOUT:-900}"
model="${MODEL:-}"

case "$agent" in claude | codex) ;; *) echo "AGENT is claude or codex" >&2; exit 2 ;; esac
case "$arm" in skill | none) ;; *) echo "ARM is skill or none" >&2; exit 2 ;; esac
curl -sf "$RUSTICDOC_URL/health" >/dev/null || { echo "no service at $RUSTICDOC_URL" >&2; exit 1; }

if [ $# -eq 0 ]; then
  set -- $(python3 -c 'import json,sys; print(" ".join(c["id"] for c in json.load(open(sys.argv[1]))["cases"]))' "$cases_file")
fi

label="$agent-$arm${model:+-$model}"
out="$here/out/$label"
rm -rf "$out"
mkdir -p "$out"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# What Claude may do without asking; print mode refuses everything else. bash
# only runs the skills' own scripts. python3 can still run anything.
claude_tools='Read Write Edit Glob Grep Bash(curl:*) Bash(bash .claude/skills/rusticdoc-templates/scripts/detect.sh:*) Bash(bash .claude/skills/rusticdoc-templates/scripts/call.sh:*) Bash(bash .claude/skills/rusticdoc-api/scripts/detect.sh:*) Bash(bash .claude/skills/rusticdoc-api/scripts/call.sh:*) Bash(python3:*) Bash(rusticdoc:*) Bash(ls:*) Bash(cat:*) Bash(unzip:*) Bash(jq:*)'

run_case() {
  local id="$1" dir="$work/$1" started prompt
  mkdir -p "$dir"
  python3 - "$cases_file" "$id" "$here/fixtures" "$dir" <<'EOF'
import json, os, shutil, sys
cases, case_id, fixtures, folder = sys.argv[1:]
case = {c["id"]: c for c in json.load(open(cases))["cases"]}[case_id]
for name in case["inputs"]:
    source = f"{fixtures}/{name}"
    (shutil.copytree if os.path.isdir(source) else shutil.copy)(source, f"{folder}/{name}")
open(f"{folder}/.prompt", "w").write(case["prompt"])
EOF
  prompt="$(cat "$dir/.prompt")

A rusticdoc service is running at $RUSTICDOC_URL${RUSTICDOC_API_KEY:+, and its API key is in the RUSTICDOC_API_KEY environment variable}. Work in the current folder and leave the files named above in it."
  rm "$dir/.prompt"
  git -C "$dir" init -q
  if [ "$arm" = skill ]; then
    local skills="$dir/.claude/skills"
    [ "$agent" = codex ] && skills="$dir/.agents/skills"
    mkdir -p "$skills"
    cp -R "$repo/skills/rusticdoc-templates" "$repo/skills/rusticdoc-api" "$skills/"
  fi
  started=$(date +%s)
  (
    cd "$dir"
    if [ "$agent" = claude ]; then
      perl -e 'alarm shift; exec @ARGV' "$timeout" \
        claude -p "$prompt" --setting-sources project --allowedTools $claude_tools \
        --output-format json ${model:+--model "$model"} >.agent.json 2>.agent.err || true
    else
      perl -e 'alarm shift; exec @ARGV' "$timeout" \
        codex exec --skip-git-repo-check -C "$dir" -s workspace-write \
        -c sandbox_workspace_write.network_access=true ${model:+-m "$model"} \
        -o .agent.last "$prompt" >.agent.log 2>&1 </dev/null || true
    fi
  )
  local seconds=$(($(date +%s) - started))
  local result
  result="$(python3 "$here/check.py" "$cases_file" "$id" "$dir")"
  rm -rf "$dir/.git" "$dir/.claude" "$dir/.agents"
  cp -R "$dir" "$out/$id"
  python3 -c 'import json,sys; r=json.loads(sys.argv[1]); r.update(agent=sys.argv[2], arm=sys.argv[3], model=sys.argv[4] or None, seconds=int(sys.argv[5])); print(json.dumps(r))' \
    "$result" "$agent" "$arm" "$model" "$seconds" >>"$out/results.jsonl"
  echo "$label $id: $(python3 -c 'import json,sys; r=json.loads(sys.argv[1]); print("pass" if r["pass"] else "FAIL " + "; ".join(r["failures"])[:300])' "$result") (${seconds}s)"
}
export -f run_case
export agent arm model timeout cases_file here repo work out label claude_tools

printf '%s\n' "$@" | xargs -P "$jobs" -I{} bash -c 'run_case "$1"' _ {}

python3 "$here/report.py" "$out/results.jsonl"

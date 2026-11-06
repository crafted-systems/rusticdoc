#!/usr/bin/env python3
"""Turns replay.py's results.jsonl into a summary. Usage: summarize.py results.jsonl"""
import collections
import json
import sys

records = {}
for line in open(sys.argv[1]):
    record = json.loads(line)
    records[record["key"]] = record

buckets = collections.defaultdict(list)
per_feature = collections.defaultdict(collections.Counter)
for record in records.values():
    official, slim = record["official"]["ok"], record["slim"]["ok"]
    if official and slim:
        bucket = "same-pass"
    elif official and record["removed_engine"]:
        bucket = "regression-removed-engine"
    elif official and record.get("needs_chromium") and not record["slim"].get("reason", "").startswith("status 2"):
        bucket = "regression-needs-chromium"
    elif official:
        bucket = "regression"
    elif slim:
        bucket = "slim-only"
    else:
        bucket = "both-fail"
    buckets[bucket].append(record)
    per_feature[record["feature"]][bucket] += 1

order = ["same-pass", "regression", "regression-removed-engine", "regression-needs-chromium", "both-fail", "slim-only"]
print(f"scenarios: {len(records)}")
for bucket in order:
    print(f"{bucket}: {len(buckets[bucket])}")

skipped = collections.Counter()
for record in records.values():
    for text in record["slim"].get("skipped", []):
        skipped[text] += 1
if skipped:
    print("\nexpectations this harness cannot evaluate:")
    for text, count in skipped.most_common():
        print(f"  {count}x {text}")

print(f"\n{'feature':45}" + "".join(f"{b.split('-')[0][:10]:>11}" for b in ["same", "regress", "removed", "chromium", "both", "slim"]))
for feature, counts in sorted(per_feature.items()):
    print(f"{feature:45}" + "".join(f"{counts[b]:>11}" for b in order))

for bucket in order[1:]:
    if not buckets[bucket]:
        continue
    print(f"\n=== {bucket}")
    for record in sorted(buckets[bucket], key=lambda r: r["key"]):
        print(f"- {record['key']}")
        side = record["slim"] if bucket != "slim-only" else record["official"]
        print(f"    slim: step {side.get('step')}: {side.get('text')}\n      {side.get('reason')}")
        if bucket == "both-fail":
            print(f"    official: step {record['official'].get('step')}: {record['official'].get('reason')}")

---
title: Background jobs
description: Run a request as a background job, poll it until it finishes, and download the result from any replica.
sidebar: { order: 6 }
---

Run a request as a background job and you get a `202` straight away, with the result kept for you to fetch when it's ready. Use one when a request takes longer than your client wants to wait and you can't host a [webhook](/docs/guides/webhooks/) receiver.

## Turn it on

Jobs keep their state and results in the store, so any replica can serve a job that another one ran. That means you need `RUSTICDOC_STORE`, and for one container a directory will do:

```bash
RUSTICDOC_STORE=file:///data/rusticdoc
```

Without a store, a request that asks for a job gets `501 store_not_configured` and nothing runs. Leave out the header to run it synchronously instead.

## Start a job

Add `Prefer: respond-async` to any request that produces a file: `/v1/render`, `/v1/render/batch`, `/v1/convert` or `/v1/pdf/*`. The rest of the request doesn't change, so it's a multipart form or, for a stored template, [JSON](/docs/guides/stored-templates/#send-json-instead-of-a-form).

```bash test
curl -F template=@letter.txt http://localhost:9027/v1/templates -o stored.json
ID=$(jq -r .id stored.json)

cat > body.json <<EOF
{
  "templateId": "$ID",
  "data": {"customer": {"name": "Ada"}, "total": 1500}
}
EOF
curl -s -D headers.txt -H "Prefer: respond-async" \
  -H "Content-Type: application/json" -d @body.json \
  http://localhost:9027/v1/render -o job.json
grep -qi "^location: /v1/jobs/" headers.txt
jq -e '.status == "queued"' job.json
```

The `202` has the job in its body and its address in `Location`, plus `Preference-Applied: respond-async` and `Retry-After: 1`.

```json
{
  "id": "019a5d2c-4e1f-7b3a-9c2d-0e4f5a6b7c8d",
  "status": "queued",
  "createdAt": "2026-11-09T10:15:02.114Z",
  "runsUntil": "2026-11-09T10:16:32.114Z",
  "requestId": "4740dce6-331f-4c2f-88f4-51d6c1c4b317"
}
```

Mistakes in the request are reported straight away, as they are without the header, so a bad step list or a missing field gets a `400` and starts no job. A failure while the job runs, like a template error, is recorded on the job.

You can't ask for a webhook and a job in one request (you get a `400`). Routes that return JSON, like `/v1/templates/validate`, ignore the header and respond straight away.

## Follow it

`GET /v1/jobs/{id}` tells you where the job is. Poll it until `status` is `done` or `failed`. Until then, each response carries `Retry-After: 1`.

```bash test
curl -F template=@letter.txt http://localhost:9027/v1/templates -o stored.json
curl -s -H "Prefer: respond-async" -F templateId="$(jq -r .id stored.json)" \
  -F "data=<letter.json" http://localhost:9027/v1/render -o job.json
JOB=$(jq -r .id job.json)

for i in $(seq 50); do
  curl -s "http://localhost:9027/v1/jobs/$JOB" -o state.json
  jq -e '.status == "done" or .status == "failed"' state.json > /dev/null \
    && break
  sleep 0.2
done
jq -e '.status == "done" and .result.name == "letter.txt"' state.json

curl -s "http://localhost:9027/v1/jobs/$JOB/result" -o result.txt
grep -q "Dear Ada" result.txt
```

| Field | Meaning |
|---|---|
| `id` | The job's id |
| `status` | `queued`, `running`, `done` or `failed` |
| `createdAt`, `startedAt`, `finishedAt` | RFC 3339, to the millisecond |
| `expiresAt` | When the job and its result are removed. Set once the job ends |
| `runsUntil` | A job that hasn't ended by then never will, and shows as failed |
| `requestId` | The `X-Request-Id` of the request that started it, for the logs |
| `result` | Once done: `name`, `contentType`, `size` and `sha256` of the file |
| `error` | Once failed: `error`, `message` and `status`, as the request would have returned them |

A failed job looks like this:

```json
{
  "status": "failed",
  "error": {
    "error": "template_error",
    "message": "template error: undefined value (in <string>:1) (line 1)",
    "status": 422
  }
}
```

As on a synchronous response, an engine failure is reported by the name of its status, with the detail in the log under `requestId`.

## Download the result

`GET /v1/jobs/{id}/result` returns the file with its type and name, the same way the request would have.

| Status | When |
|---|---|
| `200` | The job is done |
| `409 job_not_done` | It's still queued or running. Check the job first |
| `409 job_failed` | It failed, so there's no file. The error is on the job |
| `404 not_found` | No such job for this caller, or it expired |

## How long a job is kept

A job and its result are kept for `RUSTICDOC_JOB_TTL` (default `1h`) after the job ends. One caller's results may hold `RUSTICDOC_MAX_JOB_STORAGE` (default `5GB`) at a time; a job whose result would go past that fails with `quota_exceeded`. After that, both routes return `404` immediately. Each replica clears expired jobs out of the store every quarter of the TTL, at least 10 seconds and at most 10 minutes apart.

A job runs on the replica that accepted it, so if that replica stops before the job ends, nothing finishes it. Once `runsUntil` passes, the job shows as `failed` with `job_lost` and a `503` status, and it's removed a TTL later. Send the request again.

## Who sees a job

Jobs always belong to the caller that started them, whatever `RUSTICDOC_STORE_SCOPE` says, because results hold the caller's data. Another caller's job id gets a `404`. Every key of a [tenant](/docs/guides/authentication/#tenant-keys) is the same caller, so a job started with one key can be fetched with another.

## Limits

Background jobs and webhook jobs share the same slots:

| Setting | Default | Meaning |
|---|---|---|
| `RUSTICDOC_MAX_ASYNC_JOBS` | `32` | Total webhook and background jobs running at the same time |
| `RUSTICDOC_MAX_ASYNC_JOBS_PER_CALLER` | `8` | The same, for one caller |

Past either limit, a request gets `429 too_many_jobs` before its body is read. A job holds its slot until its result is stored, and it has the same time limit the request would have synchronously: `RUSTICDOC_API_TIMEOUT`, or `RUSTICDOC_BATCH_TIMEOUT` for a batch or a multi-file conversion.

Results don't count towards any other limit, so what the store holds in jobs is whatever callers made within one TTL. If the store can't be reached when a request arrives, the request gets `503 store_unavailable` and no job starts.

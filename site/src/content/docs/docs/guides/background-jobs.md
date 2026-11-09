---
title: Background jobs
description: Run a request in the background with Prefer respond-async, ask where it is, and download the result from any replica.
sidebar: { order: 6 }
---

A background job answers `202` at once and keeps the result for you to fetch. After this page you can start one, follow it to the end, download what it made, and know how long it is kept. Use it when a request takes longer than your client wants to wait and you cannot host a [webhook](/docs/guides/webhooks/) receiver.

## Turn it on

Jobs keep their states and results in the store, so any replica can answer for a job another one ran. They need `RUSTICDOC_STORE`; a directory will do for one container:

```bash
RUSTICDOC_STORE=file:///data/rusticdoc
```

Without a store, a request that asks for a job gets `501 store_not_configured` and nothing runs. Leave out the header to run it synchronously instead.

## Start a job

Add `Prefer: respond-async` to any request that produces a file: `/v1/render`, `/v1/render/batch`, `/v1/convert` and `/v1/pdf/*`. The rest of the request is as usual, a multipart form or, for a stored template, [JSON](/docs/guides/stored-templates/#send-json-instead-of-a-form).

```bash test
curl -F template=@letter.txt http://localhost:9027/v1/templates -o stored.json
ID=$(jq -r .id stored.json)

curl -s -D headers.txt -H "Prefer: respond-async" -H "Content-Type: application/json" \
  -d "{\"templateId\":\"$ID\",\"data\":{\"customer\":{\"name\":\"Ada\"},\"total\":1500}}" \
  http://localhost:9027/v1/render -o job.json
grep -qi "^location: /v1/jobs/" headers.txt
jq -e '.status == "queued"' job.json
```

The `202` carries the job in its body, its address in `Location`, `Preference-Applied: respond-async`, and `Retry-After: 1`.

```json
{
  "id": "019a5d2c-4e1f-7b3a-9c2d-0e4f5a6b7c8d",
  "status": "queued",
  "createdAt": "2026-11-09T10:15:02.114Z",
  "runsUntil": "2026-11-09T10:16:32.114Z",
  "requestId": "4740dce6-331f-4c2f-88f4-51d6c1c4b317"
}
```

A mistake in the request is answered at once, as without the header: a bad step list or a missing field is a `400` and starts no job. A failure while the job runs, such as a template error, is recorded on the job.

Asking for a webhook and a job in one request is a `400`. Routes that answer in JSON, such as `/v1/templates/validate`, ignore the header and answer at once.

## Follow it

`GET /v1/jobs/{id}` says where the job is. Ask again until `status` is `done` or `failed`; while it is neither, the answer carries `Retry-After: 1`.

```bash test
curl -F template=@letter.txt http://localhost:9027/v1/templates -o stored.json
curl -s -H "Prefer: respond-async" -F templateId="$(jq -r .id stored.json)" -F "data=<letter.json" \
  http://localhost:9027/v1/render -o job.json
JOB=$(jq -r .id job.json)

for i in $(seq 50); do
  curl -s "http://localhost:9027/v1/jobs/$JOB" -o state.json
  jq -e '.status == "done" or .status == "failed"' state.json > /dev/null && break
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
| `expiresAt` | When the job and its result are removed. Set once it ends |
| `runsUntil` | A job that has not ended by then never will, and reads as failed |
| `requestId` | The `X-Request-Id` of the request that started it, for the logs |
| `result` | Once done: `name`, `contentType`, `size` and `sha256` of the file |
| `error` | Once failed: `error`, `message` and `status`, what the request would have answered |

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

An engine's failure reads as the status's name, as on a synchronous response, with the detail in the log under `requestId`.

## Download the result

`GET /v1/jobs/{id}/result` answers with the file, its type and its name, as the request would have.

| Status | When |
|---|---|
| `200` | The job is done |
| `409 job_not_done` | It is queued or running. Ask the job first |
| `409 job_failed` | It failed, so there is no file. The job holds the error |
| `404 not_found` | No such job for this caller, or it expired |

## How long a job is kept

`RUSTICDOC_JOB_TTL` (default `1h`) is how long a job and its result are kept after it ends. Past it, both routes answer `404` at once, and every replica removes expired jobs from the store now and then: every quarter of the TTL, between 10 seconds and 10 minutes apart.

A job runs on the replica that accepted it. If that replica stops before the job ends, nothing finishes it: once `runsUntil` passes, the job reads as `failed` with `job_lost` and a `503` status, and is removed a TTL later. Send the request again.

## Who sees a job

Jobs are always the caller's own, whatever `RUSTICDOC_STORE_SCOPE` says, because results hold the caller's data. Another caller's job id answers `404`. Every key of a [tenant](/docs/guides/authentication/#tenant-keys) is the same caller, so a job started with one key can be fetched with another.

## Limits

Background jobs and webhook jobs share the same places:

| Setting | Default | Meaning |
|---|---|---|
| `RUSTICDOC_MAX_ASYNC_JOBS` | `32` | Webhook and background jobs running at once, in all |
| `RUSTICDOC_MAX_ASYNC_JOBS_PER_CALLER` | `8` | The same, for one caller |

Past either, the request gets `429 too_many_jobs` before its body is read. A job holds its place until its result is stored. It has the same time limit as the request would have synchronously: `RUSTICDOC_API_TIMEOUT`, or `RUSTICDOC_BATCH_TIMEOUT` for a batch or a multi-file conversion.

Results count toward nothing else: what the store holds in jobs is what callers made within one TTL. If the store cannot be reached when the request arrives, it gets `503 store_unavailable` and no job starts.

---
title: Observability
description: Follow a request through rusticdoc's logs, metrics, health routes and request ids.
sidebar:
  order: 6
---

## Follow a request by its id

Every response carries `x-request-id`. If you send your own, the service uses it:

```bash test
curl -D - -o /dev/null -H 'x-request-id: order-2041' \
  http://localhost:9027/v1/version
```

The response comes back with `x-request-id: order-2041`. If you don't send one, the service generates it. An id you send is limited to printable ASCII and 128 bytes. The same id shows up in the logs and on webhook deliveries.

When an engine fails, the response only gives the name of the status, like `Service Unavailable`. The detail is in the log, under the request id.

## Logs

Logs are JSON lines on standard output. Set `RUSTICDOC_LOG_FORMAT=pretty` to get text for a terminal instead. `RUSTICDOC_LOG_LEVEL` takes a `tracing` filter like `info` or `rusticdoc=debug`.

```bash
docker logs rusticdoc
```

### Access log

Each request logs one line with `log_type` set to `access`. A status of 500 or above is logged at level `ERROR`, and everything else at `INFO`. Here's one, spread over several lines to fit:

```json
{
  "timestamp": "2026-11-06T20:26:21.925591Z",
  "level": "INFO",
  "message": "request handled",
  "log_type": "access",
  "correlation_id": "0962144d-8980-41a2-9bdf-25ef19e48dd5",
  "remote_addr": "172.17.0.1:55136",
  "caller": "-",
  "host": "localhost:9027",
  "method": "POST",
  "path": "/v1/render",
  "referer": "",
  "user_agent": "curl/8.7.1",
  "status": 200,
  "latency": 1661752043,
  "latency_human": "1.661752043s",
  "latency_ms": 1661,
  "bytes_in": 38426,
  "bytes_out": 0,
  "span": {
    "request_id": "0962144d-8980-41a2-9bdf-25ef19e48dd5",
    "name": "request"
  }
}
```

| Field | Contains |
|---|---|
| `correlation_id` | The request id |
| `remote_addr` | The peer's address. Behind a proxy, that's the proxy |
| `caller` | A short form of the credential, like `key:024a24260d62`. It's `none` when a request was refused for having no credentials, and `-` when the service has none configured. The key itself is never logged |
| `host`, `method`, `path`, `referer`, `user_agent` | Taken from the request |
| `status` | The HTTP status |
| `latency`, `latency_human`, `latency_ms` | Time taken, in nanoseconds, as text, and in milliseconds |
| `bytes_in` | The request's `Content-Length` |
| `bytes_out` | Meant to be the response's `Content-Length`, but it was `0` on all 1,002 access lines checked, including responses that had a length. Don't rely on it |

`/health` and `/ready` are left out of the access log. Every other route is logged, including `/metrics`.

### Audit log

A change to a stored object logs a line with `log_type` set to `audit`:

```json
{
  "timestamp": "2026-11-06T20:30:44.957604Z",
  "level": "INFO",
  "message": "template stored",
  "log_type": "audit",
  "action": "template.upload",
  "caller": "public",
  "template": "01a1b28e-7cdc-7312-9094-7ebc1970f6de",
  "size": 37435,
  "span": {
    "request_id": "2ed44408-4ed4-437c-9aac-9f66a79e827d",
    "name": "request"
  }
}
```

| `action` | Other fields |
|---|---|
| `template.upload` | `template`, `size` |
| `template.delete` | `template` |
| `font.upload` | `font`, `file` |
| `font.delete` | `font` |

`caller` is `public` when the service has no credentials configured.

### Other lines

Each document job logs `pipeline finished` with `output`, `total_ms` and `timings`, and a failed request logs `request failed` with the error and its detail. Webhook attempts log `webhook delivered`, `webhook rejected`, `webhook failed` and `webhook gave up`.

## Metrics

`GET /metrics` serves the Prometheus text format. It's open when no credentials are configured. Once they are, it needs them too, unless you set `RUSTICDOC_METRICS_PUBLIC=true`.

```bash test
curl http://localhost:9027/metrics | grep '^# TYPE'
```

| Metric | Type | Labels | What it measures |
|---|---|---|---|
| `rusticdoc_requests_total` | Counter | `route`, `status` | HTTP requests, by matched route and status |
| `rusticdoc_request_seconds` | Histogram | `stage` | Request latency. The label is called `stage` but holds the route |
| `rusticdoc_stage_seconds` | Histogram | `stage` | How long a pipeline stage took, like `template` or `convert` |
| `rusticdoc_inflight` | Gauge | `stage` | Work in progress. `stage` is `pipeline` |
| `rusticdoc_webhook_deliveries_total` | Counter | `outcome` | Webhook delivery attempts: `delivered`, `failed`, `gave_up` |
| `rusticdoc_store_font_syncs_total` | Counter | `outcome` | Runs of the stored-font sync: `ok`, `changed`, `failed` |

Both histograms have buckets that double from 0.01 to about 82 seconds. A family has no series until something has been counted in it, so `rusticdoc_webhook_deliveries_total` doesn't appear until the first delivery.

A few queries to start from:

| What you want to know | Query |
|---|---|
| Rate of server errors | `sum(rate(rusticdoc_requests_total{status=~"5.."}[5m]))` |
| 95th percentile of conversion time | `histogram_quantile(0.95, sum by (le) (rate(rusticdoc_stage_seconds_bucket{stage="convert"}[5m])))` |
| Webhooks that were given up on | `rate(rusticdoc_webhook_deliveries_total{outcome="gave_up"}[1h])` |
| A replica that can't sync fonts | `rate(rusticdoc_store_font_syncs_total{outcome="failed"}[5m]) > 0` |

## Health and readiness

```bash test
curl http://localhost:9027/health
curl http://localhost:9027/ready
```

| Route | Returns | Use it for |
|---|---|---|
| `/health` | `200` whenever the service is responding | Liveness |
| `/ready` | The same, or `503` with `{"status":"syncing"}` while a replica with a store hasn't synced its fonts yet | Readiness |

A caller the service knows gets the detail, and so does anyone when no credentials are configured:

| Field | Contains |
|---|---|
| `status` | `up` |
| `uptime_seconds` | Time since the service started |
| `engines.libreoffice`, `engines.qpdf` | Whether each engine is configured |
| `details.libreoffice.status` | `up` or `down`, depending on whether it's configured |
| `details.libreoffice.pool` | `workers`, `busy`, `idle`, `started`, `restarts` and `max_jobs` |
| `details.store` | Only with a store: `fontsSynced`, and for an admin, or when no credentials are configured, `location` |

When credentials are configured, a caller without them only sees `{"status":"up"}`.

`engines.libreoffice` tells you LibreOffice is configured. It doesn't tell you a conversion would work, and the way to test that is to send one.

`/v1/version` needs credentials and lists the version and the engines in use.

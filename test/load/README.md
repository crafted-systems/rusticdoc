# Load check

This posts the sample invoice to `/v1/render` with ApacheBench and records how many requests a second the container answers, how long they take and how much memory it uses. The figures on the sizing page come from here.

- `run.sh` starts the container, runs every scenario and writes `out/results.md`.
- `body.py` writes the multipart body that ab posts.
- `report.py` turns the ab reports, the memory samples and the service's own counts into the table.
- `invoice.txt` is the text template. The DOCX template and the data are the site's samples, `site/public/samples/invoice-template.docx` and `invoice.json`.

`make load` (or `test/load/run.sh IMAGE`) needs Docker, curl, unzip, python3 and `ab`, which macOS ships. It takes about eight minutes and installs nothing.

## What a run does

1. It starts the image the way `deploy/compose.yaml` does: read-only, no capabilities, `--memory 6g`, `--pids-limit 512`, 2 GB of `/tmp` and 512 MB of home directory on tmpfs, and the Compose values for the timeout and the workers. The port is published on `127.0.0.1`, on whichever port is free. The container is removed when the script ends.
2. For each scenario it sends one request by hand and checks that the answer is the filled document: the total in the text, the customer's name in the DOCX, `%PDF-` at the start of a PDF, and the PDF/A mark in a PDF/A. A scenario that fails this is skipped, and the script exits with 1.
3. It runs ab with keep-alive at 1, 4, 8 and 16 connections, each for a fixed number of requests that lasts 15 to 30 seconds on the machine below.
4. It samples `docker stats` about twice a second during each run and keeps the highest figure.
5. It reads the service's request counts from `/metrics` and the LibreOffice pool from `/health` before and after each run. A run is clean only when ab reports no failed and no non-2xx responses and the service counted a `200` for every request. Otherwise the table says `run failed` where the rate would be and lists the status codes.

| Scenario | Template | `steps` | LibreOffice jobs per request |
|---|---|---|---|
| `text` | `invoice.txt` | None | 0 |
| `docx` | The sample DOCX | None | 0 |
| `convert` | The sample DOCX | `convert` | 1 |
| `convert-pdfa` | The sample DOCX | `convert,pdfa` | 2 |
| `pdfa` | The sample DOCX | `pdfa` | 1 |

The container's settings come from the environment, under the names Compose uses: `RUSTICDOC_WORKERS`, `RUSTICDOC_LIBREOFFICE_MAX_JOBS`, `RUSTICDOC_RENDER_WORKERS`, `RUSTICDOC_RENDER_MEMORY`, `RUSTICDOC_API_TIMEOUT`, `RUSTICDOC_MEM_LIMIT` and `RUSTICDOC_PIDS_LIMIT`. So `RUSTICDOC_WORKERS=4 make load` measures four LibreOffice workers.

The request counts are at the top of `run.sh`. Change them if a run on your machine falls outside 15 to 30 seconds.

## Last run

10 November 2026.

| | |
|---|---|
| Image | `rusticdoc:local`, ID `9ed8af2f1716`, arm64, rusticdoc 0.1.0, built on 8 November 2026 |
| Machine | Apple M4 Pro, 14 cores (10 performance, 4 efficiency), 24 GB of memory, macOS 27.0.1 |
| Docker | Docker Desktop, engine 29.9.3. Its VM has 8 CPUs and 5.78 GiB of memory |
| Container limits | `--memory 6g`, `--pids-limit 512`, `/tmp` 2 GB, home directory 512 MB. The VM has less than 6 GiB, so `docker stats` showed a limit of 5.783 GiB |
| Settings | `RUSTICDOC_API_TIMEOUT=60s`, `RUSTICDOC_WORKERS=2`, `RUSTICDOC_LIBREOFFICE_MAX_JOBS=10`, `RUSTICDOC_RENDER_WORKERS=4`, `RUSTICDOC_RENDER_MEMORY=1GB`. Everything else at its default, with no credentials, no store and no fonts mounted |
| Client | ApacheBench 2.3 with keep-alive, on the same Mac |
| Document | A one-page invoice: a 37 KB DOCX or a 400-byte text template, with 556 bytes of data. The PDF is 55 KB |

| Template and steps | Connections | Requests/s | Mean, ms | p50, ms | p95, ms | p99, ms | Failed | Peak memory | LibreOffice restarts |
|---|---|---|---|---|---|---|---|---|---|
| Text, no steps | 1 | 508.6 | 2.0 | 2 | 3 | 5 | 0 of 10000 | 86 MiB | 0 |
| Text, no steps | 4 | 1281.4 | 3.1 | 3 | 5 | 8 | 0 of 24000 | 91 MiB | 0 |
| Text, no steps | 8 | 1694.0 | 4.7 | 5 | 6 | 8 | 0 of 30000 | 97 MiB | 0 |
| Text, no steps | 16 | 1591.8 | 10.1 | 10 | 12 | 19 | 0 of 30000 | 97 MiB | 0 |
| DOCX, no steps | 1 | 148.5 | 6.7 | 6 | 8 | 14 | 0 of 3000 | 95 MiB | 0 |
| DOCX, no steps | 4 | 541.2 | 7.4 | 7 | 10 | 13 | 0 of 10000 | 106 MiB | 0 |
| DOCX, no steps | 8 | 629.9 | 12.7 | 12 | 17 | 33 | 0 of 12000 | 110 MiB | 0 |
| DOCX, no steps | 16 | 649.3 | 24.6 | 24 | 30 | 41 | 0 of 12000 | 112 MiB | 0 |
| DOCX, `convert` | 1 | 16.1 | 62.2 | 50 | 155 | 170 | 0 of 300 | 167 MiB | 30 |
| DOCX, `convert` | 4 | 27.9 | 143.5 | 131 | 276 | 341 | 0 of 600 | 226 MiB | 59 |
| DOCX, `convert` | 8 | 27.5 | 290.6 | 202 | 501 | 627 | 0 of 600 | 237 MiB | 60 |
| DOCX, `convert` | 16 | 27.2 | 589.2 | 604 | 722 | 940 | 0 of 600 | 241 MiB | 60 |
| DOCX, `convert,pdfa` | 1 | 7.8 | 128.5 | 107 | 215 | 253 | 0 of 180 | 236 MiB | 37 |
| DOCX, `convert,pdfa` | 4 | 13.3 | 300.8 | 251 | 496 | 588 | 0 of 300 | 243 MiB | 60 |
| DOCX, `convert,pdfa` | 8 | 12.7 | 630.5 | 605 | 850 | 1590 | 0 of 300 | 246 MiB | 60 |
| DOCX, `convert,pdfa` | 16 | 14.2 | 1130.0 | 1154 | 1284 | 1492 | 0 of 300 | 249 MiB | 60 |
| DOCX, `pdfa` | 1 | 15.4 | 65.1 | 52 | 159 | 179 | 0 of 350 | 216 MiB | 35 |
| DOCX, `pdfa` | 4 | 27.4 | 146.1 | 130 | 293 | 389 | 0 of 600 | 244 MiB | 60 |
| DOCX, `pdfa` | 8 | 28.0 | 285.5 | 194 | 494 | 661 | 0 of 600 | 245 MiB | 60 |
| DOCX, `pdfa` | 16 | 24.1 | 663.3 | 586 | 1388 | 2604 | 0 of 600 | 242 MiB | 60 |

Latency is what ab saw, in whole milliseconds, so it includes the time a request waited for a worker.

## What it shows

- **Nothing failed.** All 136,330 requests got a `200`, and no `503` or `429` was seen. That says little about the admission limits, because 16 connections does not reach them. The longest request took 3.2 seconds against a time limit of 60, and `429` is only for webhook and background jobs, which this check does not send.
- **Conversion stops gaining at 4 connections.** It stays near 28 a second from 4 to 16, because the pool has two LibreOffice workers. More connections only wait longer: the median goes from 131 ms at 4 to 604 ms at 16. One more run with `RUSTICDOC_WORKERS=4` gave 16, 49, 48 and 50 a second at 1, 4, 8 and 16 connections, and a peak of 313 MiB.
- **Worker restarts are part of the cost.** A worker restarts after 10 jobs, so 600 conversions meant 60 restarts. They show in the tail: at one connection the median is 50 ms and the 95th percentile 155 ms. Two more runs with `RUSTICDOC_LIBREOFFICE_MAX_JOBS=1000`, so no restarts, gave 38 to 46 a second at 4 and 16 connections with the same two workers. At one connection they gave 13, 18 and 20, so the gain there is not clear.
- **`convert,pdfa` is half the speed of `pdfa`.** It runs LibreOffice twice for each request. `steps=pdfa` exports PDF/A in one pass, at the rate of a plain `convert`.
- **Filling without LibreOffice levels off between 4 and 8 connections**, near 1,600 a second for the text template and 640 for the DOCX. The container had four render workers. This check did not try other values.
- **Memory stayed low.** The peak was 249 MiB, and under 115 MiB for the runs that did not convert. One small document does not test the memory limit.

## What it does not show

This is one Mac, with the service inside Docker Desktop's VM and the client on the same machine, taking CPU from it. The Mac was in everyday use and seven other containers were running in the VM. There is one small document.

Three full runs on the day gave these ranges at 4 to 16 connections: 1,281 to 1,694 a second for text, 514 to 679 for DOCX, 21 to 30 for `convert`, 13 to 15 for `convert,pdfa` and 19 to 29 for `pdfa`. The widest gap on one row was 30%, so a difference of that size between two rows can be noise.

Use the figures to compare one scenario with another, not as a promise of capacity. A larger document will be slower, and a Linux server will differ.

The image was built two days before the run and the source has changed since. This build refused the `date` filter in a text template, so `invoice.txt` prints its dates as they are sent.

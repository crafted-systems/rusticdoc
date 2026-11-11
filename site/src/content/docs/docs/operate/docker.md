---
title: Docker
description: Run one rusticdoc container that other people can use, with credentials, limits, fonts, health checks and upgrades.
sidebar:
  order: 1
---

Before you leave a container running for other people, give it credentials, limits and fonts, and know how to check and upgrade it.

## The command

```bash
docker run -d --name rusticdoc --restart unless-stopped \
  --read-only --cap-drop ALL --security-opt no-new-privileges \
  --memory 6g --pids-limit 512 \
  --tmpfs /tmp:size=2g --tmpfs /home/rusticdoc:uid=1001,gid=1001,size=512m \
  --env-file rusticdoc.env \
  -v "$PWD/fonts:/fonts:ro" \
  -p 127.0.0.1:9027:9027 ghcr.io/crafted-systems/rusticdoc:0.1.3
```

[Install](/docs/start/install/) explains the hardening flags. The others are for:

| Flag | Reason |
|---|---|
| `--memory 6g` | Room for four render workers at their 1 GiB limit, plus the service and LibreOffice. See [sizing](/docs/operate/sizing/) |
| `--pids-limit 512` | Bounds the child processes, which is what LibreOffice and the render workers are |
| `--tmpfs ...size=` | Temporary files live in memory and count against `--memory`, so writing 300 MB to `/tmp` raises the container's memory use by about 300 MiB |
| `--env-file` | Keeps secrets out of your shell history and off the command line |
| `-v .../fonts:/fonts:ro` | Lets you add fonts without a rebuild. See [fonts](/docs/guides/fonts/) |

Do not add `--init`. It puts a process at PID 1 whose environment, including your keys, can be read from inside the container. Without it, reading `/proc/1/environ` as the service's user gives `Permission denied`.

## Set credentials

Start from a `rusticdoc.env` like this:

```bash
RUSTICDOC_API_KEYS=change-me-one,change-me-two
RUSTICDOC_WEBHOOK_SECRET=change-me
```

Without credentials, anyone who can reach the port can use the service, and it warns about that every time it starts. A half-configured setup, like a basic auth user with no password, stops it from starting. [Authentication](/docs/guides/authentication/) covers API keys, basic auth and OpenID Connect.

The service speaks plain HTTP, so put a reverse proxy in front of it for TLS and rate limits, and leave the port on `127.0.0.1` or a private network. [Security](/docs/operate/security/) lists what else you're responsible for.

## Check it

The image has a health check that runs `rusticdoc health` every 30 seconds. From outside the container:

```bash test
curl http://localhost:9027/health
curl http://localhost:9027/ready
```

`/health` tells you the service is responding. `/ready` also waits until the replica has its copy of the stored fonts, if there's a store. Once credentials are configured, a caller without them sees only the status. [Observability](/docs/operate/observability/) covers both routes, along with logs and metrics.

## Compose

`deploy/compose.yaml` runs the same hardened container. From the root of a checkout:

```bash
docker compose -f deploy/compose.yaml up -d --build
```

To run an image you already have, set `RUSTICDOC_IMAGE` and use `--no-build`. These variables shape the container, and the service itself doesn't read them:

| Variable | Default | What it sets |
|---|---|---|
| `RUSTICDOC_IMAGE` | `rusticdoc:local` | The image |
| `RUSTICDOC_PUBLISH_HOST` | `127.0.0.1` | The address to publish on |
| `RUSTICDOC_PORT` | `9027` | The port to publish on |
| `RUSTICDOC_MEM_LIMIT` | `6g` | `mem_limit` |
| `RUSTICDOC_PIDS_LIMIT` | `512` | `pids_limit` |
| `RUSTICDOC_FONTS` | `./fonts` | The fonts directory, relative to `deploy/compose.yaml` |
| `RUSTICDOC_TMP_SIZE` | `2g` | The size of the `/tmp` tmpfs |
| `RUSTICDOC_HOME_SIZE` | `512m` | The size of the home tmpfs |

Compose passes these on to the service, and leaves empty ones unset:

| Variable | Compose default |
|---|---|
| `RUSTICDOC_API_TIMEOUT` | `60s`, where the service's own default is `30s` |
| `RUSTICDOC_WORKERS` | `2` |
| `RUSTICDOC_API_KEYS` | Empty |
| `RUSTICDOC_ADMIN_KEYS` | Empty |
| `RUSTICDOC_WEBHOOK_SECRET` | Empty |
| `RUSTICDOC_LOG_LEVEL` | `info` |
| `RUSTICDOC_RENDER_MEMORY` | `1GB` |
| `RUSTICDOC_RENDER_WORKERS` | `4` |
| `RUSTICDOC_STORE` | Empty |
| `AWS_ENDPOINT`, `AWS_ALLOW_HTTP`, `AWS_REGION`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` | Passed only if set |

To pass any other setting from the [configuration reference](/docs/reference/configuration/), add it to the file.

## Stop and upgrade

On `SIGTERM` the service stops accepting connections and lets the requests in flight finish. `docker stop` waits 10 seconds by default, which can be shorter than a conversion, so give it longer. The Kubernetes manifest allows 90 seconds:

```bash
docker stop -t 90 rusticdoc
```

Nothing the service keeps in the container has to survive it, so to upgrade you stop it, remove it and start the new image with the same command. Without a store there's nothing to migrate, and with one the store stays where it is.

Rebuild or update the image regularly. A scan finds CVEs in Debian's packages, and the ones Debian has fixed reach you when the image is rebuilt. See [security](/docs/operate/security/) for the numbers from the last scan.

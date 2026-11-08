---
title: Install
description: Run rusticdoc from the image, with Compose or from source, and use its CLI.
sidebar:
  order: 2
---

This page shows you how to run the service with the right container flags, with Compose, and from source, and how to run the CLI. It ends with what the machine needs.

## The image

The image will be `ghcr.io/vmosoti/rusticdoc:0.1.0`. It is not published yet, so for now build it from a checkout of the repository:

```bash
docker build -t rusticdoc .
```

The build pins its base images by digest. The result is one Rust binary on Debian 13 with LibreOffice, its Python bridge, qpdf and pdfcpu. It has no Node, Java or Chromium. It runs as user 1001 and listens on port 9027.

## Run it hardened

```bash
docker run -d --name rusticdoc --read-only --cap-drop ALL --security-opt no-new-privileges \
  --tmpfs /tmp --tmpfs /home/rusticdoc:uid=1001,gid=1001 \
  -p 127.0.0.1:9027:9027 ghcr.io/vmosoti/rusticdoc:0.1.0
```

| Flag | Why |
|---|---|
| `--read-only` | A compromised engine cannot change the image's files |
| `--cap-drop ALL` | The service binds an unprivileged port and needs no Linux capabilities |
| `--security-opt no-new-privileges` | Nothing inside can gain privileges through a setuid binary |
| `--tmpfs /tmp` | Scratch space for conversions. With `--read-only` and no `/tmp`, a conversion answers `500` |
| `--tmpfs /home/rusticdoc:uid=1001,gid=1001` | A writable home directory for the engines. A DOCX conversion also worked without it, but Compose and the Kubernetes manifest both provide one, so that is the setup to copy |
| `-p 127.0.0.1:9027:9027` | Publishes the port on this machine only |

Do not add `--init`. The service runs as PID 1 so that it can close its `/proc/1/environ` to the user LibreOffice runs as. With `--init`, PID 1 is `docker-init`, and its environment, which holds your keys, is readable from inside the container.

With no credentials, the service answers anyone and warns at every start. Set `RUSTICDOC_API_KEYS` before you publish the port more widely. [Docker](/docs/operate/docker/) has more, including limits on memory and processes.

## Compose

`deploy/compose.yaml` sets the same flags and adds limits on memory, processes and temporary files. It reads its settings from your environment. From the root of a checkout:

```bash
docker compose -f deploy/compose.yaml up -d --build
```

To use the published image instead of building one:

```bash
RUSTICDOC_IMAGE=ghcr.io/vmosoti/rusticdoc:0.1.0 docker compose -f deploy/compose.yaml up -d --no-build
```

[Docker](/docs/operate/docker/) lists the variables it reads.

## Build from source

You need the Rust toolchain that `rust-toolchain.toml` pins, which is 1.99.1. `rustup` installs it for you.

```bash
cargo build --release -p rusticdoc
./target/release/rusticdoc serve --bind 127.0.0.1:9027
```

The binary looks for its engines at fixed paths, not on your `PATH`. Set `RUSTICDOC_LIBREOFFICE_BIN`, `RUSTICDOC_QPDF_BIN` and `RUSTICDOC_PDFCPU_BIN` if yours are elsewhere. LibreOffice also needs its Python UNO bridge, which is the `python3-uno` package on Debian. The [configuration reference](/docs/reference/configuration/) lists the paths it tries.

`rusticdoc engines` shows what it found. An engine that is missing fails only the steps that need it, with `503`. On a Mac with no engines installed, text templates rendered and a `convert` step answered `503`. The render memory limit relies on Linux and has not been shown to hold on macOS, so do not serve untrusted templates from a Mac.

## The CLI

The service and the CLI are the same binary, so the image has the CLI in it:

```bash
docker run --rm --entrypoint rusticdoc ghcr.io/vmosoti/rusticdoc:0.1.0 engines
```

To work on files in your current directory, mount it. The container's user has to be able to write there, so on Linux pass your own user ID. Group 0 owns the home directory, so keep that group:

```bash
docker run --rm -v "$PWD:/work" -w /work --user "$(id -u):0" \
  --entrypoint rusticdoc ghcr.io/vmosoti/rusticdoc:0.1.0 \
  render invoice.docx --data invoice.json --steps convert -o invoice.pdf
```

From a source build, `cargo build --release -p rusticdoc` gives you the same commands in `target/release/rusticdoc`.

The CLI reads `--data` from a file.

The CLI renders templates in its own process, with no separate worker and no memory limit, because it assumes the template is yours. `serve` isolates them. Run untrusted templates through the service. The [CLI reference](/docs/reference/cli/) lists the commands.

## System requirements

| | What is known |
|---|---|
| Container runtime | Docker. CI builds and smoke-tests the image on `ubuntu-latest`. The examples in these docs ran on an arm64 build |
| Disk | About 300 MB to download, unpacking to about 810 MB, measured on the arm64 build. Docker Desktop keeps both copies and shows about 1.1 GB |
| Memory at rest | 150 to 270 MiB in `docker stats`, measured after a first conversion with two LibreOffice workers. This grows with what you ask of it |
| Memory under load | Compose limits the container to 6 GiB, which leaves room for four render workers at 1 GiB each, the service and two LibreOffice workers. [Sizing](/docs/operate/sizing/) has the arithmetic and a failure seen at less |
| CPU | The Kubernetes manifest requests one CPU per pod. No minimum has been measured |
| Operating system for the service | Linux, for the memory limit on render workers. The image is Linux |
| Rust, from source | 1.99.1, pinned in `rust-toolchain.toml` |

There are no load test figures yet. Size for your own documents and measure them.

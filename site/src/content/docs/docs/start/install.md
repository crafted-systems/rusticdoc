---
title: Install
description: Run rusticdoc from the image, with Compose or from source, and use its CLI.
sidebar:
  order: 2
---

You can run rusticdoc from the published image, with Compose, or from a source build. The same binary is also the CLI.

## The image

The image is `ghcr.io/crafted-systems/rusticdoc:0.1.1`, built for amd64 and arm64. `latest` points at the newest release, but name a version anywhere you need to know what's running. You can also build it from a checkout of the repository:

```bash
docker build -t rusticdoc .
```

The build pins its base images by digest. What you get is one Rust binary on Debian 13 with LibreOffice, its Python bridge, qpdf and pdfcpu. There's no Node, Java or Chromium in it. It runs as user 1001 and listens on port 9027.

## Run it hardened

```bash
docker run -d --name rusticdoc \
  --read-only --cap-drop ALL --security-opt no-new-privileges \
  --tmpfs /tmp --tmpfs /home/rusticdoc:uid=1001,gid=1001 \
  -p 127.0.0.1:9027:9027 ghcr.io/crafted-systems/rusticdoc:0.1.1
```

| Flag | Why |
|---|---|
| `--read-only` | A compromised engine can't change the image's files |
| `--cap-drop ALL` | The service binds an unprivileged port, so it needs no Linux capabilities |
| `--security-opt no-new-privileges` | Nothing inside can gain privileges through a setuid binary |
| `--tmpfs /tmp` | Scratch space for conversions. With `--read-only` and no `/tmp`, a conversion returns `500` |
| `--tmpfs /home/rusticdoc:uid=1001,gid=1001` | A writable home directory for the engines. A DOCX conversion works without one, but Compose and the Kubernetes manifest both provide it, so that's the setup to copy |
| `-p 127.0.0.1:9027:9027` | Publishes the port on this machine only |

Don't add `--init`. The service runs as PID 1 so that it can close its `/proc/1/environ` to the user LibreOffice runs as. With `--init`, PID 1 is `docker-init` instead, and its environment, which holds your keys, can be read from inside the container.

With no credentials set, the service accepts requests from anyone and logs a warning every time it starts. Set `RUSTICDOC_API_KEYS` before you publish the port more widely. [Docker](/docs/operate/docker/) goes further, including limits on memory and processes.

## Compose

`deploy/compose.yaml` sets the same flags and adds limits on memory, processes and temporary files. It reads its settings from your environment. From the root of a checkout:

```bash
docker compose -f deploy/compose.yaml up -d --build
```

To use the published image instead of building one:

```bash
RUSTICDOC_IMAGE=ghcr.io/crafted-systems/rusticdoc:0.1.1 \
  docker compose -f deploy/compose.yaml up -d --no-build
```

[Docker](/docs/operate/docker/) lists the variables it reads.

## Build from source

You need the Rust toolchain pinned in `rust-toolchain.toml`, which is 1.99.1. `rustup` installs it for you.

```bash
cargo build --release -p rusticdoc
./target/release/rusticdoc serve --bind 127.0.0.1:9027
```

The binary looks for its engines at fixed paths, not on your `PATH`. If yours are somewhere else, set `RUSTICDOC_LIBREOFFICE_BIN`, `RUSTICDOC_QPDF_BIN` and `RUSTICDOC_PDFCPU_BIN`. LibreOffice also needs its Python UNO bridge, which is the `python3-uno` package on Debian. The [configuration reference](/docs/reference/configuration/) lists the paths it tries.

`rusticdoc engines` shows what it found. A missing engine only fails the steps that need it, with `503`. On a Mac with no engines installed, for example, text templates render and a `convert` step returns `503`. The memory limit on rendering relies on Linux and isn't known to hold on macOS, so don't serve untrusted templates from a Mac.

## The CLI

### Download it

Every [GitHub release](https://github.com/crafted-systems/rusticdoc/releases) has the CLI for Linux and macOS attached, along with a `SHA256SUMS` file:

| System | Archive |
|---|---|
| Linux, x86-64 | `rusticdoc-0.1.1-x86_64-unknown-linux-gnu.tar.gz` |
| Linux, ARM64 | `rusticdoc-0.1.1-aarch64-unknown-linux-gnu.tar.gz` |
| macOS, Apple silicon | `rusticdoc-0.1.1-aarch64-apple-darwin.tar.gz` |
| macOS, Intel | `rusticdoc-0.1.1-x86_64-apple-darwin.tar.gz` |

These commands fetch the Apple silicon build, check it and put it on your `PATH`:

```bash
curl -fsSLO https://github.com/crafted-systems/rusticdoc/releases/download/v0.1.1/rusticdoc-0.1.1-aarch64-apple-darwin.tar.gz
curl -fsSLO https://github.com/crafted-systems/rusticdoc/releases/download/v0.1.1/SHA256SUMS
shasum -a 256 --check --ignore-missing SHA256SUMS
tar -xzf rusticdoc-0.1.1-aarch64-apple-darwin.tar.gz
sudo mv rusticdoc-0.1.1-aarch64-apple-darwin/rusticdoc /usr/local/bin/
rusticdoc --version
```

On Linux, `sha256sum --check --ignore-missing SHA256SUMS` does the check.

- macOS quarantines a downloaded binary, and the builds aren't notarised. If it refuses to open, run `xattr -d com.apple.quarantine /usr/local/bin/rusticdoc`.
- The Linux builds need glibc 2.35 or newer, which Ubuntu 22.04 and Debian 12 have. They also need fontconfig so that HTML layout can find fonts.
- There's no Windows build, because the CLI's process handling is Unix-only. Run the Linux build under WSL, or use the image as shown below.

By itself, the downloaded CLI can render, validate and run the PDF operations that its built-in engine covers. A `convert` step needs LibreOffice installed, and some PDF operations need qpdf or pdfcpu. [Build from source](#build-from-source) explains how the binary finds them, and `rusticdoc engines` shows what it found.

### From the image

The service and the CLI are the same binary, so the image already has the CLI in it:

```bash
docker run --rm --entrypoint rusticdoc ghcr.io/crafted-systems/rusticdoc:0.1.1 engines
```

To work on files in your current directory, mount it. The container's user has to be able to write there, so on Linux pass your own user ID. Keep group 0, because that group owns the home directory:

```bash
docker run --rm -v "$PWD:/work" -w /work --user "$(id -u):0" \
  --entrypoint rusticdoc ghcr.io/crafted-systems/rusticdoc:0.1.1 \
  render invoice.docx --data invoice.json --steps convert -o invoice.pdf
```

From a source build, `cargo build --release -p rusticdoc` gives you the same commands in `target/release/rusticdoc`.

The CLI reads `--data` from a file.

It renders templates in its own process, with no separate worker and no memory limit, because it assumes the template is yours. `serve` isolates them, so run untrusted templates through the service. The [CLI reference](/docs/reference/cli/) lists the commands.

## System requirements

| | Notes |
|---|---|
| Container runtime | Docker. CI builds and smoke-tests the image on `ubuntu-latest`, and the examples in these docs ran on an arm64 build |
| Disk | About 300 MB to download and about 810 MB unpacked, measured on the arm64 build. Docker Desktop keeps both copies, so it shows about 1.1 GB |
| Memory at rest | 150 to 270 MiB in `docker stats`, measured after a first conversion with two LibreOffice workers. It grows with the work you give it |
| Memory under load | Compose limits the container to 6 GiB. That leaves room for four render workers at 1 GiB each, the service and two LibreOffice workers. [Sizing](/docs/operate/sizing/) has the arithmetic, and a failure seen with less |
| CPU | The Kubernetes manifest requests one CPU per pod |
| Operating system for the service | Linux, because the memory limit on render workers needs it. The image is Linux |
| Rust, from source | 1.99.1, pinned in `rust-toolchain.toml` |

[Sizing](/docs/operate/sizing/#measured-under-load) has load test figures for one small document on one machine, so still measure with your own documents before you size.

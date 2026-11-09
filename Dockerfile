# syntax=docker/dockerfile:1
#
# Builds the rusticdoc service image: one Rust binary on Debian 13 with
# LibreOffice (from trixie-backports) and its Python UNO bridge, qpdf,
# pdfcpu, and poppler's pdftoppm for template previews (libpoppler itself
# comes with LibreOffice's PDF import). No Node, Java or Chromium. Build
# from the project root:
#   docker build -t rusticdoc .

# Base images are pinned by digest; dependabot's docker ecosystem keeps them fresh.

# ----------------------------------------------
# pdfcpu, a static Go binary, verified through the Go checksum database.
# ----------------------------------------------
FROM golang:1-trixie@sha256:af1bfc2614ebb1dbd31af514b78740fd678850e76498762b425ecee4c5c6ea80 AS pdfcpu
ARG PDFCPU_VERSION=v0.16.4
RUN CGO_ENABLED=0 GOBIN=/out go install -ldflags "-s -w -X 'main.version=$PDFCPU_VERSION' -X 'github.com/pdfcpu/pdfcpu/pkg/pdfcpu/model.VersionStr=$PDFCPU_VERSION'" \
    github.com/pdfcpu/pdfcpu/cmd/pdfcpu@$PDFCPU_VERSION

# ----------------------------------------------
# Build
# ----------------------------------------------
FROM rust:1.99.1-slim-trixie@sha256:7eab7ce19273ce785e0ca7578bdf7bc1fcbae621775a8197379a04c9f6e03cd5 AS build
WORKDIR /src
# Servo's style engine, under Fulgur, generates code with Python at build time.
RUN apt-get update -qq \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends python3 \
    && rm -rf /var/lib/apt/lists/*
# Dependencies first, so source edits do not refetch them.
COPY Cargo.toml Cargo.lock ./
COPY crates/template/Cargo.toml crates/template/Cargo.toml
COPY crates/core/Cargo.toml crates/core/Cargo.toml
COPY crates/server/Cargo.toml crates/server/Cargo.toml
COPY crates/cli/Cargo.toml crates/cli/Cargo.toml
RUN mkdir -p crates/template/src crates/core/src crates/server/src crates/cli/src \
    && echo 'pub fn placeholder() {}' > crates/template/src/lib.rs \
    && echo 'pub fn placeholder() {}' > crates/core/src/lib.rs \
    && echo 'pub fn placeholder() {}' > crates/server/src/lib.rs \
    && echo 'fn main() {}' > crates/cli/src/main.rs \
    && cargo build --release --locked -p rusticdoc \
    && rm -rf crates/*/src
COPY crates ./crates
# The missing-font report reads the image's font aliases at compile time.
COPY deploy/fonts.conf deploy/fonts.conf
# Touch so cargo sees the real sources as newer than the placeholders.
RUN find crates -name '*.rs' -exec touch {} + \
    && cargo build --release --locked -p rusticdoc \
    && cp target/release/rusticdoc /rusticdoc

# ----------------------------------------------
# Runtime
# ----------------------------------------------
FROM debian:13-slim@sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2 AS runtime

ENV TZ=UTC LANG=C.UTF-8 LC_ALL=C.UTF-8

LABEL org.opencontainers.image.title="rusticdoc" \
      org.opencontainers.image.description="Document engine: Office and PDF templates with {{ }} tags, LibreOffice conversion, PDF operations." \
      org.opencontainers.image.source="https://github.com/vmosoti/rusticdoc" \
      org.opencontainers.image.base.name="debian:13-slim"

COPY deploy/dpkg-excludes /etc/dpkg/dpkg.cfg.d/rusticdoc
RUN groupadd --gid 1001 rusticdoc \
    && useradd --uid 1001 --gid rusticdoc --shell /usr/sbin/nologin --home /home/rusticdoc --no-create-home rusticdoc \
    && mkdir -p /home/rusticdoc /fonts \
    && echo "deb http://deb.debian.org/debian trixie-backports main" >> /etc/apt/sources.list \
    && apt-get update -qq \
    && apt-get upgrade -yqq \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends \
        ca-certificates qpdf poppler-utils fontconfig python3 \
        fonts-crosextra-carlito fonts-crosextra-caladea fonts-liberation fonts-liberation2 \
        fonts-dejavu fonts-noto-cjk fonts-noto-color-emoji fonts-noto-core \
        hyphen-en-us hyphen-en-gb hyphen-de hyphen-fr hyphen-es hyphen-it hyphen-nl hyphen-pt-pt hyphen-pt-br \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends -t trixie-backports \
        libreoffice-writer libreoffice-calc libreoffice-impress libreoffice-draw libreoffice-math python3-uno \
    && rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/* \
    # Arbitrary user IDs, as on OpenShift: group 0 owns the home directory.
    && usermod -aG root rusticdoc \
    && chgrp -R 0 /home/rusticdoc /fonts \
    && chmod -R g=u /home/rusticdoc /fonts

# LibreOffice has no setting that stops its HTML loader fetching stylesheets, so
# beside the dead proxy in its profile, its HTTP, WebDAV and CMIS providers go.
RUN rm -f /usr/lib/libreoffice/program/libucpdav1.so /usr/lib/libreoffice/program/libucpcmis1lo.so

COPY deploy/fonts.conf /etc/fonts/conf.d/100-rusticdoc.conf
COPY --from=pdfcpu /out/pdfcpu /usr/bin/pdfcpu
COPY --from=build /rusticdoc /usr/bin/rusticdoc
COPY LICENSE /usr/share/doc/rusticdoc/LICENSE

ENV RUSTICDOC_LIBREOFFICE_BIN=/usr/lib/libreoffice/program/soffice.bin \
    RUSTICDOC_QPDF_BIN=/usr/bin/qpdf \
    RUSTICDOC_PDFCPU_BIN=/usr/bin/pdfcpu \
    RUSTICDOC_FONTS_DIR=/fonts \
    RUSTICDOC_LOG_FORMAT=json

USER rusticdoc
WORKDIR /home/rusticdoc
VOLUME ["/fonts"]
EXPOSE 9027
HEALTHCHECK --interval=30s --timeout=6s --start-period=20s --retries=3 CMD ["/usr/bin/rusticdoc", "health"]
# As PID 1 it reaps and forwards signals itself, so that /proc/1/environ, which holds
# the service's secrets, can be closed to the user LibreOffice runs as.
ENTRYPOINT ["/usr/bin/rusticdoc"]
CMD ["serve"]

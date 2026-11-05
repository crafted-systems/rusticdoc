# syntax=docker/dockerfile:1
#
# Builds the rusticdoc service image: one Rust binary on Debian 13 with
# LibreOffice (from trixie-backports) and its Python UNO bridge, qpdf and
# pdfcpu. No Node, Java or Chromium. Build from the project root:
#   docker build -t rusticdoc .

ARG RUST_IMAGE=rust:1-slim-trixie
ARG DEBIAN_IMAGE=debian:13-slim
ARG GOLANG_IMAGE=golang:1-trixie

# ----------------------------------------------
# pdfcpu, a static Go binary, verified through the Go checksum database.
# ----------------------------------------------
FROM $GOLANG_IMAGE AS pdfcpu
ARG PDFCPU_VERSION=v0.16.4
RUN CGO_ENABLED=0 GOBIN=/out go install -ldflags "-s -w -X 'main.version=$PDFCPU_VERSION' -X 'github.com/pdfcpu/pdfcpu/pkg/pdfcpu/model.VersionStr=$PDFCPU_VERSION'" \
    github.com/pdfcpu/pdfcpu/cmd/pdfcpu@$PDFCPU_VERSION

# ----------------------------------------------
# Build
# ----------------------------------------------
FROM $RUST_IMAGE AS build
WORKDIR /src
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
# Touch so cargo sees the real sources as newer than the placeholders.
RUN find crates -name '*.rs' -exec touch {} + \
    && cargo build --release --locked -p rusticdoc \
    && cp target/release/rusticdoc /rusticdoc

# ----------------------------------------------
# Runtime
# ----------------------------------------------
FROM $DEBIAN_IMAGE AS runtime

ARG APT_SNAPSHOT=""
ENV TZ=UTC LANG=C.UTF-8 LC_ALL=C.UTF-8

LABEL org.opencontainers.image.title="rusticdoc" \
      org.opencontainers.image.description="Document engine: Office and PDF templates with {{ }} tags, LibreOffice conversion, PDF operations." \
      org.opencontainers.image.source="https://github.com/vmosoti/rusticdoc" \
      org.opencontainers.image.base.name="debian:13-slim"

RUN echo "apt snapshot: $APT_SNAPSHOT" \
    && groupadd --gid 1001 rusticdoc \
    && useradd --uid 1001 --gid rusticdoc --shell /usr/sbin/nologin --home /home/rusticdoc --no-create-home rusticdoc \
    && mkdir -p /home/rusticdoc /fonts \
    && echo "deb http://deb.debian.org/debian trixie-backports main" >> /etc/apt/sources.list \
    && apt-get update -qq \
    && apt-get upgrade -yqq \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends \
        ca-certificates tini qpdf fontconfig python3 \
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

COPY deploy/fonts.conf /etc/fonts/conf.d/100-rusticdoc.conf
COPY --from=pdfcpu /out/pdfcpu /usr/bin/pdfcpu
COPY --from=build /rusticdoc /usr/bin/rusticdoc

ENV RUSTICDOC_BIND=0.0.0.0:3000 \
    RUSTICDOC_LIBREOFFICE_BIN=/usr/lib/libreoffice/program/soffice.bin \
    RUSTICDOC_QPDF_BIN=/usr/bin/qpdf \
    RUSTICDOC_PDFCPU_BIN=/usr/bin/pdfcpu \
    RUSTICDOC_FONTS_DIR=/fonts \
    RUSTICDOC_LOG_FORMAT=json

USER rusticdoc
WORKDIR /home/rusticdoc
VOLUME ["/fonts"]
EXPOSE 3000
HEALTHCHECK --interval=30s --timeout=6s --start-period=20s --retries=3 CMD ["/usr/bin/rusticdoc", "health"]
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/bin/rusticdoc"]
CMD ["serve"]

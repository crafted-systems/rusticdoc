IMAGE ?= rusticdoc:local
PORT ?= 9027
GENERATOR ?= openapitools/openapi-generator-cli:v7.12.0

.PHONY: build test lint fmt run openapi docker up down parity outbound isolation disk replicas sdk sdk-typescript sdk-python sdk-go clean

build:
	cargo build --release -p rusticdoc

test:
	cargo test --workspace

lint:
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings

fmt:
	cargo fmt --all

# The service with pretty logs, from your shell. Engines are found on PATH or
# via RUSTICDOC_LIBREOFFICE_BIN and RUSTICDOC_QPDF_BIN.
run:
	RUSTICDOC_LOG_FORMAT=pretty cargo run -p rusticdoc -- serve --bind 127.0.0.1:$(PORT)

# The API description the SDKs are generated from. CI checks it is current.
openapi:
	cargo run -q -p rusticdoc -- openapi > openapi.json

docker:
	docker build -t $(IMAGE) .

up:
	RUSTICDOC_IMAGE=$(IMAGE) RUSTICDOC_PORT=$(PORT) docker compose -f deploy/compose.yaml up -d --build

down:
	docker compose -f deploy/compose.yaml down

# Gotenberg's own scenarios, replayed against this image and the official one.
parity:
	test/parity.sh $(IMAGE) parity-results

# Crafted documents must reach no network and read no local file.
outbound:
	test/security/outbound.sh $(IMAGE)

isolation:
	test/security/isolation.sh $(IMAGE)

disk:
	test/security/disk.sh $(IMAGE)

# Several containers on one store: a bucket, then a shared volume.
replicas:
	test/store/replicas.sh $(IMAGE)

# SDKs: generated into sdk/<language> from openapi.json. Each is a starting
# point to publish from its own package manager.
sdk: sdk-typescript sdk-python sdk-go

sdk-typescript: openapi
	docker run --rm -v "$(PWD):/local" $(GENERATOR) generate -i /local/openapi.json -g typescript-fetch \
		-o /local/sdk/typescript --additional-properties=npmName=@rusticdoc/client,supportsES6=true,typescriptThreePlus=true

sdk-python: openapi
	docker run --rm -v "$(PWD):/local" $(GENERATOR) generate -i /local/openapi.json -g python \
		-o /local/sdk/python --additional-properties=packageName=rusticdoc,projectName=rusticdoc

sdk-go: openapi
	docker run --rm -v "$(PWD):/local" $(GENERATOR) generate -i /local/openapi.json -g go \
		-o /local/sdk/go --additional-properties=packageName=rusticdoc,moduleName=github.com/vmosoti/rusticdoc/sdk/go

clean:
	cargo clean
	rm -rf sdk/typescript sdk/python sdk/go

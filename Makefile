IMAGE ?= rusticdoc:local
PORT ?= 9027
PYTHON ?= python3
GENERATOR ?= openapitools/openapi-generator-cli:v7.12.0

.PHONY: build test lint fmt run openapi docker up down outbound isolation disk replicas sign-check golden barcodes office sdk sdk-typescript sdk-python sdk-go clean

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

# Signs PDFs with the CLI and checks them with pdfsig, and with pyHanko when it
# is installed or named by PYHANKO: B-B, then B-T, B-LT, B-LTA and certification
# against a local timestamp authority, OCSP responder and CRL. Needs openssl
# (3+ for the levels), python3, curl and poppler-utils.
sign-check:
	test/sign/verify.sh

# The golden corpus rendered on two architectures and compared. Needs both
# images, such as rusticdoc:arm64 and rusticdoc:amd64, plus PyMuPDF and Pillow.
GOLDEN_A ?= rusticdoc:arm64
GOLDEN_B ?= rusticdoc:amd64
golden:
	test/golden/run.sh $(GOLDEN_A)
	test/golden/run.sh $(GOLDEN_B)
	$(PYTHON) test/golden/compare.py test/golden/out/arm64 test/golden/out/amd64

# Every barcode symbology, decoded with ZXing and ZBar.
barcodes:
	PYTHON=$(PYTHON) test/barcodes/check.sh $(IMAGE)

# The Office-made templates, filled for opening in Word, Excel and PowerPoint.
office:
	test/office/render.sh $(IMAGE)

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

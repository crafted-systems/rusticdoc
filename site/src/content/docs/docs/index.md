---
title: Documentation
description: Where to find how to install, use and run rusticdoc.
template: doc
---

rusticdoc fills Office and PDF templates from JSON, converts them with LibreOffice and runs PDF operations on the result. It's written in Rust and ships as one hardened container image. You can call it over HTTP or run it as a command-line tool.

The current release is 0.1.4. Read the [known limits](/docs/project/known-limits/) before you depend on it.

## Start here

- [Quickstart](/docs/start/quickstart/) takes you from nothing to a filled invoice PDF.
- [Install](/docs/start/install/) covers the image, Compose, building from source, the CLI and system requirements.
- [Concepts](/docs/start/concepts/) explains how a request moves through templates, steps and engines.

## Write templates

- [Syntax](/docs/templates/syntax/)
- [Filters](/docs/templates/filters/)
- [Office templates](/docs/templates/office/)

## Use the API

- [The pipeline](/docs/guides/pipeline/)
- [Convert documents](/docs/guides/convert/)
- [PDF operations](/docs/guides/pdf-operations/)
- [Stored templates](/docs/guides/stored-templates/)
- [Fonts](/docs/guides/fonts/)
- [Webhooks](/docs/guides/webhooks/)
- [Authentication](/docs/guides/authentication/)

## Run it

- [Docker](/docs/operate/docker/)
- [Many containers](/docs/operate/many-containers/)
- [Kubernetes](/docs/operate/kubernetes/)
- [Sizing](/docs/operate/sizing/)
- [Security](/docs/operate/security/)
- [Observability](/docs/operate/observability/)

## Use with AI

- [Agent skills](/docs/ai/skills/)

## Reference

- [HTTP API](/docs/reference/api/)
- [Configuration](/docs/reference/configuration/)
- [CLI](/docs/reference/cli/)
- [Errors](/docs/reference/errors/)

## Project

- [Known limits](/docs/project/known-limits/)
- [Changelog](/docs/project/changelog/)
- [SDKs](/docs/project/sdks/)
- [Licence](/docs/project/license/)

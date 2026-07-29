---
status: Accepted
date: 2026-07-29
---
# Decision 04 — Releases are drafts until the installed binary passes the full e2e suite

## Context

The owner required the pipeline to verify the release artifact itself: install via the shipped install scripts from the draft, run the whole executable spec against the installed binary, on every supported OS - only then publish. GitHub issue #40, sprint wave 5.

## Decision

release.yml creates a true draft; a 3-OS verify matrix installs from it (explicit LUABOX_DRAFT_INSTALL=1 opt-in path in scripts/install.sh and install.ps1) and runs acceptance + lsp_acceptance via LUABOX_E2E_BIN; publish-release performs the single draft-to-latest transition; post-publish smoke covers what only works on a live release.

## Consequences

A release that fails verification never existed for users. Cost: the draft-API install path's first true end-to-end test is the first real tag push (mock-tested until then - docs/02-guides/01-releasing.md).

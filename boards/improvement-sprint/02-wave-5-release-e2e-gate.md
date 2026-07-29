---
column: done
labels: [wave, release]
priority: high
updatedAt: 2026-07-29T05:20:00.000Z
---
# Wave 5 — draft-gated release pipeline (#40)

release.yml now creates a true draft, installs it via the shipped install scripts on 3 OSes (explicit `LUABOX_DRAFT_INSTALL=1` opt-in), runs the full e2e suites against the installed binary via `LUABOX_E2E_BIN`, and only then flips draft to latest (.github/workflows/release.yml). Merged in b21a993. Residual risk: the draft-API path is mock-tested only until the first real tag push — see docs/02-guides/01-releasing.md. GitHub issue #40.

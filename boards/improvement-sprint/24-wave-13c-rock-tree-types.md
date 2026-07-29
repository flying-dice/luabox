---
column: doing
labels: [wave, docs]
priority: high
agent: opus-13c
live: true
status: launching
updatedAt: 2026-07-29T22:24:00.000Z
---
# Wave 13c — cross-package types from a bare luarocks tree (#30)

Owner pre-release burn-down; queued behind 13b (shared crates). Design decision + implementation: harvest LuaCATS annotations from installed rock sources so a bare `luarocks install --tree lua_modules` yields visible, checked signatures without a defs package. The documented sharp edge goes away.

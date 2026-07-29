---
column: doing
labels: [wave, review-round]
priority: high
agent: opus-w11
live: true
status: agent launching
updatedAt: 2026-07-29T14:02:00.000Z
---
# Wave 11 — round-5 findings (watch drain swallow, EPIPE abort)

F1: stop the post-rerun drain from discarding real edits (crates/luabox-cli/src/watch.rs:160,:243) - the triggers_rerun kind filter already prevents the loop; drained events that pass the filter must be acted on, not thrown away. Regression test: an edit inside the drain window must be reported. F2: BrokenPipe on stdout must exit quietly instead of SIGABRT/134 (crates/luabox-cli/src/project.rs:39 and every report print path).

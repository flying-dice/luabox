# LB0510 shape matrix

One Lua program per shape of `metatable-without-index` (LB0510), with the
verdict each is held to on **two independent axes**:

| column     | authority                                                                       |
| ---------- | ------------------------------------------------------------------------------- |
| `findings` | `luabox lint` — how many LB0510 diagnostics the program produces (0 = silent)    |
| `runtime`  | `lua5.4` — `crash` if executing the program exits non-zero, `ok` if it exits 0   |

`expected.tsv` records both. `../lb0510-matrix.sh` re-derives both and fails
on any disagreement, so the false-positive and false-negative claims in
`docs/03-reference/02-limitations.md` are falsifiable by anyone with the
release binary and an interpreter — they are not prose about a measurement
somebody once took. The `findings` column is a count rather than a
warn/silent flag so that one-finding-per-`setmetatable`-site is pinned too.

The two columns are deliberately **not** asserted equal. Where they differ,
the `note` column says so and the rule's documented bound is what explains it:

- a non-zero `findings` against a `runtime` of `ok` is an over-approximation
  the no-metafield arm makes on purpose — the construction cannot serve the
  lookup the `---@class` annotation promises even if this file never performs
  one. `plain_carrier_uninvoked` is the deliberate instance;
- a `findings` of 0 against a `runtime` of `crash` would be a false negative.
  There is currently none in the matrix.

Every program is a self-contained chunk: no `require`, no arguments, and its
output does not matter — only its exit status does. Add a shape by dropping in
a `.lua` file and adding its row; the runner fails on a program with no row and
on a row with no program.

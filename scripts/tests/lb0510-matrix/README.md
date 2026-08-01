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
  one. `plain_carrier_uninvoked` is the deliberate instance, and the *only*
  one: every such row in the metafield arm has been a bug;
- a `findings` of 0 against a `runtime` of `crash` is a **disclosed false
  negative**. The `metafield_fn_*` rows are exactly those, one per bound named
  in `docs/03-reference/02-limitations.md`. They are pinned so that closing one
  is a deliberate act rather than a surprise.

## Twins

Every shape that can be written both invoked and uninvoked is committed **both
ways**. This is not padding. Round 7 found the matrix asserting
`metafield_call_factory_reaches_self` correctly while the identical file ending
`print(type(f))` — silent at runtime, byte-identical in lint output — had never
been written down, so the harness could not see the false positive:

> The matrix would have caught it if a fixture had been written for the
> uninvoked case; it has one for every invoked case.

A twin pair whose `findings` agree while their `runtime` disagrees is therefore
a *claim*, not an accident: either the lint column is wrong on one of them, or
the difference is a documented bound and the `note` says which one. When you
add or change a shape, add its twin.

Every program is a self-contained chunk: no `require`, no arguments, and its
output does not matter — only its exit status does. Add a shape by dropping in
a `.lua` file and adding its row; the runner fails on a program with no row and
on a row with no program.

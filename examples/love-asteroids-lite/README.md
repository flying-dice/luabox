# love-asteroids-lite

A minimal [LÖVE](https://love2d.org) game skeleton: a rectangle you move with
the arrow keys. It shows how luabox types a framework via a definition package
and packages a runnable `.love` archive.

```
love-asteroids-lite/
├── luabox.toml            # [build] mode = "love", [types] defs = ["love2d"]
├── defs/love2d.d.lua      # a MINIMAL LÖVE type-defs starter subset (~45 lines)
├── src/main.lua           # love.load / love.update / love.draw
├── src/conf.lua           # love.conf — window config
└── assets/README.txt      # placeholder; copied into the .love verbatim
```

## Typing a framework with a defs package

LÖVE's API lives in the ambient `love` global. `defs/love2d.d.lua` is a
`---@meta` definition file describing just the slice this example uses —
`love.graphics.rectangle`, `love.keyboard.isDown`, and the `load`/`update`/
`draw`/`conf` callbacks, with real signatures. It is wired in with:

```toml
[types]
defs = ["love2d"]   # resolves defs/love2d.d.lua by its file stem
```

With that, `luabox check` type-checks your calls into LÖVE — a wrong-arity
`love.graphics.rectangle(...)` is caught before you ever launch the game.

> The defs file is a **starter subset**, not the full LÖVE API. For a real
> project, replace it with the community's complete LÖVE definitions.

```sh
luabox check        # 0 errors — LÖVE calls typed against the defs
luabox fmt --check
luabox lint         # clean — defs/love2d.d.lua is a `---@meta` defs module,
                     # so declaring the `love` global there needs no [lint] entry
```

## Packaging a `.love`

```sh
luabox bundle --mode love
```

This produces `dist/asteroids-lite.love` — a zip archive with `main.lua` and
`conf.lua` at its root and `assets/` copied in verbatim. Verify the contents:

```sh
unzip -l dist/asteroids-lite.love
#   main.lua
#   conf.lua
#   assets/README.txt
```

## Running it

luabox is not a runtime — it builds *for* LÖVE, it doesn't embed it. Install
LÖVE, then:

```sh
love dist/asteroids-lite.love
```

Arrow keys move the square. From here, add asteroids.

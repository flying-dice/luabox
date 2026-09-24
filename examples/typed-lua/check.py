#!/usr/bin/env python3
"""Check the Typed Lua examples' hand-written compiler output.

Each example keeps its source in src/ and the output the compiler must
produce in dist/. Until the compiler exists, dist/ is written by hand; this
script holds it to the spec's erasure rule and to Lua itself:

1. Every src/**/*.luac has a dist/**/*.lua with the same number of lines.
2. Each output line is its source line with characters replaced by spaces
   (trailing spaces dropped), so every remaining token keeps its line and
   column.
3. Every dist/**/*.lua loads in a stock Lua interpreter.
4. Where an example has expected-output.txt, `lua main.lua` run in dist/
   prints exactly that.

Usage: python3 check.py [--lua PATH]   (default: $LUA, else `lua` on PATH)
"""

import argparse
import os
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent


def lines(path):
    return path.read_text(encoding="utf-8").split("\n")


def check_erasure(src, out):
    errors = []
    s_lines, o_lines = lines(src), lines(out)
    if len(s_lines) != len(o_lines):
        return [f"{out}: {len(o_lines)} lines, source has {len(s_lines)}"]
    for n, (s, o) in enumerate(zip(s_lines, o_lines), 1):
        if o != o.rstrip():
            errors.append(f"{out}:{n}: trailing whitespace")
        if len(o) > len(s):
            errors.append(f"{out}:{n}: longer than the source line")
            continue
        for col, (sc, oc) in enumerate(zip(s, o), 1):
            if oc != sc and oc != " ":
                errors.append(
                    f"{out}:{n}:{col}: {oc!r} where the source has {sc!r}"
                )
                break
    return errors


def check_example(example, lua):
    errors = []
    src, dist = example / "src", example / "dist"
    for luac in sorted(src.rglob("*.luac")):
        out = dist / luac.relative_to(src).with_suffix(".lua")
        if not out.exists():
            errors.append(f"{luac}: no output at {out}")
            continue
        errors += check_erasure(luac, out)
    for out in sorted(dist.rglob("*.lua")):
        rel = out.relative_to(dist)
        plain = src / rel
        if not (src / rel.with_suffix(".luac")).exists() and not plain.exists():
            errors.append(f"{out}: no source for this output")
        if plain.exists() and plain.read_bytes() != out.read_bytes():
            errors.append(f"{out}: plain Lua must be copied unchanged")
        result = subprocess.run(
            [lua, "-e", f"assert(loadfile([[{out}]]))"],
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            errors.append(f"{out}: does not load: {result.stderr.strip()}")
    expected = example / "expected-output.txt"
    if expected.exists():
        result = subprocess.run(
            [lua, "main.lua"], cwd=dist, capture_output=True, text=True
        )
        want = expected.read_text(encoding="utf-8")
        if result.returncode != 0 or result.stdout != want:
            errors.append(
                f"{example.name}: `lua main.lua` printed:\n{result.stdout}"
                f"{result.stderr}\nexpected:\n{want}"
            )
    return errors


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--lua", default=os.environ.get("LUA", "lua"))
    args = parser.parse_args()
    examples = sorted(p for p in ROOT.iterdir() if (p / "src").is_dir())
    failures = 0
    for example in examples:
        errors = check_example(example, args.lua)
        failures += len(errors)
        print(f"{'FAIL' if errors else 'ok  '} {example.name}")
        for error in errors:
            print(f"     {error}")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()

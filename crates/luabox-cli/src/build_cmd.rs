//! `luabox build [--target <t>] [--out dir]` — under construction (#22).

use std::path::Path;

use anyhow::bail;

pub fn run(_cwd: &Path, _target: Option<&str>, _out: Option<&Path>) -> anyhow::Result<()> {
    bail!("`luabox build` is not implemented yet — planned for P3 (see SPEC.md §18)")
}

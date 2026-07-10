//! `luabox lint [--fix]` — under construction (ticket #15).

use std::path::Path;

use anyhow::bail;

pub fn run(_cwd: &Path, _fix: bool) -> anyhow::Result<()> {
    bail!("`luabox lint` is not implemented yet — planned for P1 (see SPEC.md §18)")
}

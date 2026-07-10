//! `luabox bundle` — under construction (#24).

use std::path::Path;

use anyhow::bail;

pub fn run(_cwd: &Path, _minify: bool, _sourcemap: bool) -> anyhow::Result<()> {
    bail!("`luabox bundle` is not implemented yet — planned for P3 (see SPEC.md §18)")
}

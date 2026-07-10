//! `luabox test [pattern]` — under construction (#25).

use std::path::Path;

use anyhow::bail;

pub fn run(
    _cwd: &Path,
    _pattern: Option<&str>,
    _watch: bool,
    _coverage: bool,
) -> anyhow::Result<()> {
    bail!("`luabox test` is not implemented yet — planned for P4 (see SPEC.md §18)")
}

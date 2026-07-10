//! `luabox run <script|task>` — under construction (#28).

use std::path::Path;

use anyhow::bail;

pub fn run(_cwd: &Path, _script: &str, _args: &[String]) -> anyhow::Result<()> {
    bail!("`luabox run` is not implemented yet — planned for P4 (see SPEC.md §18)")
}

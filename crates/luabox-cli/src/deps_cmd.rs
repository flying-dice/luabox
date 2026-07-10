//! `luabox add/remove/install/update/vendor` — under construction (#21).

use std::path::Path;

use anyhow::bail;

pub fn add(_cwd: &Path, _package: &str, _dev: bool) -> anyhow::Result<()> {
    bail!("`luabox add` is not implemented yet — planned for P2 (see SPEC.md §18)")
}

pub fn remove(_cwd: &Path, _package: &str) -> anyhow::Result<()> {
    bail!("`luabox remove` is not implemented yet — planned for P2 (see SPEC.md §18)")
}

pub fn install(_cwd: &Path) -> anyhow::Result<()> {
    bail!("`luabox install` is not implemented yet — planned for P2 (see SPEC.md §18)")
}

pub fn update(_cwd: &Path, _package: Option<&str>) -> anyhow::Result<()> {
    bail!("`luabox update` is not implemented yet — planned for P2 (see SPEC.md §18)")
}

pub fn vendor(_cwd: &Path) -> anyhow::Result<()> {
    bail!("`luabox vendor` is not implemented yet — planned for P2 (see SPEC.md §18)")
}

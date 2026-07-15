//! Black-box acceptance for the nvm-style toolchain/run wiring (#3).
//!
//! Drives the real `luabox` binary against a temp-dir fixture with a pinned
//! toolchain that provisions a fake `luarocks`, plus a decoy `luarocks` on the
//! system `PATH`. Proves `luabox run` resolves the toolchain's luarocks before
//! `PATH` (both the bare-executable fallback and `[tasks]` shells), that the
//! generated `LUAROCKS_CONFIG` reaches the child, and that with no pin the
//! system `PATH` is used unchanged. The environment is controlled entirely via
//! `Command::env`, so nothing here touches the host's real toolchains.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

/// Write an executable shim named `stem` (`.cmd` on Windows) into `dir` that
/// prints `marker` and the child's `LUAROCKS_CONFIG`, so a test can tell which
/// shim ran and confirm the env injection reached it.
#[cfg(windows)]
fn write_shim(dir: &Path, stem: &str, marker: &str) {
    fs::create_dir_all(dir).unwrap();
    let body = format!("@echo off\r\necho {marker}\r\necho cfg=%LUAROCKS_CONFIG%\r\n");
    fs::write(dir.join(format!("{stem}.cmd")), body).unwrap();
}

#[cfg(unix)]
fn write_shim(dir: &Path, stem: &str, marker: &str) {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(dir).unwrap();
    let body = format!("#!/bin/sh\necho {marker}\necho \"cfg=$LUAROCKS_CONFIG\"\n");
    let path = dir.join(stem);
    fs::write(&path, body).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// A minimal, valid interpreter marker file so the toolchain is "installed"
/// (`toolchain_interpreter` finds it). It is never actually spawned by these
/// scenarios.
fn write_interpreter(toolchain_dir: &Path) {
    let name = if cfg!(windows) { "lua.cmd" } else { "lua" };
    fs::write(toolchain_dir.join(name), "").unwrap();
}

/// The scenario fixture: a `HOME`/toolchains root with a pinned `5.4`
/// toolchain (interpreter + provisioned luarocks shim), a decoy `luarocks` on
/// a separate PATH directory, and a project dir.
struct Fixture {
    root: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let toolchain = root.path().join("toolchains").join("5.4");
        fs::create_dir_all(&toolchain).unwrap();
        write_interpreter(&toolchain);
        // The toolchain's own luarocks, provisioned under `luarocks/`.
        write_shim(&toolchain.join("luarocks"), "luarocks", "TOOLCHAIN-LUAROCKS");
        // A decoy on the system PATH that must lose to the toolchain's.
        write_shim(&root.path().join("decoy"), "luarocks", "DECOY-PATH");
        fs::create_dir_all(root.path().join("proj")).unwrap();
        Self { root }
    }

    fn proj(&self) -> std::path::PathBuf {
        self.root.path().join("proj")
    }

    fn write_manifest(&self, extra: &str) {
        let manifest = format!(
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n{extra}"
        );
        fs::write(self.proj().join("luabox.toml"), manifest).unwrap();
    }

    fn pin(&self) {
        fs::write(
            self.proj().join("luabox-toolchain.toml"),
            "toolchain = \"5.4\"\n",
        )
        .unwrap();
    }

    /// Run `luabox <args...>` in the project with PATH = decoy dir first, then
    /// the inherited PATH, and the toolchains root pointed at the fixture.
    fn run(&self, args: &[&str]) -> Output {
        let decoy = self.root.path().join("decoy");
        let existing = std::env::var_os("PATH").unwrap_or_default();
        let mut dirs = vec![decoy];
        dirs.extend(std::env::split_paths(&existing));
        let path = std::env::join_paths(dirs).unwrap();

        Command::new(env!("CARGO_BIN_EXE_luabox"))
            .args(args)
            .current_dir(self.proj())
            .env("PATH", path)
            .env("LUABOX_TOOLCHAINS", self.root.path().join("toolchains"))
            .env_remove("LUABOX_LUA")
            .env_remove("LUABOX_RUN_DEPTH")
            .output()
            .expect("failed to spawn luabox")
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn run_resolves_toolchain_luarocks_before_path_decoy() {
    let fx = Fixture::new();
    fx.write_manifest("");
    fx.pin();

    let out = fx.run(&["run", "luarocks", "--", "install", "lpeg"]);
    let text = stdout(&out);
    assert!(out.status.success(), "run failed: {text}\n{out:?}");
    assert!(
        text.contains("TOOLCHAIN-LUAROCKS"),
        "expected the toolchain's luarocks, got:\n{text}"
    );
    assert!(
        !text.contains("DECOY-PATH"),
        "the PATH decoy won over the toolchain:\n{text}"
    );
    // The generated LUAROCKS_CONFIG reached the child (its filename appears in
    // the echoed `cfg=<path>` line).
    assert!(
        text.contains("luarocks-config.lua"),
        "LUAROCKS_CONFIG was not injected:\n{text}"
    );
}

#[test]
fn tasks_see_the_prepended_toolchain_path() {
    let fx = Fixture::new();
    // A task invoking bare `luarocks` — resolved by the shell via PATH, which
    // `luabox run` has prefixed with the toolchain's bin dirs.
    fx.write_manifest("[tasks]\nrocks = \"luarocks --version\"\n");
    fx.pin();

    let out = fx.run(&["run", "rocks"]);
    let text = stdout(&out);
    assert!(out.status.success(), "task failed: {text}\n{out:?}");
    assert!(
        text.contains("TOOLCHAIN-LUAROCKS"),
        "task did not hit the toolchain luarocks:\n{text}"
    );
    assert!(
        !text.contains("DECOY-PATH"),
        "task hit the PATH decoy:\n{text}"
    );
}

#[test]
fn no_pin_uses_the_system_path_unchanged() {
    let fx = Fixture::new();
    fx.write_manifest("");
    // No pin file: the toolchain must not be consulted; the decoy wins.
    let out = fx.run(&["run", "luarocks"]);
    let text = stdout(&out);
    assert!(out.status.success(), "run failed: {text}\n{out:?}");
    assert!(
        text.contains("DECOY-PATH"),
        "expected the system-PATH luarocks with no pin, got:\n{text}"
    );
    assert!(
        !text.contains("TOOLCHAIN-LUAROCKS"),
        "the toolchain was consulted despite no pin:\n{text}"
    );
}

//! Deeply nested source must survive the *whole* toolchain, not just the
//! parser (round-8 F5).
//!
//! `luabox-syntax`'s `MAX_DEPTH` (220) has a headroom proof — but it proves one
//! thing: that **parsing** at the limit fits a default 2 MiB thread stack
//! (`parser.rs`, `parsing_at_the_depth_limit_fits_a_default_stack`). Everything
//! downstream then walks trees 2.2x deeper than the reference implementations
//! accept, with no guard of its own: lowering, inference, the formatter, the
//! bundler and the diagnostic renderer all recurse over the same shape and none
//! of them was ever measured against a stack.
//!
//! So this drives the **real binary** over a real project, once per construct,
//! at the depth reference Lua accepts:
//!
//! * `luabox check` — parse + harvest + lower + typecheck + render;
//! * `luabox fmt` and `luabox fmt --check` — the formatter's own walk, and the
//!   round trip that proves its output is stable at depth;
//! * `luabox build --bundle --minify --sourcemap --target 5.1` — the check
//!   gate again, then lowering 5.4 -> 5.1, the bundler, the minifier and the
//!   source-map writer.
//!
//! The real binary rather than an in-process thread on purpose: a user's
//! `luabox check` runs on the binary's own threads — the dispatcher thread
//! `main` spawns AND the rayon workers the per-file `par_iter` spreads real
//! work across — and both are pinned to an explicit stack in `main.rs`
//! precisely because the platform defaults differ (8 MiB main on Linux/macOS,
//! **1 MiB** under MSVC, 2 MiB for unconfigured workers). Each project here
//! carries [`DEEP_FILES`] deep files so the workers genuinely participate;
//! see that constant for why one file would silently test only the
//! dispatcher. This is an ordinary workspace test with no platform gate, so
//! `ci.yml`'s `check` matrix runs it on ubuntu-latest, macos-latest *and*
//! windows-latest via `cargo test --workspace`. That is what closes the
//! Windows half of the gap: it is CI, not a local run, that proves it.
//!
//! ## Why 195 and not 220
//!
//! 195 is the depth `parser.rs`'s `everything_reference_lua_accepts_parses`
//! uses, and the margin is the contract LIMITATIONS.md states: `lua5.4` and
//! `luac5.4` reject at 197-198 of each of these constructs (`LUAI_MAXCCALLS` is
//! 200), so 195 is comfortably inside every reference implementation's limit
//! and therefore inside luabox's promise. Depths between 196 and `MAX_DEPTH`
//! are accepted by the parser as slack, not promised by the toolchain.

// test code — panics document assumptions
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::process::{Command, Output};

/// The nesting depth every leg runs at — see the module docs.
const DEPTH: usize = 195;

/// The constructs `parser.rs` pins its own depth guarantees on, in a form the
/// rest of the pipeline can also be handed: each is a complete Lua module that
/// checks clean and returns a value, so it can be a bundle entry point.
fn deep_module(construct: &str) -> String {
    match construct {
        "table" => format!(
            "local x = {}{}\nreturn x\n",
            "{".repeat(DEPTH),
            "}".repeat(DEPTH)
        ),
        "paren" => format!(
            "local x = {}1{}\nreturn x\n",
            "(".repeat(DEPTH),
            ")".repeat(DEPTH)
        ),
        "call" => format!(
            "local function f(v)\n  return v\nend\nlocal x = {}1{}\nreturn x\n",
            "f(".repeat(DEPTH),
            ")".repeat(DEPTH)
        ),
        "not" => format!("local x = {}true\nreturn x\n", "not ".repeat(DEPTH)),
        "if" => format!(
            "local c = true\nlocal x = 0\n{}x = 1 {}\nreturn x\n",
            "if c then ".repeat(DEPTH),
            "end ".repeat(DEPTH)
        ),
        other => panic!("unknown construct {other}"),
    }
}

/// How many deep files each project carries.
///
/// MORE THAN ONE IS LOAD-BEARING. `check` runs its per-file pipeline inside
/// `par_iter`, and with a single file rayon never leaves the calling thread —
/// which `main` pins at 16 MiB — so a one-file fixture can never observe a
/// WORKER overflowing (workers only got their pinned stack when `real_main`
/// gained `ThreadPoolBuilder::stack_size`; before that they sat on the 2 MiB
/// default and this test passed anyway). Enough files to out-number every
/// runner's cores forces genuine work-stealing, so the deep recursion
/// provably runs on workers too.
const DEEP_FILES: usize = 32;

/// A project whose `src/main.lua` **and** 31 sibling modules are each
/// `deep_module(construct)` — see [`DEEP_FILES`] for why the siblings exist.
fn deep_project(construct: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("luabox.toml"),
        "[package]\nname = \"deep\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n\n\
         [build]\ntarget = \"5.1\"\nout = \"dist\"\nentry = [\"src/main.lua\"]\n",
    )
    .expect("write luabox.toml");
    std::fs::create_dir(dir.path().join("src")).expect("mkdir src");
    let module = deep_module(construct);
    std::fs::write(dir.path().join("src").join("main.lua"), &module).expect("write main.lua");
    for n in 1..DEEP_FILES {
        std::fs::write(
            dir.path().join("src").join(format!("deep_{n:02}.lua")),
            &module,
        )
        .expect("write sibling module");
    }
    dir
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_luabox"))
        .args(args)
        .current_dir(root)
        .output()
        .expect("spawn luabox")
}

/// Run `luabox <args…>` and require it to succeed.
///
/// A stack overflow ends the process on a signal (SIGSEGV on unix, a
/// `STATUS_STACK_OVERFLOW` exception on Windows) rather than with an exit code,
/// so the assertion is deliberately on `success()` and prints both streams: a
/// failure here is either "the pipeline overflowed" or "the pipeline reported
/// something it should not have", and the output says which.
fn must_succeed(root: &Path, construct: &str, args: &[&str]) {
    let output = run(root, args);
    assert!(
        output.status.success(),
        "{DEPTH} nested {construct}s: `luabox {}` failed with {:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        args.join(" "),
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn the_whole_pipeline_survives_the_deepest_source_reference_lua_accepts() {
    for construct in ["table", "paren", "call", "not", "if"] {
        let project = deep_project(construct);
        let root = project.path();

        // Parse -> harvest -> lower -> typecheck -> render.
        must_succeed(root, construct, &["check"]);
        // ...and against a second dialect, so the legality walk runs twice.
        must_succeed(root, construct, &["check", "--target", "5.1"]);

        // The formatter's walk, then the round trip: reformatting deep source
        // must converge, or `fmt --check` fails on its own output.
        must_succeed(root, construct, &["fmt"]);
        must_succeed(root, construct, &["fmt", "--check"]);

        // Lowering 5.4 -> 5.1, the bundler, the minifier, the source map.
        must_succeed(
            root,
            construct,
            &["build", "--bundle", "--minify", "--sourcemap"],
        );
    }
}

#[test]
fn the_deep_fixtures_really_are_deep() {
    // Guards the test above: a fixture that had quietly stopped nesting would
    // let every leg pass without exercising anything. Checked through the
    // parser's own contract — reference Lua rejects these at ~197, so at 195
    // they must still be accepted (no diagnostics at all), and the source must
    // carry the nesting it claims.
    for construct in ["table", "paren", "call", "not", "if"] {
        let source = deep_module(construct);
        // The opener to count, and how many of it the module holds: `call`
        // carries one extra `(` in the `function f(v)` header its expression
        // needs, and the `if` fixture's own `if c then` prelude is part of the
        // nesting rather than extra.
        let (opener, want) = match construct {
            "table" => ("{", DEPTH),
            "paren" => ("(", DEPTH),
            "call" => ("(", DEPTH + 1),
            "not" => ("not ", DEPTH),
            _ => ("if c then ", DEPTH),
        };
        assert_eq!(
            source.matches(opener).count(),
            want,
            "{construct}: fixture is not {DEPTH} deep"
        );

        let project = deep_project(construct);
        let output = run(project.path(), &["check"]);
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("0 errors, 0 warnings"),
            "{construct}: the fixture must check clean, so a failure above is about depth\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

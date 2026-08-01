//! `gen-corpus` — deterministically generates a synthetic idiomatic-Lua
//! corpus used by `scripts/perf-gate.{sh,ps1}` to exercise the SPEC.md
//! §16.1 perf gates (cold start, `fmt`/`check` throughput on ~100 kLOC).
//!
//! Not a workspace member (see root `Cargo.toml` `[workspace] exclude`):
//! it is a build-time tool, not something the shipped `luabox` binary
//! depends on or bundles.
//!
//! Usage:
//!   gen-corpus --out <dir> [--seed <u64>] [--files <n>] [--lines-per-file <n>]
//!                          [--rock-tree [<X.Y>]]
//!
//! `--rock-tree` writes a whole *project* instead of a bare directory of
//! modules: the generated files land under
//! `<out>/lua_modules/share/lua/<X.Y>/`, the layout `luarocks install --tree
//! lua_modules` produces, beside a `luabox.toml` and a one-line
//! `src/main.lua`. That is the shape `scripts/lsp-startup-bench.sh` needs to
//! reproduce the language server's startup rock-harvest measurement, and it
//! is generated rather than described so the number can be re-measured
//! instead of quoted.
//!
//! Same seed + same flags always produces byte-identical output — the
//! generator uses its own tiny splitmix64 PRNG (no external dependency)
//! seeded once and consumed in a fixed order.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const NOUNS: &[&str] = &[
    "item", "value", "node", "entry", "record", "widget", "token", "packet", "cell", "unit",
    "frame", "chunk", "slot", "field", "key", "payload", "event", "job", "task", "resource",
    "buffer", "session", "message", "handle", "cursor",
];

const VERBS: &[&str] = &[
    "compute", "update", "merge", "normalize", "validate", "transform", "collect", "summarize",
    "filter", "dispatch", "render", "serialize", "parse", "resolve", "schedule", "reconcile",
];

const ADJECTIVES: &[&str] = &[
    "primary", "secondary", "cached", "pending", "active", "stale", "final", "raw", "normalized",
    "sorted", "shared", "local", "remote", "default",
];

const OPS: &[&str] = &["+", "-", "*"];

/// Tiny deterministic PRNG (splitmix64) so the corpus is reproducible
/// without pulling in the `rand` crate.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn range(&mut self, n: usize) -> usize {
        debug_assert!(n > 0);
        (self.next_u64() % n as u64) as usize
    }

    fn choice<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.range(items.len())]
    }

    /// True with probability `pct` percent (0..=100).
    fn chance(&mut self, pct: u64) -> bool {
        self.next_u64() % 100 < pct
    }
}

struct Args {
    out: PathBuf,
    seed: u64,
    files: usize,
    lines_per_file: usize,
    /// `Some(version)` writes a rock-tree project; `None` writes the modules
    /// straight into `--out`.
    rock_tree: Option<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut out = PathBuf::from("target/corpus");
    let mut seed: u64 = 42;
    let mut files: usize = 50;
    let mut lines_per_file: usize = 2000;
    let mut rock_tree: Option<String> = None;

    let mut args = std::env::args().skip(1).peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => {
                out = PathBuf::from(args.next().ok_or("--out requires a value")?);
            }
            "--seed" => {
                seed = args
                    .next()
                    .ok_or("--seed requires a value")?
                    .parse()
                    .map_err(|_| "--seed must be a u64".to_string())?;
            }
            "--files" => {
                files = args
                    .next()
                    .ok_or("--files requires a value")?
                    .parse()
                    .map_err(|_| "--files must be a usize".to_string())?;
            }
            "--lines-per-file" => {
                lines_per_file = args
                    .next()
                    .ok_or("--lines-per-file requires a value")?
                    .parse()
                    .map_err(|_| "--lines-per-file must be a usize".to_string())?;
            }
            "--rock-tree" => {
                // The version is optional: `--rock-tree` alone means 5.4.
                let version = match args.peek() {
                    Some(next) if !next.starts_with("--") => args.next(),
                    _ => None,
                };
                rock_tree = Some(version.unwrap_or_else(|| "5.4".to_string()));
            }
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument `{other}` (see --help)")),
        }
    }

    Ok(Args {
        out,
        seed,
        files,
        lines_per_file,
        rock_tree,
    })
}

fn print_help() {
    println!(
        "gen-corpus — deterministic synthetic Lua corpus generator\n\n\
         USAGE:\n    gen-corpus --out <dir> [--seed <u64>] [--files <n>] [--lines-per-file <n>]\n\n\
         OPTIONS:\n    \
         --out <dir>              output directory (default: target/corpus)\n    \
         --seed <u64>             PRNG seed, same seed => byte-identical output (default: 42)\n    \
         --files <n>              number of .lua files to write (default: 50)\n    \
         --lines-per-file <n>     approx. line count per file (default: 2000)\n    \
         --rock-tree [<X.Y>]      write a project whose modules live under\n                             \
         <out>/lua_modules/share/lua/<X.Y>/ (default version: 5.4),\n                             \
         with a luabox.toml and a src/main.lua beside them\n"
    );
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(err) => {
            eprintln!("gen-corpus: error: {err}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(err) = run(&args) {
        eprintln!("gen-corpus: error: {err}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn run(args: &Args) -> std::io::Result<()> {
    let modules_dir = match &args.rock_tree {
        Some(version) => args
            .out
            .join("lua_modules")
            .join("share")
            .join("lua")
            .join(version),
        None => args.out.clone(),
    };
    fs::create_dir_all(&modules_dir)?;
    let mut rng = Rng::new(args.seed);
    let mut total_lines = 0usize;

    for file_idx in 0..args.files {
        let content = gen_file(&mut rng, file_idx, args.lines_per_file);
        total_lines += content.matches('\n').count();
        let path = modules_dir.join(format!("module_{file_idx:04}.lua"));
        let mut f = fs::File::create(&path)?;
        f.write_all(content.as_bytes())?;
    }

    if let Some(version) = &args.rock_tree {
        write_project(&args.out, version)?;
    }

    println!(
        "gen-corpus: wrote {} files (~{total_lines} lines total, seed {}) to {}",
        args.files,
        args.seed,
        modules_dir.display()
    );
    Ok(())
}

/// The manifest and entry file that turn a bare `lua_modules/` tree into a
/// project the CLI and the language server both discover. `src/main.lua`
/// names one generated module so the file the benchmark opens is a realistic
/// consumer of the tree rather than an empty chunk.
fn write_project(out: &Path, version: &str) -> std::io::Result<()> {
    fs::create_dir_all(out.join("src"))?;
    fs::write(
        out.join("luabox.toml"),
        format!(
            "[package]\n\
             name = \"gen-corpus\"\n\
             version = \"0.0.0\"\n\
             edition = \"{version}\"\n\
             \n\
             [build]\n\
             target = \"{version}\"\n\
             \n\
             [types]\n\
             strict = true\n"
        ),
    )?;
    fs::write(
        out.join("src").join("main.lua"),
        "local m = require(\"module_0000\")\nreturn m\n",
    )
}

/// Generate one ~`target_lines`-line idiomatic Lua module: a mix of plain
/// functions, config tables, OOP metatable classes, and filter/loop
/// functions, each documented with `---@` annotations, wired up as a
/// `local M = {}` module that exports a random subset of its members.
fn gen_file(rng: &mut Rng, file_idx: usize, target_lines: usize) -> String {
    let mut out = String::with_capacity(target_lines * 32);
    let mut line_count = 0usize;

    push(&mut out, &mut line_count, &format!(
        "-- Module {file_idx:04} — generated corpus file (deterministic; see tools/gen-corpus).\n\
         -- Do not hand-edit; regenerate with `cargo run -p gen-corpus --release -- --out <dir>`.\n\n\
         local M = {{}}\n\n"
    ));

    let mut counter = 0usize;
    let mut exported = Vec::new();

    while line_count < target_lines {
        counter += 1;
        let name = format!("f{file_idx:04}_{counter:04}");
        let block = match rng.range(4) {
            0 => gen_function(rng, &name),
            1 => gen_table(rng, &name),
            2 => gen_class(rng, &name),
            _ => gen_filter_function(rng, &name),
        };
        push(&mut out, &mut line_count, &block);
        push(&mut out, &mut line_count, "\n");

        if rng.chance(35) {
            exported.push(name);
        }
    }

    if !exported.is_empty() {
        for name in &exported {
            push(&mut out, &mut line_count, &format!("M.{name} = {name}\n"));
        }
        push(&mut out, &mut line_count, "\n");
    }
    push(&mut out, &mut line_count, "return M\n");

    out
}

fn push(out: &mut String, line_count: &mut usize, s: &str) {
    *line_count += s.matches('\n').count();
    out.push_str(s);
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// A small pure-ish function with numeric params and `---@` annotations.
fn gen_function(rng: &mut Rng, name: &str) -> String {
    let verb = rng.choice(VERBS);
    let noun = rng.choice(NOUNS);
    let op = rng.choice(OPS);
    format!(
        "---{cap_verb} the {noun} derived from `a` and `b`.\n\
         ---@param a number\n\
         ---@param b number\n\
         ---@return number\n\
         local function {name}(a, b)\n\
         \x20   local result = a {op} b\n\
         \x20   if result < 0 then\n\
         \x20       result = -result\n\
         \x20   end\n\
         \x20   return result\n\
         end\n",
        cap_verb = capitalize(verb),
    )
}

/// A config-shaped table literal.
fn gen_table(rng: &mut Rng, name: &str) -> String {
    let noun = rng.choice(NOUNS);
    let adj = rng.choice(ADJECTIVES);
    let id = rng.range(10_000);
    let weight = rng.range(100);
    let enabled = rng.chance(70);
    format!(
        "---@class {name}\n\
         -- {cap_adj} {noun} configuration table.\n\
         local {name} = {{\n\
         \x20   id = {id},\n\
         \x20   label = \"{noun}\",\n\
         \x20   enabled = {enabled},\n\
         \x20   weight = {weight},\n\
         \x20   tags = {{ \"{adj}\", \"{noun}\" }},\n\
         }}\n",
        cap_adj = capitalize(adj),
    )
}

/// An OOP metatable class: constructor + two methods.
fn gen_class(rng: &mut Rng, name: &str) -> String {
    let noun = rng.choice(NOUNS);
    format!(
        "---@class {name}\n\
         ---@field kind string\n\
         ---@field value number\n\
         local {name} = {{}}\n\
         {name}.__index = {name}\n\
         \n\
         ---Create a new {name}.\n\
         ---@param value number\n\
         ---@return {name}\n\
         function {name}.new(value)\n\
         \x20   local self = setmetatable({{}}, {name})\n\
         \x20   self.kind = \"{noun}\"\n\
         \x20   self.value = value\n\
         \x20   return self\n\
         end\n\
         \n\
         ---Return a formatted description of the {noun}.\n\
         ---@return string\n\
         function {name}:describe()\n\
         \x20   return string.format(\"%s(%s=%d)\", self.kind, \"{noun}\", self.value)\n\
         end\n\
         \n\
         ---Scale the underlying value in place.\n\
         ---@param factor number\n\
         ---@return number\n\
         function {name}:scale(factor)\n\
         \x20   self.value = self.value * factor\n\
         \x20   return self.value\n\
         end\n"
    )
}

/// A loop-driven filter function over a numeric list.
fn gen_filter_function(rng: &mut Rng, name: &str) -> String {
    let noun = rng.choice(NOUNS);
    format!(
        "---Collect the {noun} entries that meet a threshold.\n\
         ---@param items number[]\n\
         ---@param threshold number\n\
         ---@return number[]\n\
         local function {name}(items, threshold)\n\
         \x20   local out = {{}}\n\
         \x20   for i = 1, #items do\n\
         \x20       local v = items[i]\n\
         \x20       if v >= threshold then\n\
         \x20           out[#out + 1] = v\n\
         \x20       end\n\
         \x20   end\n\
         \x20   return out\n\
         end\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_for_same_seed() {
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn gen_file_hits_target_line_count_roughly() {
        let mut rng = Rng::new(1);
        let content = gen_file(&mut rng, 0, 200);
        let lines = content.matches('\n').count();
        assert!(lines >= 200, "expected at least 200 lines, got {lines}");
        assert!(content.trim_end().ends_with("return M"));
    }

    #[test]
    fn gen_file_is_deterministic() {
        let mut r1 = Rng::new(99);
        let mut r2 = Rng::new(99);
        let a = gen_file(&mut r1, 3, 150);
        let b = gen_file(&mut r2, 3, 150);
        assert_eq!(a, b);
    }
}

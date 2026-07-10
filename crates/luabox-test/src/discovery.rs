//! Zero-config test discovery (SPEC.md §11).
//!
//! A `.lua` file is a test file if **any** of these hold:
//!   * its name ends with `_test.lua` (e.g. `math_test.lua`), or
//!   * its name ends with `.test.lua` (e.g. `math.test.lua`), or
//!   * it lives anywhere under a directory named `tests/` — searched
//!     **recursively**, so `tests/unit/math.lua` counts too.
//!
//! `*.d.lua` definition files are never tests. The walk skips dot-entries
//! (`.git/`, editor state) and the project's build output directory. Result
//! order is deterministic (sorted by entry name at every level).

use std::path::{Path, PathBuf};

/// Discover every test file under `root`, excluding `out_dir` (the build
/// output directory, if any). Deterministically ordered.
#[must_use]
pub fn discover(root: &Path, out_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut found = Vec::new();
    walk(root, out_dir, false, &mut found);
    found
}

/// `in_tests` becomes true once we descend into a `tests/` directory, which
/// makes every `.lua` file below it a test regardless of its name.
fn walk(dir: &Path, out_dir: Option<&Path>, in_tests: bool, found: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = read.flatten().collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);

    for entry in entries {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            if out_dir == Some(path.as_path()) {
                continue;
            }
            let child_in_tests = in_tests || name == "tests";
            walk(&path, out_dir, child_in_tests, found);
        } else if is_test_file(&name, in_tests) {
            found.push(path);
        }
    }
}

/// Whether a file (given its name and whether it sits under `tests/`) is a
/// test file. Pure — the core of the discovery rules, unit-tested directly.
#[must_use]
pub fn is_test_file(name: &str, in_tests: bool) -> bool {
    let is_lua = Path::new(name)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("lua"));
    if !is_lua || name.ends_with(".d.lua") {
        return false;
    }
    in_tests || name.ends_with("_test.lua") || name.ends_with(".test.lua")
}

#[cfg(test)]
mod tests {
    use super::{discover, is_test_file};
    use std::fs;

    #[test]
    fn naming_rules() {
        assert!(is_test_file("math_test.lua", false));
        assert!(is_test_file("math.test.lua", false));
        assert!(!is_test_file("math.lua", false));
        // Under tests/, any .lua counts.
        assert!(is_test_file("math.lua", true));
        // Definition files never count, even under tests/.
        assert!(!is_test_file("api.d.lua", true));
        // Non-lua never counts.
        assert!(!is_test_file("README.md", true));
    }

    #[test]
    fn discovers_by_name_and_tests_dir_recursively() {
        let root = tempfile::tempdir().unwrap();
        let p = root.path();
        fs::create_dir_all(p.join("src")).unwrap();
        fs::create_dir_all(p.join("tests/unit")).unwrap();
        fs::create_dir_all(p.join("dist")).unwrap();
        fs::create_dir_all(p.join(".git")).unwrap();

        fs::write(p.join("src/math_test.lua"), "").unwrap();
        fs::write(p.join("src/math.test.lua"), "").unwrap();
        fs::write(p.join("src/math.lua"), "").unwrap(); // not a test
        fs::write(p.join("src/api.d.lua"), "").unwrap(); // def, not a test
        fs::write(p.join("tests/basic.lua"), "").unwrap(); // under tests/
        fs::write(p.join("tests/unit/deep.lua"), "").unwrap(); // recursive
        fs::write(p.join("dist/built_test.lua"), "").unwrap(); // in out dir
        fs::write(p.join(".git/hook_test.lua"), "").unwrap(); // dot dir

        let out = p.join("dist");
        let mut got: Vec<String> = discover(p, Some(&out))
            .iter()
            .map(|f| {
                f.strip_prefix(p)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        got.sort();

        assert_eq!(
            got,
            vec![
                "src/math.test.lua",
                "src/math_test.lua",
                "tests/basic.lua",
                "tests/unit/deep.lua",
            ]
        );
    }
}

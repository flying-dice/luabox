//! Renderers: one function per output format (SPEC.md §14).
//!
//! Every renderer takes the diagnostics plus a source-lookup callback,
//! `Fn(&str) -> Option<String>`, which returns the text of a file so snippets
//! and line/column can be computed. Callbacks are passed as trait objects so
//! the renderers stay monomorphisation-free and easy to dispatch over.

use std::fmt::Write as _;

use serde_json::json;

use crate::code::Severity;
use crate::diagnostic::{Diagnostic, Label};
use crate::registry;

/// A source-lookup callback: file name in, whole-file text out.
pub type SourceLookup<'a> = &'a dyn Fn(&str) -> Option<String>;

/// The machine and human output formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// rustc-style human-readable text.
    Human,
    /// Stable JSON (the `Diagnostic` serde structure).
    Json,
    /// SARIF 2.1.0 (static analysis interchange).
    Sarif,
    /// GitHub Actions workflow commands (`::error ...::`).
    GithubActions,
    /// GitLab Code Quality report (JSON array).
    GitlabCodeQuality,
}

/// Render diagnostics to the requested format.
#[must_use]
pub fn render(diags: &[Diagnostic], format: Format, lookup: SourceLookup<'_>) -> String {
    match format {
        Format::Human => render_human(diags, lookup),
        Format::Json => render_json(diags),
        Format::Sarif => render_sarif(diags, lookup),
        Format::GithubActions => render_github_actions(diags, lookup),
        Format::GitlabCodeQuality => render_gitlab_code_quality(diags, lookup),
    }
}

/// 1-based line and column for a byte offset in `source`.
fn line_col(source: &str, offset: usize) -> (usize, usize) {
    let mut line = 1usize;
    let mut col = 1usize;
    for (idx, ch) in source.char_indices() {
        if idx >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// The text of a 1-based line, without its terminator.
fn line_text(source: &str, line: usize) -> &str {
    source.lines().nth(line.saturating_sub(1)).unwrap_or("")
}

/// Character length of a byte range, clamped so a zero-width span still shows
/// one caret.
fn caret_width(source: &str, range: &std::ops::Range<usize>) -> usize {
    let slice = source.get(range.clone()).unwrap_or("");
    slice.chars().count().max(1)
}

// ---- Human --------------------------------------------------------------

/// rustc-style plain-text rendering.
#[must_use]
pub fn render_human(diags: &[Diagnostic], lookup: SourceLookup<'_>) -> String {
    let mut out = String::new();
    for (i, diag) in diags.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        render_one_human(&mut out, diag, lookup);
    }
    out
}

fn render_one_human(out: &mut String, diag: &Diagnostic, lookup: SourceLookup<'_>) {
    let _ = writeln!(
        out,
        "{}[{}]: {}",
        diag.severity.keyword(),
        diag.code,
        diag.message
    );

    // Primary label first, then secondary labels, so the error site leads.
    let mut labels: Vec<&Label> = diag.labels.iter().filter(|l| l.primary).collect();
    labels.extend(diag.labels.iter().filter(|l| !l.primary));

    for label in labels {
        render_label_human(out, label, lookup);
    }

    for suggestion in &diag.suggestions {
        let _ = writeln!(out, "help: {}", suggestion.message);
        if !suggestion.replacement.is_empty() {
            let _ = writeln!(out, "     replace with: {}", suggestion.replacement);
        }
    }

    for note in &diag.notes {
        let _ = writeln!(out, "note: {note}");
    }
}

fn render_label_human(out: &mut String, label: &Label, lookup: SourceLookup<'_>) {
    let file = &label.span.file;
    let Some(source) = lookup(file) else {
        // No source available: still report where and what.
        let _ = writeln!(out, " --> {file} (bytes {:?})", label.span.range);
        if !label.message.is_empty() {
            let _ = writeln!(out, "     {}", label.message);
        }
        return;
    };

    let (line, col) = line_col(&source, label.span.range.start);
    let text = line_text(&source, line);
    let gutter = line.to_string();
    let pad = " ".repeat(gutter.len());
    let caret = if label.primary { '^' } else { '-' };
    let underline: String =
        std::iter::repeat_n(caret, caret_width(&source, &label.span.range)).collect();
    let indent = " ".repeat(col.saturating_sub(1));

    let _ = writeln!(out, "{pad} --> {file}:{line}:{col}");
    let _ = writeln!(out, "{pad} |");
    let _ = writeln!(out, "{gutter} | {text}");
    if label.message.is_empty() {
        let _ = writeln!(out, "{pad} | {indent}{underline}");
    } else {
        let _ = writeln!(out, "{pad} | {indent}{underline} {}", label.message);
    }
}

// ---- JSON ---------------------------------------------------------------

/// Stable JSON: the pretty-printed `Diagnostic` serde structure.
#[must_use]
pub fn render_json(diags: &[Diagnostic]) -> String {
    serde_json::to_string_pretty(diags).unwrap_or_else(|_| "[]".to_string())
}

// ---- SARIF --------------------------------------------------------------

fn sarif_level(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
    }
}

/// SARIF 2.1.0, minimal but valid: one run, driver `luabox`, rules pulled from
/// the registry for every code that appears, and one result per diagnostic.
#[must_use]
pub fn render_sarif(diags: &[Diagnostic], lookup: SourceLookup<'_>) -> String {
    // De-duplicate rules by code, preserving first-seen order.
    let mut rule_ids: Vec<String> = Vec::new();
    let mut rules = Vec::new();
    for diag in diags {
        let id = diag.code.to_string();
        if rule_ids.contains(&id) {
            continue;
        }
        rule_ids.push(id.clone());
        let mut rule = json!({ "id": id });
        if let Some(entry) = registry::explain(&diag.code) {
            rule["name"] = json!(entry.title);
            rule["shortDescription"] = json!({ "text": entry.title });
            rule["fullDescription"] = json!({ "text": entry.explain });
        }
        rules.push(rule);
    }

    let results: Vec<_> = diags
        .iter()
        .map(|diag| {
            let mut result = json!({
                "ruleId": diag.code.to_string(),
                "level": sarif_level(diag.severity),
                "message": { "text": diag.message },
            });
            if let Some(location) = sarif_location(diag, lookup) {
                result["locations"] = json!([location]);
            }
            result
        })
        .collect();

    let doc = json!({
        "version": "2.1.0",
        "$schema": "https://raw.githubusercontent.com/oasis-tcs/sarif-spec/master/Schemata/sarif-schema-2.1.0.json",
        "runs": [{
            "tool": { "driver": { "name": "luabox", "rules": rules } },
            "results": results,
        }],
    });
    serde_json::to_string_pretty(&doc).unwrap_or_else(|_| "{}".to_string())
}

fn sarif_location(diag: &Diagnostic, lookup: SourceLookup<'_>) -> Option<serde_json::Value> {
    let label = diag.primary_label()?;
    let mut region = json!({});
    if let Some(source) = lookup(&label.span.file) {
        let (line, col) = line_col(&source, label.span.range.start);
        region["startLine"] = json!(line);
        region["startColumn"] = json!(col);
    }
    Some(json!({
        "physicalLocation": {
            "artifactLocation": { "uri": label.span.file },
            "region": region,
        }
    }))
}

// ---- GitHub Actions -----------------------------------------------------

fn github_command(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
    }
}

/// GitHub Actions workflow commands, one per diagnostic:
/// `::error file=...,line=...,col=...::<code> <message>`.
#[must_use]
pub fn render_github_actions(diags: &[Diagnostic], lookup: SourceLookup<'_>) -> String {
    let mut out = String::new();
    for diag in diags {
        let command = github_command(diag.severity);
        let mut props = String::new();
        if let Some(label) = diag.primary_label() {
            let _ = write!(props, "file={}", label.span.file);
            if let Some(source) = lookup(&label.span.file) {
                let (line, col) = line_col(&source, label.span.range.start);
                let _ = write!(props, ",line={line},col={col}");
            }
        }
        let message = escape_github(&format!("{}: {}", diag.code, diag.message));
        if props.is_empty() {
            let _ = writeln!(out, "::{command}::{message}");
        } else {
            let _ = writeln!(out, "::{command} {props}::{message}");
        }
    }
    out
}

/// Escape the reserved characters in a GitHub workflow-command message.
fn escape_github(message: &str) -> String {
    message
        .replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

// ---- GitLab Code Quality ------------------------------------------------

fn gitlab_severity(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "major",
        Severity::Warning => "minor",
    }
}

/// A stable FNV-1a fingerprint over a diagnostic's identity (code + file +
/// range). Deterministic across runs and Rust versions.
fn fingerprint(diag: &Diagnostic) -> String {
    let (file, start, end) = diag.primary_label().map_or_else(
        || (String::new(), 0, 0),
        |l| (l.span.file.clone(), l.span.range.start, l.span.range.end),
    );
    let key = format!("{}:{file}:{start}:{end}", diag.code);
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in key.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// GitLab Code Quality report: a JSON array of issues.
#[must_use]
pub fn render_gitlab_code_quality(diags: &[Diagnostic], _lookup: SourceLookup<'_>) -> String {
    let issues: Vec<_> = diags
        .iter()
        .map(|diag| {
            let (path, begin) = diag.primary_label().map_or_else(
                || (String::new(), 0),
                |l| {
                    // GitLab wants a 1-based line, but without source we fall
                    // back to a byte-offset-derived begin of 1.
                    (l.span.file.clone(), 1)
                },
            );
            json!({
                "description": diag.message,
                "check_name": diag.code.to_string(),
                "fingerprint": fingerprint(diag),
                "severity": gitlab_severity(diag.severity),
                "location": { "path": path, "lines": { "begin": begin } },
            })
        })
        .collect();
    serde_json::to_string_pretty(&issues).unwrap_or_else(|_| "[]".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code::Code;
    use crate::diagnostic::{Span, Suggestion};

    const SRC: &str = "local x = 1\nlocal = 2\nprint(x)\n";

    fn fixture() -> Vec<Diagnostic> {
        let err_code: Code = "LB0001".parse().unwrap();
        let warn_code: Code = "LB1001".parse().unwrap();
        // Byte offsets into SRC: "local = 2" starts at 12; the `=` is at 18.
        let error = Diagnostic::error(err_code, "unexpected token `=`")
            .with_label(Label::primary(
                Span::new("main.lua", 18..19),
                "expected an identifier",
            ))
            .with_label(Label::secondary(
                Span::new("main.lua", 12..17),
                "while parsing this local",
            ))
            .with_suggestion(Suggestion::new(
                Span::new("main.lua", 18..18),
                "name ",
                "give the local a name",
            ))
            .with_note("Lua locals require a name before `=`.");
        let warning = Diagnostic::warning(warn_code, "edition `6.0` is not recognised").with_label(
            Label::primary(Span::new("luabox.toml", 0..3), "unknown edition"),
        );
        vec![error, warning]
    }

    fn lookup(file: &str) -> Option<String> {
        match file {
            "main.lua" => Some(SRC.to_string()),
            "luabox.toml" => Some("edition = \"6.0\"\n".to_string()),
            _ => None,
        }
    }

    #[test]
    fn human_is_rustc_shaped() {
        let out = render(&fixture(), Format::Human, &lookup);
        assert!(out.contains("error[LB0001]: unexpected token `=`"), "{out}");
        assert!(out.contains("--> main.lua:2:7"), "{out}");
        assert!(out.contains('^'), "{out}");
        assert!(out.contains("expected an identifier"), "{out}");
        assert!(out.contains("while parsing this local"), "{out}");
        assert!(out.contains("help: give the local a name"), "{out}");
        assert!(out.contains("note: Lua locals require a name"), "{out}");
        assert!(out.contains("warning[LB1001]"), "{out}");
    }

    #[test]
    fn json_is_stable_and_parses() {
        let out = render(&fixture(), Format::Json, &lookup);
        let value: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(value[0]["code"], "LB0001");
        assert_eq!(value[0]["severity"], "error");
        assert_eq!(value[0]["labels"][0]["primary"], true);
        assert_eq!(value[1]["code"], "LB1001");
        assert_eq!(value[1]["severity"], "warning");
    }

    #[test]
    fn sarif_is_valid_json_with_expected_shape() {
        let out = render(&fixture(), Format::Sarif, &lookup);
        let value: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(value["version"], "2.1.0");
        assert_eq!(value["runs"][0]["tool"]["driver"]["name"], "luabox");
        let rules = &value["runs"][0]["tool"]["driver"]["rules"];
        assert_eq!(rules[0]["id"], "LB0001");
        let results = &value["runs"][0]["results"];
        assert_eq!(results[0]["ruleId"], "LB0001");
        assert_eq!(results[0]["level"], "error");
        assert_eq!(
            results[0]["locations"][0]["physicalLocation"]["region"]["startLine"],
            2
        );
        assert_eq!(results[1]["level"], "warning");
    }

    #[test]
    fn github_actions_emits_workflow_commands() {
        let out = render(&fixture(), Format::GithubActions, &lookup);
        assert!(
            out.contains("::error file=main.lua,line=2,col=7::LB0001: unexpected token `=`"),
            "{out}"
        );
        assert!(out.contains("::warning file=luabox.toml"), "{out}");
    }

    #[test]
    fn gitlab_code_quality_is_an_array_with_fingerprints() {
        let out = render(&fixture(), Format::GitlabCodeQuality, &lookup);
        let value: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(value.is_array());
        assert_eq!(value[0]["check_name"], "LB0001");
        assert_eq!(value[0]["severity"], "major");
        assert_eq!(value[1]["severity"], "minor");
        let fp = value[0]["fingerprint"].as_str().unwrap();
        assert_eq!(fp.len(), 16);
        // Fingerprints are deterministic.
        let again = render(&fixture(), Format::GitlabCodeQuality, &lookup);
        let value2: serde_json::Value = serde_json::from_str(&again).unwrap();
        assert_eq!(value2[0]["fingerprint"], value[0]["fingerprint"]);
    }

    #[test]
    fn human_without_source_still_renders() {
        let code: Code = "LB0001".parse().unwrap();
        let diag = Diagnostic::error(code, "boom")
            .with_label(Label::primary(Span::new("missing.lua", 0..3), "here"));
        let out = render(&[diag], Format::Human, &|_| None);
        assert!(out.contains("error[LB0001]: boom"), "{out}");
        assert!(out.contains("missing.lua"), "{out}");
    }
}

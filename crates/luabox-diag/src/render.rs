//! Renderers: one function per output format (SPEC.md §14).
//!
//! Every renderer takes the diagnostics plus a source-lookup callback,
//! `Fn(&str) -> Option<String>`, which returns the text of a file so snippets
//! and line/column can be computed. Callbacks are passed as trait objects so
//! the renderers stay monomorphisation-free and easy to dispatch over.
//!
//! The callback is expensive — the CLI's reads a file off disk — and returns
//! the text *owned*, so calling it once per label cloned the whole file per
//! diagnostic. Every renderer therefore goes through [`Sources`], a per-render
//! cache that calls the lookup once per distinct file and keeps the text
//! alongside an [`IndexedSource`] line table.
//!
//! # Cost
//!
//! Rendering *n* diagnostics over a file of size *f* costs O(f + n log f),
//! with no term in `n x f` left:
//!
//! - the source is fetched and indexed once per distinct file, not per label
//!   ([`Sources`]);
//! - line **and** column are binary searches over that index — the column
//!   used to be counted by walking from the line start, which is O(file) per
//!   label on a single-line file (see the `line_index` module docs);
//! - the human renderer prints a *window* of the source line, never the whole
//!   line ([`window_line`]), so one enormous line cannot make the **output**
//!   quadratic even when the lookups are not.
//!
//! What is still linear in a single diagnostic's own size — and deliberately
//! so — is its span: `caret_width` counts the characters a label covers.
//! A diagnostic that spans a megabyte pays for that megabyte once.

use std::collections::HashMap;
use std::fmt::Write as _;

use serde_json::json;

use crate::code::Severity;
use crate::diagnostic::{Diagnostic, Label};
use crate::line_index::IndexedSource;
use crate::registry;

/// A source-lookup callback: file name in, whole-file text out.
pub type SourceLookup<'a> = &'a dyn Fn(&str) -> Option<String>;

/// The source files one render call needs, fetched and indexed once each.
///
/// A miss is cached too (as `None`), so a label naming a file the lookup
/// cannot supply — a generated path, a file deleted since the diagnostic was
/// produced — does not re-ask for it once per diagnostic.
struct Sources<'a> {
    lookup: SourceLookup<'a>,
    files: HashMap<String, Option<IndexedSource>>,
}

impl<'a> Sources<'a> {
    fn new(lookup: SourceLookup<'a>) -> Self {
        Sources {
            lookup,
            files: HashMap::new(),
        }
    }

    /// The indexed text of `file`, or `None` if the lookup cannot supply it.
    fn get(&mut self, file: &str) -> Option<&IndexedSource> {
        if !self.files.contains_key(file) {
            let indexed = (self.lookup)(file).map(IndexedSource::new);
            self.files.insert(file.to_owned(), indexed);
        }
        self.files.get(file).and_then(Option::as_ref)
    }
}

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
    let mut sources = Sources::new(lookup);
    let mut out = String::new();
    for (i, diag) in diags.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        render_one_human(&mut out, diag, &mut sources);
    }
    out
}

fn render_one_human(out: &mut String, diag: &Diagnostic, sources: &mut Sources<'_>) {
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
        render_label_human(out, label, sources);
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

/// Widest source line the human renderer prints in full. Past this the line
/// is windowed around the label (rustc does the same).
///
/// This is a bound on *output*, not just on comfort. Without it every label
/// on a long line printed the whole line plus a column-wide run of spaces:
/// 10 k diagnostics on a 377 kB single-line file produced 3.5 GB of stdout,
/// which is neither readable nor, at that size, survivable.
const HUMAN_LINE_WINDOW: usize = 200;

/// How many characters of the window sit to the left of the label, so a span
/// is shown with some of what precedes it rather than flush against the edge.
const HUMAN_LINE_MARGIN: usize = 20;

/// Marks a windowed line as continuing past what is shown.
const ELLIPSIS: &str = "...";

/// One source line as the human renderer should print it, windowed if it is
/// too long to print whole.
struct LineWindow {
    /// What to put after the `N | ` gutter.
    text: String,
    /// Characters of padding before the caret run.
    indent: usize,
    /// Caret run length, clipped to what the window actually shows.
    width: usize,
}

/// Window `line` around a label at 1-based character column `col` / byte
/// offset `span_off` *within the line*, whose caret run is `width` characters.
///
/// A line no wider than [`HUMAN_LINE_WINDOW`] is returned untouched, with the
/// caret placement the renderer has always used — byte-for-byte the same
/// output as before windowing existed. Only a longer line is cut down, to at
/// most `HUMAN_LINE_WINDOW` characters plus `...` markers on whichever sides
/// continue. Column *numbers* are never windowed: the `file:line:col` header
/// keeps naming the true column, in every format.
///
/// The label sits [`HUMAN_LINE_MARGIN`] characters into the window when
/// there is line on both sides of it; when the line runs out on the right,
/// the window takes the slack back on the left, so it always shows as much
/// as it is allowed to.
///
/// Every step is bounded by the window, never by the line: the long-line test
/// stops after `HUMAN_LINE_WINDOW + 1` characters, and each of the two walks
/// that find the window's edges takes at most `HUMAN_LINE_WINDOW` steps —
/// `char_indices().rev()` walks back from the *end* of the head slice, so the
/// leading part of a megabyte-long line is never touched. That is what keeps
/// a one-line file from being quadratic to render.
fn window_line(line: &str, col: usize, span_off: usize, width: usize) -> LineWindow {
    if line.chars().nth(HUMAN_LINE_WINDOW).is_none() {
        return LineWindow {
            text: line.to_owned(),
            indent: col.saturating_sub(1),
            width,
        };
    }

    // An offset past the line's end (a label on the `\n`, or on the `\r` of a
    // CRLF pair, which `line_text` has already stripped) pins to the end.
    let span_off = span_off.min(line.len());

    // Right edge first: at most all but the left margin, but take whatever
    // the line actually has.
    let tail = line.get(span_off..).unwrap_or("");
    let mut right = 0usize;
    let mut end = line.len();
    for (i, _) in tail.char_indices() {
        if right == HUMAN_LINE_WINDOW - HUMAN_LINE_MARGIN {
            end = span_off + i;
            break;
        }
        right += 1;
    }

    // Left edge: the margin, plus whatever the right side did not spend.
    let budget = HUMAN_LINE_WINDOW - right;
    let head = line.get(..span_off).unwrap_or("");
    let mut start = span_off;
    let mut left = 0usize;
    for (i, _) in head.char_indices().rev() {
        if left == budget {
            break;
        }
        start = i;
        left += 1;
    }

    let mut text = String::new();
    if start > 0 {
        text.push_str(ELLIPSIS);
    }
    text.push_str(line.get(start..end).unwrap_or(""));
    if end < line.len() {
        text.push_str(ELLIPSIS);
    }
    LineWindow {
        indent: left + if start > 0 { ELLIPSIS.len() } else { 0 },
        // Clip the carets to the characters still on screen, so a span
        // running off the window does not underline the `...` and beyond.
        width: line
            .get(span_off..end)
            .unwrap_or("")
            .chars()
            .take(width)
            .count()
            .max(1),
        text,
    }
}

fn render_label_human(out: &mut String, label: &Label, sources: &mut Sources<'_>) {
    let file = &label.span.file;
    let Some(source) = sources.get(file) else {
        // No source available: still report where and what.
        let _ = writeln!(out, " --> {file} (bytes {:?})", label.span.range);
        if !label.message.is_empty() {
            let _ = writeln!(out, "     {}", label.message);
        }
        return;
    };

    let (line, col) = source.line_col(label.span.range.start);
    let span_off = label
        .span
        .range
        .start
        .saturating_sub(source.line_range(line).start);
    let shown = window_line(
        source.line_text(line),
        col,
        span_off,
        caret_width(source.text(), &label.span.range),
    );
    let gutter = line.to_string();
    let pad = " ".repeat(gutter.len());
    let caret = if label.primary { '^' } else { '-' };
    let underline: String = std::iter::repeat_n(caret, shown.width).collect();
    let indent = " ".repeat(shown.indent);
    let text = shown.text;

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

    let mut sources = Sources::new(lookup);
    let results: Vec<_> = diags
        .iter()
        .map(|diag| {
            let mut result = json!({
                "ruleId": diag.code.to_string(),
                "level": sarif_level(diag.severity),
                "message": { "text": diag.message },
            });
            if let Some(location) = sarif_location(diag, &mut sources) {
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

fn sarif_location(diag: &Diagnostic, sources: &mut Sources<'_>) -> Option<serde_json::Value> {
    let label = diag.primary_label()?;
    let mut region = json!({});
    if let Some(source) = sources.get(&label.span.file) {
        let (line, col) = source.line_col(label.span.range.start);
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
    let mut sources = Sources::new(lookup);
    let mut out = String::new();
    for diag in diags {
        let command = github_command(diag.severity);
        let mut props = String::new();
        if let Some(label) = diag.primary_label() {
            let _ = write!(props, "file={}", label.span.file);
            if let Some(source) = sources.get(&label.span.file) {
                let (line, col) = source.line_col(label.span.range.start);
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

/// The path an unspanned **manifest-level** finding (`LB1xxx` — the
/// manifest/config block, see [`crate::code::Code`]) reports.
const MANIFEST_PATH: &str = "luabox.toml";

/// The path any other genuinely fileless finding reports: the project root,
/// spelled the way a repository-relative path is.
const PROJECT_ROOT_PATH: &str = ".";

fn gitlab_severity(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "major",
        Severity::Warning => "minor",
    }
}

/// The code block that owns `luabox.toml` — manifest/config diagnostics
/// (`LB1xxx`: editions, `[types] defs`, `[lint]` keys). See
/// [`Code::block`](crate::code::Code::block).
const MANIFEST_BLOCK: u16 = 1;

/// Where a fileless diagnostic's finding is reported.
///
/// GitLab's Code Quality parser rejects an issue whose `location.path` is
/// empty, so a diagnostic with no span cannot simply say "nowhere" — it needs
/// a stable synthetic path, and which one it gets is decided per **code
/// family**, by what the finding is actually about:
///
/// - the manifest block ([`MANIFEST_BLOCK`] — `LB1001` an unrecognised
///   edition, `LB1002` an unresolvable `[types] defs` package, `LB1004` an
///   unknown `[lint]` key) is a statement about `luabox.toml`, so it reports
///   [`MANIFEST_PATH`]. Those findings then land on the merge-request diff
///   whenever the manifest is part of it — precisely when they are
///   actionable;
/// - anything else genuinely fileless belongs to the project as a whole and
///   reports [`PROJECT_ROOT_PATH`].
///
/// Both are compile-time constants keyed off the code, never a scanned or
/// configured value, so the same finding resolves to the same path on every
/// run over every checkout — which is what lets [`fingerprint`] hash it.
const fn synthetic_path(code: crate::code::Code) -> &'static str {
    if code.block() == MANIFEST_BLOCK {
        MANIFEST_PATH
    } else {
        PROJECT_ROOT_PATH
    }
}

/// The `location` a GitLab issue reports for `diag`: a non-empty path and a
/// 1-based line, always.
///
/// A diagnostic with a primary label reports that label's file and its real
/// line (resolved through the lookup, falling back to line 1 for a file the
/// lookup cannot supply). One without — or one whose label names no file at
/// all — takes its [`synthetic_path`] and line 1: the schema has no spelling
/// for "no line", and 1 is the honest floor for a whole-file finding.
fn gitlab_location(diag: &Diagnostic, sources: &mut Sources<'_>) -> (String, usize) {
    match diag.primary_label() {
        Some(label) if !label.span.file.is_empty() => {
            let begin = sources
                .get(&label.span.file)
                .map_or(1, |source| source.line_col(label.span.range.start).0);
            (label.span.file.clone(), begin)
        }
        _ => (synthetic_path(diag.code).to_owned(), 1),
    }
}

/// A stable FNV-1a fingerprint over a diagnostic's identity: code + reported
/// `path` + byte range + **message**. Deterministic across runs and Rust
/// versions.
///
/// The rendered line number is deliberately *not* an input. GitLab keys a
/// finding's history (first-seen, resolved) by its fingerprint, so anything
/// that changes when the rendering changes would resurrect every finding in
/// the project as brand new. The byte range already pins the diagnostic more
/// precisely than a line does, and it is the same value whatever the renderer
/// makes of it. `path` is the location's path, which is likewise
/// lookup-independent: a label's own file, or the code family's constant
/// [`synthetic_path`].
///
/// The **message** is an input because without it the fingerprint was not an
/// identity at all. Two *different* `LB0001`s over one byte range — a real
/// shape, the parser emits "unexpected token" and "expected an identifier"
/// about the same token — hashed the same, and GitLab keeps one issue per
/// fingerprint: the second finding vanished from the report. Every unspanned
/// diagnostic of a code was worse still, sharing a single hash.
///
/// The trade-off is deliberate and worth stating: **rewording a diagnostic
/// re-keys its findings**, so they show up in GitLab as resolved-and-new
/// rather than continuous. That is the correct half to lose — a reworded
/// message is a one-off event under the tool's own control, whereas a
/// collision silently drops a finding on every run.
fn fingerprint(diag: &Diagnostic, path: &str) -> String {
    let (start, end) = diag
        .primary_label()
        .map_or((0, 0), |l| (l.span.range.start, l.span.range.end));
    let key = format!("{}:{path}:{start}:{end}:{}", diag.code, diag.message);
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in key.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// GitLab Code Quality report: a JSON array of issues.
///
/// `location.lines.begin` is the **real** 1-based line of the diagnostic's
/// primary label, resolved through `lookup` exactly as [`render_sarif`]
/// resolves `startLine`. GitLab places a finding on the merge-request diff by
/// that line and drops it when the line is not part of the diff, so the
/// hardcoded `1` this used to emit pinned every finding to the top of the file
/// and made the format non-functional in a pipeline.
///
/// Two fallbacks survive, and neither can move a finding that *does* have a
/// resolvable line:
///
/// - a label whose file the lookup cannot supply keeps `begin: 1` — the path
///   is still named, and the schema requires a `begin`;
/// - a diagnostic with no label at all has no file either, and takes the
///   synthetic path its code family earns ([`synthetic_path`]) with
///   `begin: 1`. It used to report `path: ""` and `begin: 0`, which GitLab's
///   parser rejects — a report that parsed as JSON and annotated nothing.
///
/// The [`fingerprint`] is deliberately *not* line-derived (see its docs), so
/// this correction does not renumber anybody's existing GitLab findings.
#[must_use]
pub fn render_gitlab_code_quality(diags: &[Diagnostic], lookup: SourceLookup<'_>) -> String {
    let mut sources = Sources::new(lookup);
    let issues: Vec<_> = diags
        .iter()
        .map(|diag| {
            let (path, begin) = gitlab_location(diag, &mut sources);
            json!({
                "description": diag.message,
                "check_name": diag.code.to_string(),
                "fingerprint": fingerprint(diag, &path),
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

    // --- long-line windowing ----------------------------------------------

    /// Render one primary label over `source` at `range`, human format.
    fn render_span(source: &str, range: std::ops::Range<usize>) -> String {
        let code: Code = "LB0001".parse().unwrap();
        let diag = Diagnostic::error(code, "boom")
            .with_label(Label::primary(Span::new("main.lua", range), "here"));
        let owned = source.to_string();
        let lookup = move |file: &str| (file == "main.lua").then(|| owned.clone());
        render_human(std::slice::from_ref(&diag), &lookup)
    }

    #[test]
    fn a_line_at_the_window_width_is_printed_whole_and_unchanged() {
        // The byte-identity boundary: exactly HUMAN_LINE_WINDOW characters
        // is still a short line, so nothing about the output changes.
        let line = "x".repeat(HUMAN_LINE_WINDOW);
        let out = render_span(&line, 4..5);
        assert!(out.contains(&format!("1 | {line}\n")), "{out}");
        assert!(!out.contains(ELLIPSIS), "{out}");
        assert!(out.contains("--> main.lua:1:5"), "{out}");
        assert!(
            out.contains(&format!("  | {}^ here", " ".repeat(4))),
            "{out}"
        );
    }

    #[test]
    fn a_long_line_is_windowed_around_the_label_with_markers() {
        // One character past the window: now it is cut.
        let line = "abcdefghij".repeat(200); // 2000 chars
        let out = render_span(&line, 1000..1004);
        let rendered = out
            .lines()
            .find(|l| l.starts_with("1 | "))
            .expect("a source line");
        let shown = rendered.trim_start_matches("1 | ");
        assert!(shown.starts_with(ELLIPSIS), "{rendered}");
        assert!(shown.ends_with(ELLIPSIS), "{rendered}");
        assert_eq!(
            shown.chars().count(),
            HUMAN_LINE_WINDOW + 2 * ELLIPSIS.len(),
            "the window plus both markers, nothing more: {rendered}"
        );
        // The column NUMBER is never windowed.
        assert!(out.contains("--> main.lua:1:1001"), "{out}");
        // ... and the carets still sit under the span: MARGIN characters in,
        // past the leading marker.
        let caret_line = out.lines().find(|l| l.contains('^')).expect("a caret line");
        let indent = caret_line
            .trim_start_matches("  | ")
            .chars()
            .take_while(|&c| c == ' ')
            .count();
        assert_eq!(indent, HUMAN_LINE_MARGIN + ELLIPSIS.len(), "{caret_line}");
        assert!(caret_line.contains("^^^^ here"), "{caret_line}");
    }

    #[test]
    fn a_label_at_the_start_or_end_of_a_long_line_windows_one_sided() {
        let line = "abcdefghij".repeat(200);
        let head = render_span(&line, 0..1);
        let head_line = head.lines().find(|l| l.starts_with("1 | ")).unwrap();
        assert!(
            !head_line.trim_start_matches("1 | ").starts_with(ELLIPSIS),
            "nothing precedes the label, so no leading marker: {head_line}"
        );
        assert!(head_line.ends_with(ELLIPSIS), "{head_line}");

        let tail = render_span(&line, 1999..2000);
        let tail_line = tail.lines().find(|l| l.starts_with("1 | ")).unwrap();
        assert!(tail_line.trim_start_matches("1 | ").starts_with(ELLIPSIS));
        assert!(
            !tail_line.ends_with(ELLIPSIS),
            "the window reaches the end of the line: {tail_line}"
        );
    }

    #[test]
    fn a_span_running_past_the_window_has_its_carets_clipped() {
        let line = "y".repeat(5_000);
        let out = render_span(&line, 0..5_000);
        let caret_line = out.lines().find(|l| l.contains('^')).unwrap();
        let carets = caret_line.chars().filter(|&c| c == '^').count();
        assert!(
            carets <= HUMAN_LINE_WINDOW,
            "carets must not run past the window: {carets}"
        );
        assert!(carets > 0, "{caret_line}");
    }

    #[test]
    fn windowing_a_long_line_never_splits_a_character() {
        // Multi-byte characters either side of the label, so both window
        // edges land in the middle of one if the byte math is wrong.
        let line = "中".repeat(1_000);
        let out = render_span(&line, 1_500..1_503);
        assert!(out.contains("--> main.lua:1:501"), "{out}");
        let rendered = out.lines().find(|l| l.starts_with("1 | ")).unwrap();
        assert!(rendered.contains('中'), "{rendered}");
        assert_eq!(
            rendered.trim_start_matches("1 | ").chars().count(),
            HUMAN_LINE_WINDOW + 2 * ELLIPSIS.len()
        );
    }

    #[test]
    fn rendering_many_labels_on_one_huge_line_stays_small() {
        // The output bound the window exists for: before it, each label
        // printed the whole line plus a column-wide indent, so n labels on
        // an f-byte line cost O(n x f) of stdout — GBs in the wild.
        let code: Code = "LB0001".parse().unwrap();
        let line = "z".repeat(100_000);
        let diags: Vec<Diagnostic> = (0..500)
            .map(|i| {
                Diagnostic::error(code, "boom").with_label(Label::primary(
                    Span::new("main.lua", i * 100..i * 100 + 1),
                    "here",
                ))
            })
            .collect();
        let owned = line.clone();
        let lookup = move |file: &str| (file == "main.lua").then(|| owned.clone());
        let out = render_human(&diags, &lookup);
        assert!(
            out.len() < 500 * 4 * (HUMAN_LINE_WINDOW + 64),
            "rendered {} bytes for 500 labels on a 100 kB line",
            out.len()
        );
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
        // And the location carries the label's real line, not a placeholder:
        // the error sits on line 2 of `main.lua`, the warning on line 1 of
        // `luabox.toml` — the same lines SARIF reports for the same fixture.
        assert_eq!(value[0]["location"]["path"], "main.lua");
        assert_eq!(value[0]["location"]["lines"]["begin"], 2);
        assert_eq!(value[1]["location"]["path"], "luabox.toml");
        assert_eq!(value[1]["location"]["lines"]["begin"], 1);
    }

    /// GitLab hangs a finding off `location.lines.begin` to place it on the
    /// merge-request diff, so two diagnostics in the same file must report the
    /// two lines they are actually on. This is the regression: the renderer
    /// used to discard the lookup and emit `1` for every one of them.
    #[test]
    fn gitlab_lines_come_from_the_source_lookup_per_diagnostic() {
        let code: Code = "LB0001".parse().unwrap();
        // Line 1 starts at byte 0; line 3 (`print(x)`) starts at byte 22.
        let diags = vec![
            Diagnostic::error(code, "first")
                .with_label(Label::primary(Span::new("main.lua", 6..7), "on line 1")),
            Diagnostic::error(code, "third")
                .with_label(Label::primary(Span::new("main.lua", 28..29), "on line 3")),
        ];
        let out = render(&diags, Format::GitlabCodeQuality, &lookup);
        let value: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(value[0]["location"]["lines"]["begin"], 1);
        assert_eq!(value[1]["location"]["lines"]["begin"], 3);
        // Different lines, and therefore different findings on the diff.
        assert_ne!(
            value[0]["location"]["lines"]["begin"],
            value[1]["location"]["lines"]["begin"]
        );
    }

    /// The fingerprint is the finding's identity in GitLab's history, so it
    /// must not move when the *rendering* of a location does. It hashes the
    /// code, file, and byte range — never the line — which is why teaching the
    /// renderer real line numbers renumbers nobody's existing findings.
    #[test]
    fn gitlab_fingerprints_do_not_depend_on_the_rendered_line() {
        let with_source = render(&fixture(), Format::GitlabCodeQuality, &lookup);
        // The same diagnostics rendered without any source: the lines fall
        // back to 1, and the fingerprints must be untouched by that.
        let without_source = render(&fixture(), Format::GitlabCodeQuality, &|_| None);
        let a: serde_json::Value = serde_json::from_str(&with_source).unwrap();
        let b: serde_json::Value = serde_json::from_str(&without_source).unwrap();
        assert_eq!(a[0]["location"]["lines"]["begin"], 2);
        assert_eq!(b[0]["location"]["lines"]["begin"], 1);
        assert_eq!(a[0]["fingerprint"], b[0]["fingerprint"]);
        assert_eq!(a[1]["fingerprint"], b[1]["fingerprint"]);
    }

    // --- the GitLab report's own schema -----------------------------------

    /// The severities GitLab's Code Quality parser accepts.
    const GITLAB_SEVERITIES: [&str; 5] = ["info", "minor", "major", "critical", "blocker"];

    /// Parse a GitLab report and assert every issue satisfies the format's
    /// required-field contract — each present, of the right type, and **not
    /// empty**. GitLab rejects a report whose `location.path` is `""`, which
    /// is exactly the defect a `stdout is valid JSON` check cannot see: the
    /// document parses and annotates nothing.
    fn gitlab_report(out: &str) -> Vec<serde_json::Value> {
        let issues: Vec<serde_json::Value> =
            serde_json::from_str(out).unwrap_or_else(|e| panic!("not a JSON array: {e}\n{out}"));
        for issue in &issues {
            for field in ["description", "check_name", "fingerprint"] {
                let value = issue[field]
                    .as_str()
                    .unwrap_or_else(|| panic!("`{field}` is not a string in {issue}"));
                assert!(!value.is_empty(), "`{field}` is empty in {issue}");
            }
            let severity = issue["severity"].as_str().expect("severity is a string");
            assert!(
                GITLAB_SEVERITIES.contains(&severity),
                "`{severity}` is not a GitLab severity in {issue}"
            );
            let path = issue["location"]["path"]
                .as_str()
                .unwrap_or_else(|| panic!("`location.path` is not a string in {issue}"));
            assert!(!path.is_empty(), "`location.path` is empty in {issue}");
            let begin = issue["location"]["lines"]["begin"]
                .as_u64()
                .unwrap_or_else(|| panic!("`location.lines.begin` is not an integer in {issue}"));
            assert!(begin >= 1, "`location.lines.begin` is {begin} in {issue}");
        }
        issues
    }

    /// Every diagnostic shape the renderer can be handed — spanned, spanned
    /// at an unreadable file, unspanned manifest-level, unspanned other —
    /// must come out as a report GitLab will actually ingest.
    #[test]
    fn every_gitlab_issue_satisfies_the_code_quality_schema() {
        let mut diags = fixture();
        diags.push(Diagnostic::error(
            "LB1002".parse().unwrap(),
            "cannot resolve definition package `ghost` from `[types] defs`",
        ));
        diags.push(Diagnostic::warning(
            "LB0001".parse().unwrap(),
            "no location",
        ));
        diags.push(
            Diagnostic::error("LB0001".parse().unwrap(), "unreadable")
                .with_label(Label::primary(Span::new("unknown.lua", 4..5), "here")),
        );
        let issues = gitlab_report(&render(&diags, Format::GitlabCodeQuality, &lookup));
        assert_eq!(issues.len(), 5);
    }

    /// A project-level (`LB1xxx`) finding has no span, but it does have a
    /// file it is *about* — the manifest. Naming it keeps the finding on the
    /// merge-request diff whenever `luabox.toml` itself is part of it.
    #[test]
    fn an_unspanned_manifest_finding_is_attributed_to_the_manifest() {
        let diags = vec![
            Diagnostic::warning("LB1001".parse().unwrap(), "edition `6.0` is not recognised"),
            Diagnostic::error("LB1002".parse().unwrap(), "cannot resolve `ghost`"),
        ];
        let issues = gitlab_report(&render(&diags, Format::GitlabCodeQuality, &lookup));
        assert_eq!(issues[0]["location"]["path"], "luabox.toml");
        assert_eq!(issues[1]["location"]["path"], "luabox.toml");
    }

    /// Anything else genuinely fileless belongs to the project as a whole, so
    /// it reports the project root rather than a file it is not about.
    #[test]
    fn an_unspanned_non_manifest_finding_is_attributed_to_the_project_root() {
        let diag = Diagnostic::error("LB0001".parse().unwrap(), "no location for this one");
        let issues = gitlab_report(&render(&[diag], Format::GitlabCodeQuality, &lookup));
        assert_eq!(issues[0]["location"]["path"], PROJECT_ROOT_PATH);
    }

    /// The collision the fingerprint used to have: two *different* findings
    /// of the same code over the same byte range hashed identically, and
    /// GitLab keys findings by fingerprint — so it kept one and dropped the
    /// other. One diagnostic silently disappeared from the report.
    #[test]
    fn two_distinct_findings_at_one_range_keep_distinct_fingerprints() {
        let code: Code = "LB0001".parse().unwrap();
        let span = Span::new("main.lua", 18..19);
        let diags = vec![
            Diagnostic::error(code, "unexpected token `=`")
                .with_label(Label::primary(span.clone(), "a")),
            Diagnostic::error(code, "expected an identifier").with_label(Label::primary(span, "b")),
        ];
        let issues = gitlab_report(&render(&diags, Format::GitlabCodeQuality, &lookup));
        assert_ne!(
            issues[0]["fingerprint"], issues[1]["fingerprint"],
            "same code, same range, different findings — they must not collide"
        );
    }

    /// …and the same holds without a span at all: two distinct project-level
    /// findings both collapse onto the synthetic manifest path, so the
    /// message is the only thing left to tell them apart.
    #[test]
    fn two_distinct_unspanned_findings_keep_distinct_fingerprints() {
        let code: Code = "LB1004".parse().unwrap();
        let diags = vec![
            Diagnostic::warning(code, "unknown lint rule id `unused-locl` in `[lint]`"),
            Diagnostic::warning(code, "unknown lint rule id `globl-write` in `[lint]`"),
        ];
        let issues = gitlab_report(&render(&diags, Format::GitlabCodeQuality, &lookup));
        assert_ne!(issues[0]["fingerprint"], issues[1]["fingerprint"]);
    }

    /// A fingerprint is a finding's identity in GitLab's history (first-seen,
    /// resolved), so an unchanged finding must fingerprint identically on
    /// every run — including the unspanned shapes.
    #[test]
    fn gitlab_fingerprints_are_stable_across_identical_runs() {
        let mut diags = fixture();
        diags.push(Diagnostic::error(
            "LB1002".parse().unwrap(),
            "cannot resolve `ghost`",
        ));
        diags.push(Diagnostic::warning("LB0001".parse().unwrap(), "bare"));

        let first = gitlab_report(&render(&diags, Format::GitlabCodeQuality, &lookup));
        let second = gitlab_report(&render(&diags, Format::GitlabCodeQuality, &lookup));
        let prints = |issues: &[serde_json::Value]| -> Vec<String> {
            issues
                .iter()
                .map(|i| i["fingerprint"].as_str().unwrap().to_owned())
                .collect()
        };
        assert_eq!(prints(&first), prints(&second));
    }

    /// The documented trade-off: the message is *in* the fingerprint, so
    /// rewording a diagnostic re-keys its findings in GitLab. That is the
    /// intended behaviour — the alternative is two distinct findings sharing
    /// one identity, which loses one of them outright.
    #[test]
    fn rewording_a_message_intentionally_changes_the_fingerprint() {
        let code: Code = "LB0001".parse().unwrap();
        let at = |message: &str| {
            let diag = Diagnostic::error(code, message)
                .with_label(Label::primary(Span::new("main.lua", 18..19), "here"));
            let out = render(&[diag], Format::GitlabCodeQuality, &lookup);
            let value: serde_json::Value = serde_json::from_str(&out).unwrap();
            value[0]["fingerprint"].as_str().unwrap().to_owned()
        };
        assert_ne!(at("unexpected token `=`"), at("unexpected token '='"));
    }

    /// The sweep the GitLab fix prompted: does any *other* machine format
    /// emit an empty location for an unspanned finding? None does — and each
    /// for a reason that is a property of the format, not an accident, so
    /// pin the reasons rather than trusting the reading.
    ///
    /// - **JSON** is the `Diagnostic` structure verbatim: an unspanned
    ///   diagnostic has an empty `labels` array and no location field to be
    ///   wrong about.
    /// - **SARIF** omits `result.locations` entirely. The property is
    ///   optional in SARIF 2.1.0, and absent is the specified way to say a
    ///   result has no location — an empty `artifactLocation.uri` would be
    ///   the malformed spelling, and is never emitted.
    /// - **GitHub Actions** drops the `file=` property, leaving `::error::`,
    ///   which annotates the workflow step rather than a file. The format
    ///   tolerates it by design.
    ///
    /// Fingerprints are GitLab's alone — no other renderer emits one — so the
    /// collision defect had nowhere else to live either.
    #[test]
    fn no_other_machine_format_emits_an_empty_location_for_an_unspanned_finding() {
        let bare = Diagnostic::error("LB1002".parse().unwrap(), "cannot resolve `ghost`");

        let json: serde_json::Value =
            serde_json::from_str(&render(std::slice::from_ref(&bare), Format::Json, &lookup))
                .unwrap();
        assert_eq!(json[0]["labels"].as_array().unwrap().len(), 0);
        assert!(json[0].get("location").is_none());

        let sarif: serde_json::Value =
            serde_json::from_str(&render(std::slice::from_ref(&bare), Format::Sarif, &lookup))
                .unwrap();
        let result = &sarif["runs"][0]["results"][0];
        assert_eq!(result["ruleId"], "LB1002");
        assert!(
            result.get("locations").is_none(),
            "SARIF says absent, never an empty uri: {result}"
        );

        let gha = render(std::slice::from_ref(&bare), Format::GithubActions, &lookup);
        assert_eq!(gha, "::error::LB1002: cannot resolve `ghost`\n");
        assert!(!gha.contains("file="), "no empty file property: {gha}");
    }

    /// A file the lookup cannot supply still gets a usable issue: the path is
    /// named and `begin` falls back to 1 rather than vanishing (the schema
    /// requires it).
    #[test]
    fn a_label_whose_file_has_no_source_falls_back_to_line_one_in_gitlab() {
        let code: Code = "LB0001".parse().unwrap();
        let diag = Diagnostic::error(code, "boom")
            .with_label(Label::primary(Span::new("unknown.lua", 400..403), "here"));
        let out = render(&[diag], Format::GitlabCodeQuality, &lookup);
        let value: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(value[0]["location"]["path"], "unknown.lua");
        assert_eq!(value[0]["location"]["lines"]["begin"], 1);
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

    #[test]
    fn a_label_with_no_message_renders_a_bare_underline() {
        let code: Code = "LB0001".parse().unwrap();
        // With source: the caret row carries no trailing message.
        let with_source = Diagnostic::error(code, "boom")
            .with_label(Label::primary(Span::new("main.lua", 18..19), ""));
        let out = render(&[with_source], Format::Human, &lookup);
        assert!(out.contains(" --> main.lua:2:7"), "{out}");
        assert!(out.contains("|       ^\n"), "{out}");
        assert!(!out.contains("^ "), "no trailing message: {out}");

        // Without source: only the location line, no message line under it.
        let no_source = Diagnostic::error(code, "boom")
            .with_label(Label::primary(Span::new("nowhere.lua", 0..3), ""));
        let out = render(&[no_source], Format::Human, &lookup);
        assert_eq!(out, "error[LB0001]: boom\n --> nowhere.lua (bytes 0..3)\n");
    }

    #[test]
    fn a_diagnostic_with_no_labels_renders_in_every_format() {
        let code: Code = "LB0001".parse().unwrap();
        let bare = Diagnostic::warning(code, "no location for this one");

        let human = render(std::slice::from_ref(&bare), Format::Human, &lookup);
        assert_eq!(human, "warning[LB0001]: no location for this one\n");

        // GitHub Actions: no `file=`/`line=` properties, so no space either.
        let gha = render(std::slice::from_ref(&bare), Format::GithubActions, &lookup);
        assert_eq!(gha, "::warning::LB0001: no location for this one\n");

        // GitLab: a *synthetic* path, because the format has no way to say
        // "nowhere" — an empty `location.path` is rejected outright. `LB0001`
        // is not a manifest code, so the finding belongs to the project as a
        // whole. Plus a stable fingerprint over the label-less identity.
        let gitlab = render(
            std::slice::from_ref(&bare),
            Format::GitlabCodeQuality,
            &lookup,
        );
        let value: serde_json::Value = serde_json::from_str(&gitlab).unwrap();
        assert_eq!(value[0]["location"]["path"], PROJECT_ROOT_PATH);
        assert_eq!(value[0]["location"]["lines"]["begin"], 1);
        let fingerprint = value[0]["fingerprint"].as_str().unwrap().to_owned();
        assert_eq!(fingerprint.len(), 16);
        let again = render(&[bare], Format::GitlabCodeQuality, &lookup);
        let value2: serde_json::Value = serde_json::from_str(&again).unwrap();
        assert_eq!(value2[0]["fingerprint"], fingerprint);
    }

    #[test]
    fn a_label_whose_file_has_no_source_falls_back_in_github_actions() {
        let code: Code = "LB0001".parse().unwrap();
        // `unknown.lua` is not in the lookup table: the file property is still
        // emitted, but there is no line/col to compute.
        let diag = Diagnostic::error(code, "boom")
            .with_label(Label::primary(Span::new("unknown.lua", 0..3), "here"));
        let out = render(&[diag], Format::GithubActions, &lookup);
        assert_eq!(out, "::error file=unknown.lua::LB0001: boom\n");
    }

    #[test]
    fn sarif_lists_each_rule_once_however_often_its_code_repeats() {
        let code: Code = "LB0001".parse().unwrap();
        let diags = vec![
            Diagnostic::error(code, "first")
                .with_label(Label::primary(Span::new("main.lua", 18..19), "a")),
            Diagnostic::error(code, "second")
                .with_label(Label::primary(Span::new("main.lua", 12..17), "b")),
        ];
        let out = render(&diags, Format::Sarif, &lookup);
        let value: serde_json::Value = serde_json::from_str(&out).unwrap();
        let rules = value["runs"][0]["tool"]["driver"]["rules"]
            .as_array()
            .unwrap();
        assert_eq!(rules.len(), 1, "one rule for two same-code results");
        assert_eq!(rules[0]["id"], "LB0001");
        assert_eq!(value["runs"][0]["results"].as_array().unwrap().len(), 2);
    }

    /// The lookup hands back an *owned* `String` and, in the CLI, reads it
    /// off disk — so calling it once per label cloned the whole file per
    /// diagnostic. Every renderer that resolves locations must ask for each
    /// distinct file exactly once per render, however many diagnostics and
    /// labels point into it. This is the cost the fix removes; assert it
    /// structurally so it cannot creep back.
    #[test]
    fn every_renderer_fetches_each_file_once_per_render() {
        use std::cell::RefCell;

        let code: Code = "LB0001".parse().unwrap();
        // Twelve diagnostics across two files, two labels each, plus one
        // label naming a file the lookup cannot supply (a miss must be
        // cached too, or it is re-asked once per diagnostic).
        let diags: Vec<Diagnostic> = (0..12)
            .map(|i| {
                Diagnostic::error(code, "boom")
                    .with_label(Label::primary(Span::new("main.lua", 18..19), "here"))
                    .with_label(Label::secondary(Span::new("luabox.toml", 0..3), "and here"))
                    .with_label(Label::secondary(Span::new("gone.lua", i..i + 1), "nowhere"))
            })
            .collect();

        for format in [
            Format::Human,
            Format::Sarif,
            Format::GithubActions,
            Format::GitlabCodeQuality,
        ] {
            let asked: RefCell<Vec<String>> = RefCell::new(Vec::new());
            let counting = |file: &str| {
                asked.borrow_mut().push(file.to_string());
                lookup(file)
            };
            let _ = render(&diags, format, &counting);
            let calls = asked.into_inner();
            let mut distinct = calls.clone();
            distinct.sort_unstable();
            distinct.dedup();
            assert_eq!(
                calls.len(),
                distinct.len(),
                "{format:?} asked for {} sources but only {} distinct files are \
                 involved — the lookup is being re-run per diagnostic: {calls:?}",
                calls.len(),
                distinct.len()
            );
            assert!(
                distinct.len() <= 3,
                "{format:?} touched more files than exist: {distinct:?}"
            );
        }
    }

    #[test]
    fn github_command_messages_escape_reserved_characters() {
        let code: Code = "LB0001".parse().unwrap();
        let diag = Diagnostic::error(code, "100% broken\nsecond line\rthird");
        let out = render(&[diag], Format::GithubActions, &lookup);
        assert_eq!(
            out,
            "::error::LB0001: 100%25 broken%0Asecond line%0Dthird\n"
        );
    }
}

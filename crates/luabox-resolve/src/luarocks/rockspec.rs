//! Static rockspec reading (SPEC.md §6).
//!
//! A rockspec *is a Lua file*: a sequence of assignments to well-known
//! globals (`package`, `version`, `source`, `dependencies`, `build`). Rather
//! than run it, luabox parses it with the same lossless Lua parser the rest
//! of the toolchain uses ([`luabox_syntax`]) and evaluates a **bounded,
//! side-effect-free subset** of Lua statically — enough to read the fields a
//! resolver needs, including the computed forms real rockspecs use.
//!
//! # What the evaluator supports
//!
//! Top-level `local x = …` and `x = …` assignments, evaluated top to bottom
//! into a scope, then the well-known globals are read out. Expressions it can
//! fold: string / number / boolean / `nil` literals, name references,
//! `..` concatenation, `==`/`~=`/`<`/`>`/`<=`/`>=` comparisons, `and`/`or`
//! (Lua short-circuit semantics), `not`, table constructors, indexing into
//! folded tables, and `("%s"):format(…)` / `string.format(…)`. This covers
//! the overwhelmingly common patterns (e.g. lunarmodules-style rockspecs that
//! build `source.url` from a namespace and repo via `:format`).
//!
//! Anything outside the subset folds to [`Value::Unknown`]; a field that a
//! caller actually needs but that folded to `Unknown` is a hard error naming
//! the field, never a silent wrong answer.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use luabox_syntax::lua::ast::{Expr, SourceFile, Stmt};
use luabox_syntax::lua::{Dialect, SyntaxKind, parse};

/// A statically read rockspec — only the fields the resolver needs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Rockspec {
    pub package: Option<String>,
    pub version: Option<String>,
    pub source: Source,
    /// Raw LuaRocks dependency strings (`"lpeg >= 1.0"`), in order.
    pub dependencies: Vec<String>,
    pub build: Build,
}

/// The rockspec `source` table.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Source {
    pub url: Option<String>,
    pub tag: Option<String>,
    pub branch: Option<String>,
    pub dir: Option<String>,
}

/// The rockspec `build` table (the parts that decide pure-Lua vs C).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Build {
    /// `builtin`, `none`, `make`, `cmake`, `command`, … (absent → `builtin`
    /// by LuaRocks default).
    pub build_type: Option<String>,
    /// `build.modules`: module name → the value's *shape*. Pure-Lua modules
    /// map to a single `.lua` path; anything else signals a C module.
    pub modules: BTreeMap<String, ModuleSpec>,
    /// Whether an `external_dependencies` table was present (a strong C
    /// signal — it names system libraries to link against).
    pub has_external_dependencies: bool,
}

/// The shape of one `build.modules` entry, enough to classify the rock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleSpec {
    /// A single Lua file path (`foo = "src/foo.lua"`).
    LuaFile(String),
    /// A single non-Lua file path (`.c`, `.so`, …) — a C module.
    NativeFile(String),
    /// A table value (C sources list, `{ sources = {…} }`) — a C module.
    Native,
    /// The value could not be folded statically.
    Unknown,
}

/// Parses rockspec text, reading the fields the resolver needs.
///
/// The Lua parser is error-resilient, so this never fails on syntax; fields
/// it cannot fold are simply left `None`/empty and diagnosed later by the
/// caller when actually required.
#[must_use]
pub fn read(text: &str) -> Rockspec {
    let parsed = parse(text, Dialect::Lua54);
    let file = parsed.tree();
    let scope = evaluate_scope(&file);

    let package = scope.get("package").and_then(Value::as_string);
    let version = scope.get("version").and_then(Value::as_string);
    let source = scope
        .get("source")
        .and_then(Value::as_table)
        .map(read_source)
        .unwrap_or_default();
    let dependencies = scope
        .get("dependencies")
        .and_then(Value::as_table)
        .map(|t| {
            t.array
                .iter()
                .filter_map(Value::as_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut build = scope
        .get("build")
        .and_then(Value::as_table)
        .map(read_build)
        .unwrap_or_default();
    // `external_dependencies` is a top-level rockspec global (a strong C
    // signal — it names system libraries to link), not a `build` field.
    if matches!(scope.get("external_dependencies"), Some(v) if !v.is_nil()) {
        build.has_external_dependencies = true;
    }

    Rockspec {
        package,
        version,
        source,
        dependencies,
        build,
    }
}

fn read_source(table: &Table) -> Source {
    Source {
        url: table.get("url").and_then(Value::as_string),
        tag: table.get("tag").and_then(Value::as_string),
        branch: table.get("branch").and_then(Value::as_string),
        dir: table.get("dir").and_then(Value::as_string),
    }
}

fn read_build(table: &Table) -> Build {
    let build_type = table.get("type").and_then(Value::as_string);
    let mut modules = BTreeMap::new();
    if let Some(Value::Table(mods)) = table.get("modules") {
        for (name, value) in &mods.map {
            modules.insert(name.clone(), classify_module(value));
        }
    }
    Build {
        build_type,
        modules,
        // `external_dependencies` is read from the top-level scope (see
        // `read`), not from inside `build`.
        has_external_dependencies: false,
    }
}

fn classify_module(value: &Value) -> ModuleSpec {
    match value {
        Value::Str(path) => {
            let is_lua = std::path::Path::new(path)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("lua"));
            if is_lua {
                ModuleSpec::LuaFile(path.clone())
            } else {
                ModuleSpec::NativeFile(path.clone())
            }
        }
        Value::Table(_) => ModuleSpec::Native,
        _ => ModuleSpec::Unknown,
    }
}

// ---------------------------------------------------------------------
// The bounded evaluator
// ---------------------------------------------------------------------

/// A statically folded Lua value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Num(f64),
    Bool(bool),
    Nil,
    Table(Table),
    /// Could not be folded statically.
    Unknown,
}

/// A folded table: an array part (positional items) and a string-keyed map.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Table {
    pub array: Vec<Value>,
    pub map: BTreeMap<String, Value>,
}

impl Table {
    fn get(&self, key: &str) -> Option<&Value> {
        self.map.get(key)
    }
}

impl Value {
    fn as_string(&self) -> Option<String> {
        match self {
            Self::Str(s) => Some(s.clone()),
            _ => None,
        }
    }
    fn as_table(&self) -> Option<&Table> {
        match self {
            Self::Table(t) => Some(t),
            _ => None,
        }
    }
    fn is_nil(&self) -> bool {
        matches!(self, Self::Nil)
    }
    /// Lua truthiness: everything but `nil` and `false` is truthy. `Unknown`
    /// has no definite truth value.
    fn truthy(&self) -> Option<bool> {
        match self {
            Self::Nil => Some(false),
            Self::Bool(b) => Some(*b),
            Self::Unknown => None,
            _ => Some(true),
        }
    }
    /// String/number coercion for `..`.
    fn concat_repr(&self) -> Option<String> {
        match self {
            Self::Str(s) => Some(s.clone()),
            Self::Num(n) => Some(format_lua_number(*n)),
            _ => None,
        }
    }
}

type Scope = BTreeMap<String, Value>;

/// Walks the top-level statements in order, folding `local x = …` and
/// `x = …` assignments into a scope.
fn evaluate_scope(file: &SourceFile) -> Scope {
    let mut scope = Scope::new();
    let Some(block) = file.block() else {
        return scope;
    };
    for stmt in block.stmts() {
        match stmt {
            Stmt::Local(local) => {
                let names: Vec<_> = local
                    .names()
                    .filter_map(|n| n.name().map(|t| t.text().to_owned()))
                    .collect();
                let values: Vec<Value> = local
                    .values()
                    .map(|list| list.exprs().map(|e| eval(&e, &scope)).collect())
                    .unwrap_or_default();
                for (i, name) in names.into_iter().enumerate() {
                    scope.insert(name, values.get(i).cloned().unwrap_or(Value::Nil));
                }
            }
            Stmt::Assign(assign) => {
                let targets: Vec<String> = assign
                    .targets()
                    .map(|list| list.exprs().map(|e| target_name(&e)).collect::<Vec<_>>())
                    .unwrap_or_default()
                    .into_iter()
                    .flatten()
                    .collect();
                let values: Vec<Value> = assign
                    .values()
                    .map(|list| list.exprs().map(|e| eval(&e, &scope)).collect())
                    .unwrap_or_default();
                // Only simple single-name targets (`x = …`) are tracked; a
                // dotted target (`source.url = …`) is ignored (rare at top
                // level, and folding it would need mutable table aliasing).
                if let [name] = &targets[..] {
                    scope.insert(
                        name.clone(),
                        values.into_iter().next().unwrap_or(Value::Nil),
                    );
                }
            }
            _ => {}
        }
    }
    scope
}

/// The bare name of a simple assignment target, if it is one.
fn target_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Name(n) => n.name().map(|t| t.text().to_owned()),
        _ => None,
    }
}

/// Folds one expression against the current scope.
fn eval(expr: &Expr, scope: &Scope) -> Value {
    match expr {
        Expr::Literal(lit) => eval_literal(lit),
        Expr::Name(name) => name
            .name()
            .and_then(|t| scope.get(t.text()).cloned())
            .unwrap_or(Value::Unknown),
        Expr::Paren(paren) => paren.inner().map_or(Value::Unknown, |e| eval(&e, scope)),
        Expr::Bin(bin) => eval_bin(bin, scope),
        Expr::Prefix(prefix) => eval_prefix(prefix, scope),
        Expr::Table(table) => eval_table(table, scope),
        Expr::MethodCall(call) => eval_method_call(call, scope),
        Expr::Call(call) => eval_call(call, scope),
        Expr::Field(field) => {
            let base = field.base().map_or(Value::Unknown, |e| eval(&e, scope));
            match (base, field.field_name()) {
                (Value::Table(t), Some(name)) => t.get(name.text()).cloned().unwrap_or(Value::Nil),
                _ => Value::Unknown,
            }
        }
        Expr::Index(index) => {
            let base = index.base().map_or(Value::Unknown, |e| eval(&e, scope));
            let key = index.index().map_or(Value::Unknown, |e| eval(&e, scope));
            match (base, key) {
                (Value::Table(t), Value::Str(k)) => t.get(&k).cloned().unwrap_or(Value::Nil),
                _ => Value::Unknown,
            }
        }
        _ => Value::Unknown,
    }
}

fn eval_literal(lit: &luabox_syntax::lua::ast::LiteralExpr) -> Value {
    let Some(token) = lit.token() else {
        return Value::Unknown;
    };
    match token.kind() {
        SyntaxKind::STRING => decode_string(token.text()).map_or(Value::Unknown, Value::Str),
        SyntaxKind::NUMBER => decode_number(token.text()).map_or(Value::Unknown, Value::Num),
        SyntaxKind::NIL_KW => Value::Nil,
        SyntaxKind::TRUE_KW => Value::Bool(true),
        SyntaxKind::FALSE_KW => Value::Bool(false),
        _ => Value::Unknown,
    }
}

fn eval_bin(bin: &luabox_syntax::lua::ast::BinExpr, scope: &Scope) -> Value {
    let Some(op) = bin.op_token() else {
        return Value::Unknown;
    };
    let op = op.kind();

    // Short-circuit operators evaluate the left side first.
    if matches!(op, SyntaxKind::AND_KW | SyntaxKind::OR_KW) {
        let lhs = bin.lhs().map_or(Value::Unknown, |e| eval(&e, scope));
        let Some(truthy) = lhs.truthy() else {
            return Value::Unknown;
        };
        let take_rhs = match op {
            SyntaxKind::AND_KW => truthy, // a and b: b iff a truthy
            SyntaxKind::OR_KW => !truthy, // a or b: b iff a falsy
            _ => unreachable!(),
        };
        return if take_rhs {
            bin.rhs().map_or(Value::Unknown, |e| eval(&e, scope))
        } else {
            lhs
        };
    }

    let lhs = bin.lhs().map_or(Value::Unknown, |e| eval(&e, scope));
    let rhs = bin.rhs().map_or(Value::Unknown, |e| eval(&e, scope));
    match op {
        SyntaxKind::DOT_DOT => match (lhs.concat_repr(), rhs.concat_repr()) {
            (Some(a), Some(b)) => Value::Str(format!("{a}{b}")),
            _ => Value::Unknown,
        },
        SyntaxKind::EQ_EQ => bin_eq(&lhs, &rhs).map_or(Value::Unknown, Value::Bool),
        SyntaxKind::TILDE_EQ => bin_eq(&lhs, &rhs).map_or(Value::Unknown, |e| Value::Bool(!e)),
        SyntaxKind::LT | SyntaxKind::GT | SyntaxKind::LT_EQ | SyntaxKind::GT_EQ => {
            eval_ordering(op, &lhs, &rhs)
        }
        SyntaxKind::PLUS | SyntaxKind::MINUS | SyntaxKind::STAR | SyntaxKind::SLASH => {
            eval_arith(op, &lhs, &rhs)
        }
        _ => Value::Unknown,
    }
}

/// Structural equality of two folded values, or `None` if either is unknown.
fn bin_eq(lhs: &Value, rhs: &Value) -> Option<bool> {
    if matches!(lhs, Value::Unknown) || matches!(rhs, Value::Unknown) {
        return None;
    }
    Some(lhs == rhs)
}

fn eval_ordering(op: SyntaxKind, lhs: &Value, rhs: &Value) -> Value {
    let result = match (lhs, rhs) {
        (Value::Num(a), Value::Num(b)) => order(op, a.partial_cmp(b)),
        (Value::Str(a), Value::Str(b)) => order(op, Some(a.cmp(b))),
        _ => return Value::Unknown,
    };
    result.map_or(Value::Unknown, Value::Bool)
}

fn order(op: SyntaxKind, ordering: Option<std::cmp::Ordering>) -> Option<bool> {
    use std::cmp::Ordering::{Equal, Greater, Less};
    let ord = ordering?;
    Some(match op {
        SyntaxKind::LT => ord == Less,
        SyntaxKind::GT => ord == Greater,
        SyntaxKind::LT_EQ => matches!(ord, Less | Equal),
        SyntaxKind::GT_EQ => matches!(ord, Greater | Equal),
        _ => return None,
    })
}

fn eval_arith(op: SyntaxKind, lhs: &Value, rhs: &Value) -> Value {
    let (Value::Num(a), Value::Num(b)) = (lhs, rhs) else {
        return Value::Unknown;
    };
    let result = match op {
        SyntaxKind::PLUS => a + b,
        SyntaxKind::MINUS => a - b,
        SyntaxKind::STAR => a * b,
        SyntaxKind::SLASH => a / b,
        _ => return Value::Unknown,
    };
    Value::Num(result)
}

fn eval_prefix(prefix: &luabox_syntax::lua::ast::PrefixExpr, scope: &Scope) -> Value {
    let Some(op) = prefix.op_token() else {
        return Value::Unknown;
    };
    let operand = prefix.operand().map_or(Value::Unknown, |e| eval(&e, scope));
    match op.kind() {
        SyntaxKind::NOT_KW => operand.truthy().map_or(Value::Unknown, |t| Value::Bool(!t)),
        SyntaxKind::MINUS => match operand {
            Value::Num(n) => Value::Num(-n),
            _ => Value::Unknown,
        },
        _ => Value::Unknown,
    }
}

fn eval_table(table: &luabox_syntax::lua::ast::TableExpr, scope: &Scope) -> Value {
    use luabox_syntax::lua::ast::TableField;
    let mut out = Table::default();
    for field in table.fields() {
        match field {
            TableField::Item(item) => {
                let value = item.value().map_or(Value::Unknown, |e| eval(&e, scope));
                out.array.push(value);
            }
            TableField::Name(named) => {
                if let Some(name) = named.name() {
                    let value = named.value().map_or(Value::Unknown, |e| eval(&e, scope));
                    out.map.insert(name.text().to_owned(), value);
                }
            }
            TableField::Key(keyed) => {
                let key = keyed.key().map_or(Value::Unknown, |e| eval(&e, scope));
                let value = keyed.value().map_or(Value::Unknown, |e| eval(&e, scope));
                if let Value::Str(k) = key {
                    out.map.insert(k, value);
                }
            }
        }
    }
    Value::Table(out)
}

/// Folds `receiver:format(args…)` (the only method call the subset needs).
fn eval_method_call(call: &luabox_syntax::lua::ast::MethodCallExpr, scope: &Scope) -> Value {
    let Some(method) = call.method_name() else {
        return Value::Unknown;
    };
    if method.text() != "format" {
        return Value::Unknown;
    }
    let receiver = call.receiver().map_or(Value::Unknown, |e| eval(&e, scope));
    let Value::Str(fmt) = receiver else {
        return Value::Unknown;
    };
    let args = call_args(call.args().as_ref(), scope);
    lua_format(&fmt, &args)
}

/// Folds `string.format(fmt, args…)`.
fn eval_call(call: &luabox_syntax::lua::ast::CallExpr, scope: &Scope) -> Value {
    // Only `string.format(...)` is recognized.
    let Some(Expr::Field(field)) = call.callee() else {
        return Value::Unknown;
    };
    let is_string_format = matches!(field.base(), Some(Expr::Name(base)) if base.name().is_some_and(|t| t.text() == "string"))
        && field.field_name().is_some_and(|t| t.text() == "format");
    if !is_string_format {
        return Value::Unknown;
    }
    let args = call_args(call.args().as_ref(), scope);
    let Some(Value::Str(fmt)) = args.first().cloned() else {
        return Value::Unknown;
    };
    lua_format(&fmt, &args[1..])
}

fn call_args(args: Option<&luabox_syntax::lua::ast::ArgList>, scope: &Scope) -> Vec<Value> {
    let Some(args) = args else {
        return Vec::new();
    };
    if let Some(list) = args.expr_list() {
        return list.exprs().map(|e| eval(&e, scope)).collect();
    }
    if let Some(string) = args.string_arg() {
        return decode_string(string.text())
            .map(|s| vec![Value::Str(s)])
            .unwrap_or_default();
    }
    Vec::new()
}

/// A minimal `string.format` supporting the specifiers rockspecs use:
/// `%s`, `%d`/`%i`, `%x`, and `%%`. Returns `Unknown` when a needed argument
/// is not a folded string/number.
fn lua_format(fmt: &str, args: &[Value]) -> Value {
    let mut out = String::new();
    let mut arg_index = 0usize;
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let Some(&spec) = chars.peek() else {
            return Value::Unknown;
        };
        chars.next();
        match spec {
            '%' => out.push('%'),
            's' => {
                let Some(arg) = args.get(arg_index).and_then(Value::concat_repr) else {
                    return Value::Unknown;
                };
                out.push_str(&arg);
                arg_index += 1;
            }
            'd' | 'i' => {
                let Some(Value::Num(n)) = args.get(arg_index) else {
                    return Value::Unknown;
                };
                #[allow(clippy::cast_possible_truncation)]
                let _ = write!(out, "{}", *n as i64);
                arg_index += 1;
            }
            'x' => {
                let Some(Value::Num(n)) = args.get(arg_index) else {
                    return Value::Unknown;
                };
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let _ = write!(out, "{:x}", *n as i64 as u64);
                arg_index += 1;
            }
            _ => return Value::Unknown,
        }
    }
    Value::Str(out)
}

// ---------------------------------------------------------------------
// Literal decoding
// ---------------------------------------------------------------------

/// Decodes a Lua string literal token's text (quotes and escapes, or long
/// brackets) into its value.
fn decode_string(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let first = *bytes.first()?;
    if first == b'[' {
        return decode_long_bracket(text);
    }
    if first != b'"' && first != b'\'' {
        return None;
    }
    // A well-formed literal is `"..."`/`'...'` (both quotes ASCII, so the
    // byte indices land on char boundaries). `get` rather than direct slicing
    // keeps a malformed single-quote token (e.g. an unterminated string from
    // adversarial rockspec input) from panicking — it decodes to `None`.
    let inner = text.get(1..text.len().checked_sub(1)?)?;
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            // `\n` escape and `\<newline>` line-continuation both yield a
            // newline.
            Some('n' | '\n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('a') => out.push('\u{7}'),
            Some('b') => out.push('\u{8}'),
            Some('f') => out.push('\u{c}'),
            Some('v') => out.push('\u{b}'),
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some('\'') => out.push('\''),
            // Numeric/hex escapes are uncommon in rockspec fields; pass the
            // escaped char through rather than fail the whole parse.
            Some(other) => out.push(other),
            None => break,
        }
    }
    Some(out)
}

/// Decodes a `[[ … ]]` / `[=*[ … ]=*]` long-bracket string.
fn decode_long_bracket(text: &str) -> Option<String> {
    let rest = text.strip_prefix('[')?;
    let eqs = rest.chars().take_while(|&c| c == '=').count();
    let open = format!("[{}[", "=".repeat(eqs));
    let close = format!("]{}]", "=".repeat(eqs));
    let body = text.strip_prefix(&open)?.strip_suffix(&close)?;
    // Lua drops a single leading newline right after the opening bracket.
    Some(body.strip_prefix('\n').unwrap_or(body).to_owned())
}

/// Decodes a Lua number literal (decimal or `0x…` hex integer).
fn decode_number(text: &str) -> Option<f64> {
    let text = text.trim();
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        #[allow(clippy::cast_precision_loss)]
        return i64::from_str_radix(hex, 16).ok().map(|n| n as f64);
    }
    text.parse::<f64>().ok()
}

/// Renders a folded number the way Lua's `tostring`/`..` would for the
/// integer-valued cases rockspecs actually concatenate.
fn format_lua_number(n: f64) -> String {
    if n.fract() == 0.0 && n.is_finite() {
        #[allow(clippy::cast_possible_truncation)]
        return format!("{}", n as i64);
    }
    format!("{n}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_fully_static_rockspec() {
        let text = r#"
package = "inspect"
version = "3.1.3-0"
source = {
   url = "https://github.com/kikito/inspect.lua/archive/v3.1.3.tar.gz",
   dir = "inspect.lua-3.1.3",
}
dependencies = { "lua >= 5.1" }
build = {
   type = "builtin",
   modules = { inspect = "inspect.lua" },
}
"#;
        let spec = read(text);
        assert_eq!(spec.package.as_deref(), Some("inspect"));
        assert_eq!(spec.version.as_deref(), Some("3.1.3-0"));
        assert_eq!(
            spec.source.url.as_deref(),
            Some("https://github.com/kikito/inspect.lua/archive/v3.1.3.tar.gz")
        );
        assert_eq!(spec.source.dir.as_deref(), Some("inspect.lua-3.1.3"));
        assert_eq!(spec.dependencies, vec!["lua >= 5.1".to_owned()]);
        assert_eq!(spec.build.build_type.as_deref(), Some("builtin"));
        assert_eq!(
            spec.build.modules.get("inspect"),
            Some(&ModuleSpec::LuaFile("inspect.lua".to_owned()))
        );
    }

    #[test]
    fn folds_computed_source_url_like_say() {
        // Mirrors the real `say` rockspec: locals, `:format`, `..`, and the
        // `cond and x or nil` idiom for tag/branch.
        let text = r#"
package = "say"
local rock_version = "1.4.1"
local rock_release = "3"
local namespace = "lunarmodules"
local repository = package
version = ("%s-%s"):format(rock_version, rock_release)
source = {
  url = ("git+https://github.com/%s/%s.git"):format(namespace, repository),
  branch = rock_version == "scm" and "master" or nil,
  tag = rock_version ~= "scm" and "v"..rock_version or nil,
}
dependencies = { "lua >= 5.1" }
build = {
  type = "builtin",
  modules = { say = "src/say/init.lua" },
}
"#;
        let spec = read(text);
        assert_eq!(spec.version.as_deref(), Some("1.4.1-3"));
        assert_eq!(
            spec.source.url.as_deref(),
            Some("git+https://github.com/lunarmodules/say.git")
        );
        assert_eq!(spec.source.tag.as_deref(), Some("v1.4.1"));
        assert_eq!(spec.source.branch, None, "nil branch folds away");
        assert_eq!(
            spec.build.modules.get("say"),
            Some(&ModuleSpec::LuaFile("src/say/init.lua".to_owned()))
        );
    }

    #[test]
    fn detects_c_module_shapes() {
        let text = r#"
package = "lpeg"
version = "1.0.2-1"
source = { url = "http://www.example.com/lpeg-1.0.2.tar.gz" }
build = {
  type = "builtin",
  modules = {
    lpeg = { "lpcap.c", "lpcode.c" },
    re = "re.lua",
  },
}
"#;
        let spec = read(text);
        assert_eq!(spec.build.modules.get("lpeg"), Some(&ModuleSpec::Native));
        assert_eq!(
            spec.build.modules.get("re"),
            Some(&ModuleSpec::LuaFile("re.lua".to_owned()))
        );
    }

    #[test]
    fn detects_native_file_and_external_deps() {
        let text = r#"
package = "socket"
version = "3.0-1"
source = { url = "git+https://example.com/socket.git" }
external_dependencies = { OPENSSL = { header = "openssl/ssl.h" } }
build = {
  type = "make",
  modules = { ["socket.core"] = "src/luasocket.c" },
}
"#;
        let spec = read(text);
        assert!(spec.build.has_external_dependencies);
        assert_eq!(spec.build.build_type.as_deref(), Some("make"));
        assert_eq!(
            spec.build.modules.get("socket.core"),
            Some(&ModuleSpec::NativeFile("src/luasocket.c".to_owned()))
        );
    }

    #[test]
    fn long_bracket_strings_decode() {
        assert_eq!(decode_string("[[hello]]").as_deref(), Some("hello"));
        assert_eq!(decode_string("[==[a]b]==]").as_deref(), Some("a]b"));
        assert_eq!(decode_string("[[\nline]]").as_deref(), Some("line"));
        assert_eq!(decode_string(r#""a\tb""#).as_deref(), Some("a\tb"));
        assert_eq!(decode_string("'it'").as_deref(), Some("it"));
    }

    #[test]
    fn unresolvable_fields_stay_none() {
        // A source url computed from an unknown function folds to Unknown and
        // is reported as absent (the provider errors when it needs it).
        let text = r#"
package = "mystery"
version = "1.0-1"
source = { url = detect_url() }
"#;
        let spec = read(text);
        assert_eq!(spec.package.as_deref(), Some("mystery"));
        assert_eq!(spec.source.url, None);
    }
}

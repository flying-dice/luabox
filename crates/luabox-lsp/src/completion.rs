//! Completion: after `.`/`:` on a receiver with a known class type, its
//! fields and methods; otherwise scope-visible locals, file-declared
//! globals/functions, and keywords. In a plain position the scope items are
//! augmented with **auto-require imports** (tsc-style): names exported by
//! other workspace modules but not yet in scope, each carrying an
//! `additionalTextEdits` insert of `local <name> = require("<module>").<name>`.
//! Deduplicated and sorted.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionItemLabelDetails, Position, Range, TextEdit,
};
use luabox_db::Analysis;
use luabox_hir::BindingKind;
use luabox_types::ty::Ty;

use crate::merged_ambient::MergedAmbient;
use crate::requires::{self, RequireExports};
use crate::sema::{self, FileSema};

/// Lua keywords offered in plain (non-member) positions.
const KEYWORDS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in",
    "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];

/// Compute completions at `offset` (the cursor's byte offset). `analysis` and
/// `project_root` back the auto-require pass — enumerating other modules'
/// exports and reversing each target file to its `require` module path.
/// `exports` is the shared `require` resolution ([`RequireExports`]), so
/// members of a `require` binding come from the same map the type pass checks
/// against (#54). `ambient` is the merged workspace environment the same
/// pass enforces (#56), so a class-typed receiver's members are offered with
/// the types `luabox check` gives them, wherever the class is declared.
#[must_use]
pub fn completion(
    sema: &FileSema,
    offset: usize,
    analysis: &Analysis,
    project_root: &Path,
    exports: &RequireExports,
    ambient: &MergedAmbient,
) -> Vec<CompletionItem> {
    let text = sema.index.text();
    let bytes = text.as_bytes();
    let offset = offset.min(bytes.len());

    // The identifier prefix being typed, and what precedes it.
    let mut start = offset;
    while start > 0 && is_ident_byte(bytes[start - 1]) {
        start -= 1;
    }
    let trigger = match start.checked_sub(1).map(|i| bytes[i]) {
        // `..` is concat, not member access.
        Some(b'.') if start < 2 || bytes[start - 2] != b'.' => Some(b'.'),
        Some(b':') => Some(b':'),
        _ => None,
    };

    let mut items: BTreeMap<String, CompletionItem> = BTreeMap::new();
    if let Some(trigger) = trigger {
        let resolvers = Resolvers {
            analysis,
            exports,
            ambient,
        };
        member_items(sema, text, start - 1, trigger, &resolvers, &mut items);
    } else {
        scope_items(sema, offset, &mut items);
        // Auto-require runs after scope items so names already in scope
        // (present in `items`) are left untouched.
        #[expect(
            clippy::string_slice,
            reason = "offset is a LineIndex byte offset (a char boundary); start walks back over ASCII identifier bytes, so it is one too"
        )]
        let prefix = &text[start..offset];
        auto_require_items(
            sema,
            offset,
            prefix,
            analysis,
            project_root,
            ambient,
            &mut items,
        );
    }
    items.into_values().collect()
}

/// Fields/methods of the receiver identifier ending at `dot_offset`: a
/// `require` binding's module's structural export first (#54), then the
/// receiver's class members resolved through the workspace ambient (#56) —
/// which already includes every class *this* file declares, so there is one
/// lookup, not a file-local one shadowed by a cross-file fallback (#46).
/// The three shared resolution sources a member-completion route reads, in
/// one value: the project database, the `require`-export resolution the type
/// pass checks against, and the merged workspace ambient. Every route needs
/// the same three, unchanged, so they travel together rather than as three
/// positional parameters each.
struct Resolvers<'a> {
    analysis: &'a Analysis,
    exports: &'a RequireExports,
    ambient: &'a MergedAmbient,
}

fn member_items(
    sema: &FileSema,
    text: &str,
    dot_offset: usize,
    trigger: u8,
    resolvers: &Resolvers<'_>,
    items: &mut BTreeMap<String, CompletionItem>,
) {
    let bytes = text.as_bytes();
    let mut recv_start = dot_offset;
    while recv_start > 0 && is_ident_byte(bytes[recv_start - 1]) {
        recv_start -= 1;
    }
    if recv_start == dot_offset {
        return;
    }
    #[expect(
        clippy::string_slice,
        reason = "dot_offset indexes an ASCII `.`/`:` and recv_start walks back over ASCII identifier bytes, so both are char boundaries"
    )]
    let receiver = &text[recv_start..dot_offset];
    require_member_items(sema, receiver, recv_start, trigger, resolvers, items);
    ambient_member_items(sema, receiver, recv_start, trigger, resolvers, items);
}

/// Members of a `require` binding: the named fields of the required module's
/// export type, out of the shared resolution the type pass checks against
/// (#54). Declines for every receiver that is not one, so the caller's other
/// routes are unaffected. Only when the binding carries no class reference of
/// its own — an explicit `---@type` beats an inferred module export, the
/// same precedence `hover::member_hover` uses (R7):
/// `requires::require_struct_fields` is the one shared gate.
///
/// Qualified by the *module* rather than the local name in the detail line,
/// matching the hover, so an item names where it came from rather than what
/// the file happened to call it.
fn require_member_items(
    sema: &FileSema,
    receiver: &str,
    recv_start: usize,
    trigger: u8,
    resolvers: &Resolvers<'_>,
    items: &mut BTreeMap<String, CompletionItem>,
) {
    let Some(binding) = sema.visible_binding_named(receiver, recv_start) else {
        return;
    };
    let Some((module, fields)) = requires::require_struct_fields(
        sema,
        resolvers.exports,
        resolvers.ambient,
        resolvers.analysis,
        binding,
    ) else {
        return;
    };
    for (name, field) in fields {
        let is_fun = crate::signature_help::as_function_ty(&field.ty).is_some();
        // After `:` only methods make sense.
        if trigger == b':' && !is_fun {
            continue;
        }
        let kind = if is_fun {
            if trigger == b':' {
                CompletionItemKind::METHOD
            } else {
                CompletionItemKind::FUNCTION
            }
        } else {
            CompletionItemKind::FIELD
        };
        items.insert(
            name.clone(),
            CompletionItem {
                label: name.clone(),
                kind: Some(kind),
                detail: Some(format!("{module}.{name}: {}", field.ty)),
                ..CompletionItem::default()
            },
        );
    }
}

/// Members of a class-typed receiver resolved through the workspace ambient
/// (#56): the receiver's annotated type names a class — declared in this
/// file or any other (#46) — or it is a `require` binding whose module
/// exports a class (both `---@class` spellings cross the boundary as
/// [`Ty::Named`]). The member surface is
/// [`crate::merged_ambient::MergedAmbient::class_members_of`], monomorphised
/// against the reference's own type arguments (#48) — the very shape the
/// checker enforces, so completion cannot offer what `luabox check` rejects
/// (or omit what it accepts, or offer it at the wrong type). Declines for
/// every other receiver.
///
/// Indexers are not offered (#53): an indexer declares no names, so there is
/// nothing for it to contribute to a member list — unlike hover, which must
/// answer for a *specific* name a dynamic-access class admits.
fn ambient_member_items(
    sema: &FileSema,
    receiver: &str,
    recv_start: usize,
    trigger: u8,
    resolvers: &Resolvers<'_>,
    items: &mut BTreeMap<String, CompletionItem>,
) {
    let Some(binding) = sema.visible_binding_named(receiver, recv_start) else {
        return;
    };
    let Some(ty) = requires::receiver_type(sema, resolvers.exports, binding) else {
        return;
    };
    let Some(class) = sema::named_of(&ty) else {
        return;
    };
    // Monomorphised against the reference's own type arguments, exactly as
    // the checker monomorphises the same reference at its use site (#48):
    // `Box<number>`'s `item` offers `number`, not the free `T` a bare-name
    // lookup would leave it as.
    let Some(shape) = resolvers.ambient.class_members_of(&ty) else {
        return;
    };
    for (name, field) in &shape.fields {
        let is_fun = crate::signature_help::as_function_ty(&field.ty).is_some();
        // After `:` only methods make sense.
        if trigger == b':' && !is_fun {
            continue;
        }
        let kind = if is_fun {
            if trigger == b':' {
                CompletionItemKind::METHOD
            } else {
                CompletionItemKind::FUNCTION
            }
        } else {
            CompletionItemKind::FIELD
        };
        items.entry(name.clone()).or_insert_with(|| CompletionItem {
            label: name.clone(),
            kind: Some(kind),
            detail: Some(format!("{class}.{name}: {}", field.ty)),
            ..CompletionItem::default()
        });
    }
}

/// Locals visible at `offset`, file-declared functions/globals, and keywords.
fn scope_items(sema: &FileSema, offset: usize, items: &mut BTreeMap<String, CompletionItem>) {
    for keyword in KEYWORDS {
        items.insert(
            (*keyword).to_string(),
            CompletionItem {
                label: (*keyword).to_string(),
                kind: Some(CompletionItemKind::KEYWORD),
                ..CompletionItem::default()
            },
        );
    }
    for (name, _) in sema.global_defs() {
        items.insert(
            name.clone(),
            CompletionItem {
                label: name,
                kind: Some(CompletionItemKind::VARIABLE),
                ..CompletionItem::default()
            },
        );
    }
    for info in sema.functions() {
        // Methods complete after `:`, not in plain scope.
        if info.name.contains(':') {
            continue;
        }
        items.insert(
            info.name.clone(),
            CompletionItem {
                label: info.name.clone(),
                kind: Some(CompletionItemKind::FUNCTION),
                detail: Some(info.sig),
                ..CompletionItem::default()
            },
        );
    }
    // Locals last: they override same-named globals/keywords in the map.
    for binding in sema.bindings_before(offset) {
        let kind = if binding.kind == BindingKind::LocalFunction {
            CompletionItemKind::FUNCTION
        } else {
            CompletionItemKind::VARIABLE
        };
        let detail = sema.binding_type(binding).map(|ty| sema::render_type(&ty));
        items.insert(
            binding.name.clone(),
            CompletionItem {
                label: binding.name.clone(),
                kind: Some(kind),
                detail,
                ..CompletionItem::default()
            },
        );
    }
}

/// Append auto-require import completions: for every other workspace module,
/// each exported name that matches `prefix`, is not already in scope, and
/// whose module is not already required here, offered with an
/// `additionalTextEdits` insert of `local <name> = require("<module>").<name>`.
///
/// A module's export is a structural table for the overwhelmingly common
/// `local M = {} … return M` shape; a `---@class` module export is
/// [`Ty::Named`] instead (#56, both spellings), so its members are read
/// through the workspace ambient the same way completion's member route
/// reads them — `auto_require_items` cannot offer a carrier module's names
/// by any other route, since [`Ty::Named`] carries none itself.
///
/// Runs on every plain-position completion; the per-file `module_export` is
/// salsa-memoized, so the cost is one cached lookup per workspace file. An
/// empty prefix is skipped — auto-require is a targeted, prefix-driven suggest,
/// not a dump of the whole workspace surface on every keystroke.
fn auto_require_items(
    sema: &FileSema,
    offset: usize,
    prefix: &str,
    analysis: &Analysis,
    project_root: &Path,
    ambient: &MergedAmbient,
    items: &mut BTreeMap<String, CompletionItem>,
) {
    if prefix.is_empty() {
        return;
    }

    // Modules this file already `require`s, under any local name.
    let required: HashSet<&str> = sema.requires().iter().map(|e| e.module.as_str()).collect();

    // The require insert lands on its own line; placement is shared, the
    // statement text differs per candidate (the editor applies only the
    // accepted item's edit).
    let (anchor, leading) = require_anchor(sema);

    // Deterministic first-wins when two modules export the same name.
    let mut files: Vec<&Path> = analysis.files().filter(|&f| f != sema.path).collect();
    files.sort_unstable();

    for file in files {
        let Some(module) = module_path(project_root, file) else {
            continue;
        };
        if required.contains(module.as_str()) {
            continue;
        }
        let Some(export) = analysis.module_export(file) else {
            continue;
        };
        match export.ty() {
            Some(Ty::Table(table)) => auto_require_fields(
                sema,
                offset,
                prefix,
                &module,
                &table.fields,
                anchor,
                leading,
                items,
            ),
            Some(Ty::Named(class)) => {
                if let Some(shape) = ambient.class_members(class) {
                    auto_require_fields(
                        sema,
                        offset,
                        prefix,
                        &module,
                        &shape.fields,
                        anchor,
                        leading,
                        items,
                    );
                }
            }
            _ => {}
        }
    }
}

/// One module's candidate names for auto-require, out of its export's field
/// map — a structural table's own fields, or a `---@class` export's members
/// (#49). Shared so both shapes go through the exact same offer/insert
/// logic; only where the fields came from differs.
#[expect(
    clippy::too_many_arguments,
    reason = "an internal helper, not a public API"
)]
fn auto_require_fields(
    sema: &FileSema,
    offset: usize,
    prefix: &str,
    module: &str,
    fields: &std::collections::BTreeMap<String, luabox_types::ty::FieldTy>,
    anchor: Position,
    leading: bool,
    items: &mut BTreeMap<String, CompletionItem>,
) {
    for (name, field) in fields {
        if !name.starts_with(prefix) {
            continue;
        }
        // Already in scope (a local/upvalue) or already offered by scope
        // completion (a global/function/keyword) — leave it be.
        if items.contains_key(name) || sema.visible_binding_named(name, offset).is_some() {
            continue;
        }
        let kind = if crate::signature_help::as_function_ty(&field.ty).is_some() {
            CompletionItemKind::FUNCTION
        } else {
            CompletionItemKind::VARIABLE
        };
        // `name` is a field of the module's export, so bind the field itself
        // (`require("m").name`) — binding the whole module to a field-named
        // local would make `name(...)` call the table.
        let stmt = format!("local {name} = require(\"{module}\").{name}");
        let new_text = if leading {
            format!("\n{stmt}")
        } else {
            format!("{stmt}\n")
        };
        items.insert(
            name.clone(),
            CompletionItem {
                label: name.clone(),
                kind: Some(kind),
                detail: Some(format!("Auto import from \"{module}\"")),
                label_details: Some(CompletionItemLabelDetails {
                    detail: None,
                    description: Some(module.to_string()),
                }),
                additional_text_edits: Some(vec![TextEdit {
                    range: Range::new(anchor, anchor),
                    new_text,
                }]),
                ..CompletionItem::default()
            },
        );
    }
}

/// Where a new `require` line should be inserted: the LSP [`Position`] anchor
/// (a zero-width point) and whether the statement text must be prefixed with a
/// newline (`leading`) rather than suffixed. Placed after the file's last
/// existing `require` statement, else after any leading comment/`---@meta`
/// header, else at the very top.
fn require_anchor(sema: &FileSema) -> (Position, bool) {
    let text = sema.index.text();
    let offset = require_insertion_offset(sema, text);
    // Prefix a newline when the anchor is not itself a line start (an
    // unterminated final line); otherwise suffix one so the statement occupies
    // its own line and pushes any following code down.
    let leading = offset > 0 && text.as_bytes().get(offset - 1) != Some(&b'\n');
    (sema.index.position(offset), leading)
}

/// The byte offset of the line start where a new `require` should be inserted.
fn require_insertion_offset(sema: &FileSema, text: &str) -> usize {
    // 1. Just after the last existing `require(...)` statement's line.
    if let Some(end) = sema
        .requires()
        .iter()
        .map(|e| usize::from(e.range.end()))
        .max()
    {
        #[expect(
            clippy::string_slice,
            reason = "end is a rowan range end, which always lands on a char boundary"
        )]
        return match text[end..].find('\n') {
            Some(rel) => end + rel + 1,
            None => text.len(),
        };
    }
    // 2. After the leading comment / `---@meta` header (and blank lines).
    header_end(text)
}

/// The byte offset of the first line that is neither blank nor a `--` comment
/// — where a require belongs when the file has no existing requires.
fn header_end(text: &str) -> usize {
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with("--") {
            offset += line.len();
        } else {
            break;
        }
    }
    offset
}

/// Reverse of `require` resolution: a workspace `.lua` file under `root` to its
/// module path (`<root>/a/b/c.lua` → `"a.b.c"`, `<root>/a/b/init.lua` →
/// `"a.b"`). `None` for a file outside `root` or a bare root `init.lua`.
fn module_path(root: &Path, file: &Path) -> Option<String> {
    let rel = file.strip_prefix(root).ok()?;
    let mut segments: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let last = segments.last_mut()?;
    if last == "init.lua" {
        segments.pop();
    } else {
        *last = last.strip_suffix(".lua")?.to_string();
    }
    (!segments.is_empty()).then(|| segments.join("."))
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

// test code — panics document assumptions
#[allow(
    clippy::string_slice,
    reason = "test code — panics document assumptions"
)]
#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code — panics document assumptions"
)]
mod tests {
    use std::path::{Path, PathBuf};

    use luabox_db::{Analysis, AnalysisHost, Change, Dialect, Strictness};

    use luabox_types::RockSurfaces;

    use super::{CompletionItem, CompletionItemKind, completion};
    use super::{FileSema, MergedAmbient, RequireExports, header_end, module_path};

    /// The workspace root every test file lives under.
    fn root() -> PathBuf {
        PathBuf::from(if cfg!(windows) { r"C:\ws" } else { "/ws" })
    }

    fn analyze(files: &[(&str, &str)]) -> (Analysis, PathBuf) {
        let mut host = AnalysisHost::new(Dialect::Lua54, Strictness::Warn);
        // Anchoring the root is what lets cross-file `require` resolve, which
        // the shared export map (#54) depends on.
        host.set_root(root());
        let mut first = None;
        for (rel, text) in files {
            let path = root().join(rel);
            first.get_or_insert_with(|| path.clone());
            host.apply_change(Change::SetFileText {
                path,
                dialect: Dialect::Lua54,
                text: (*text).to_string(),
            });
        }
        (host.snapshot(), first.expect("at least one file"))
    }

    /// Completions at the byte offset just past `needle` in the first file,
    /// with `rocks` in the shared `require` resolution.
    fn after_with(
        files: &[(&str, &str)],
        needle: &str,
        rocks: &RockSurfaces,
    ) -> Vec<CompletionItem> {
        let src = files[0].1;
        let offset = src.find(needle).expect("needle present") + needle.len();
        let (analysis, path) = analyze(files);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let exports = RequireExports::resolve(&analysis, &path, rocks);
        // The merged workspace layer, exactly as the server builds it (#56).
        let base = luabox_types::stdlib_defs(luabox_syntax::lua::Dialect::Lua54);
        let ambient = MergedAmbient::build(base, &analysis.project_types(), rocks.types());
        completion(&sema, offset, &analysis, &root(), &exports, &ambient)
    }

    /// Completions at the byte offset just past `needle` in the first file.
    fn after(files: &[(&str, &str)], needle: &str) -> Vec<CompletionItem> {
        after_with(files, needle, &RockSurfaces::default())
    }

    /// A two-file workspace's annotated module, mirroring `hover`'s.
    ///
    /// `version` is deliberately unannotated: its type is whatever the module
    /// surface inferred (the string *literal*), and that is precisely what
    /// completion must show — the type the checker will hold a use site to.
    /// Widening it for display would be prettier and would be the editor
    /// disagreeing with CI, which is the bug this file is about.
    const OTHER: &str = "\
local M = {}

M.version = \"1.0\"

---Helps.
---@param n number
---@return string
function M.helper(n) return tostring(n) end

return M
";

    fn labels(items: &[CompletionItem]) -> Vec<&str> {
        items.iter().map(|i| i.label.as_str()).collect()
    }

    fn item<'a>(items: &'a [CompletionItem], label: &str) -> &'a CompletionItem {
        items
            .iter()
            .find(|i| i.label == label)
            .unwrap_or_else(|| panic!("no `{label}` in {:?}", labels(items)))
    }

    #[test]
    fn a_dot_with_no_receiver_offers_nothing() {
        // A leading `.` has no identifier before it to resolve.
        let items = after(&[("main.lua", "local x = 1\n.\n")], "\n.");
        assert!(items.is_empty(), "{:?}", labels(&items));
    }

    #[test]
    fn a_receiver_with_no_known_class_offers_nothing() {
        let items = after(&[("main.lua", "local t = {}\nt.\n")], "t.");
        assert!(items.is_empty(), "{:?}", labels(&items));
    }

    #[test]
    fn a_colon_trigger_offers_only_function_typed_fields() {
        let src = "\
---@class Greeter
---@field name string
---@field greet fun(self: Greeter): string

---@type Greeter
local g = nil
g:
";
        let items = after(&[("main.lua", src)], "g:");
        assert_eq!(labels(&items), vec!["greet"], "{:?}", labels(&items));
        assert_eq!(items[0].kind, Some(CompletionItemKind::METHOD));
    }

    /// N19: an *optional* function-typed field (`fun(...)?`, lowered to
    /// `Ty::Function | Ty::Nil`) must be offered after `:` and carry the
    /// `METHOD` kind — the same predicate `signature_help`'s `as_function_ty`
    /// already uses (round 5 `an_optional_function_typed_field_still_renders_a_signature`).
    /// Before the fix, completion's flat `matches!(field.ty, Ty::Function(_))`
    /// missed the union, so the field was silently absent from the `:` list
    /// while signature help rendered a full signature for it — the editor
    /// asserting the member both does not exist and is callable off the same
    /// keystroke sequence.
    #[test]
    fn a_colon_trigger_offers_an_optional_function_typed_field_too() {
        let src = "\
---@class C
---@field grow fun(amount: number)? optional method
---@field walk fun(steps: number) plain method

---@type C
local c = nil
c:
";
        let items = after(&[("main.lua", src)], "c:");
        assert_eq!(labels(&items), vec!["grow", "walk"], "{:?}", labels(&items));
        assert_eq!(item(&items, "grow").kind, Some(CompletionItemKind::METHOD));
        assert_eq!(item(&items, "walk").kind, Some(CompletionItemKind::METHOD));
    }

    /// N19: the same predicate drives the completion item `kind` on the
    /// plain `.`-triggered list too — an optional function field must report
    /// `FUNCTION`, not `FIELD`.
    #[test]
    fn a_dot_trigger_reports_the_function_kind_for_an_optional_function_field() {
        let src = "\
---@class C
---@field grow fun(amount: number)?

---@type C
local c = nil
c.
";
        let items = after(&[("main.lua", src)], "c.");
        assert_eq!(
            item(&items, "grow").kind,
            Some(CompletionItemKind::FUNCTION),
            "{:?}",
            labels(&items)
        );
    }

    #[test]
    fn a_dot_trigger_offers_fields_and_functions_with_their_types() {
        let src = "\
---@class Greeter
---@field name string
---@field greet fun(self: Greeter): string

---@type Greeter
local g = nil
g.
";
        let items = after(&[("main.lua", src)], "g.");
        assert_eq!(
            labels(&items),
            vec!["greet", "name"],
            "{:?}",
            labels(&items)
        );
        assert_eq!(
            item(&items, "greet").kind,
            Some(CompletionItemKind::FUNCTION)
        );
        assert_eq!(item(&items, "name").kind, Some(CompletionItemKind::FIELD));
        assert_eq!(
            item(&items, "name").detail.as_deref(),
            Some("Greeter.name: string")
        );
    }

    #[test]
    fn an_indexer_field_key_is_not_offered_as_a_member() {
        let src = "\
---@class Bag
---@field [string] number
---@field size number

---@type Bag
local b = nil
b.
";
        let items = after(&[("main.lua", src)], "b.");
        assert_eq!(labels(&items), vec!["size"], "{:?}", labels(&items));
    }

    // === the ambient arm reaches a file-local receiver class too (#47) ====
    //
    // The defect: `ambient_member_items` lived only in the `else` arm of a
    // check on the receiver's *file-local* class, so a receiver whose class
    // was declared in this file fell straight to the file-local
    // `class_fields` scan and never saw an inherited member whose parent
    // lives in another file. Measured before the fix: `s.` offered `name`
    // and omitted `id`.

    #[test]
    fn a_member_inherited_from_a_parent_declared_in_another_file_is_offered() {
        let files = [
            (
                "main.lua",
                "---@class Sub : Base\n---@field name string\n\n---@type Sub\nlocal s = nil\ns.\n",
            ),
            ("other.lua", "---@class Base\n---@field id number\n"),
        ];
        let items = after(&files, "s.");
        assert_eq!(labels(&items), vec!["id", "name"], "{:?}", labels(&items));
    }

    /// Probing the other direction: a name neither class declares stays
    /// absent, even once the ambient arm is reachable for a file-local
    /// receiver.
    #[test]
    fn a_member_neither_class_declares_is_not_offered() {
        let files = [
            (
                "main.lua",
                "---@class Sub : Base\n---@field name string\n\n---@type Sub\nlocal s = nil\ns.\n",
            ),
            ("other.lua", "---@class Base\n---@field id number\n"),
        ];
        let items = after(&files, "s.");
        assert!(!labels(&items).contains(&"nope"), "{:?}", labels(&items));
    }

    // === a bound generic reference resolves its type argument (#48) =======

    #[test]
    fn a_bound_generic_receivers_member_offers_the_bound_type_in_its_detail() {
        let files = [
            ("main.lua", "---@type Box<number>\nlocal b = nil\nb.\n"),
            ("box.lua", "---@class Box<T>\n---@field item T\n"),
        ];
        let items = after(&files, "b.");
        assert_eq!(
            item(&items, "item").detail.as_deref(),
            Some("Box.item: number")
        );
    }

    /// The one-variable control: the identical class referenced bare stays
    /// lenient — the free `T` — exactly as it did before #48.
    #[test]
    fn an_unbound_generic_receiver_still_offers_the_free_parameter() {
        let files = [
            ("main.lua", "---@type Box\nlocal b = nil\nb.\n"),
            ("box.lua", "---@class Box<T>\n---@field item T\n"),
        ];
        let items = after(&files, "b.");
        assert_eq!(item(&items, "item").detail.as_deref(), Some("Box.item: T"));
    }

    // === an alias-typed class field (round 4 review R27) ==================
    //
    // Completion's `ambient_member_items` resolves through the same merged
    // `Ty` hover does (#56); no test declared an `---@alias`-typed field on
    // either surface before. This pins completion's half.

    #[test]
    fn an_alias_typed_field_offers_the_alias_expanded_in_its_detail() {
        let src = "\
---@alias Direction \"up\"|\"down\"

---@class Compass
---@field dir Direction

---@type Compass
local c = nil
c.
";
        let items = after(&[("main.lua", src)], "c.");
        assert_eq!(
            item(&items, "dir").detail.as_deref(),
            Some("Compass.dir: \"up\"|\"down\"")
        );
    }

    #[test]
    fn a_concat_operator_is_not_a_member_trigger() {
        // `..` is concatenation, so scope completion applies, not members.
        let items = after(&[("main.lua", "local s = \"a\"..\n")], "..");
        assert!(labels(&items).contains(&"local"), "{:?}", labels(&items));
    }

    #[test]
    fn scope_completion_offers_globals_functions_and_keywords() {
        let src = "\
answer = 1
function helper(n) return n end
function Cls:method() end
local visible = 2

";
        let items = after(&[("main.lua", src)], "local visible = 2\n");
        let names = labels(&items);
        assert!(names.contains(&"answer"), "{names:?}");
        assert!(names.contains(&"helper"), "{names:?}");
        assert!(names.contains(&"visible"), "{names:?}");
        assert!(names.contains(&"while"), "{names:?}");
        // A method completes after `:`, never in plain scope.
        assert!(!names.contains(&"Cls:method"), "{names:?}");
        assert_eq!(
            item(&items, "answer").kind,
            Some(CompletionItemKind::VARIABLE)
        );
        assert_eq!(
            item(&items, "helper").detail.as_deref(),
            Some("function helper(n)")
        );
    }

    #[test]
    fn auto_require_needs_a_typed_prefix() {
        // With no prefix the auto-require pass declines outright, so no item
        // carries an import edit.
        let files = [
            ("main.lua", "local x = 1\n\n"),
            (
                "lib.lua",
                "local M = {}\nfunction M.frobnicate() end\nreturn M\n",
            ),
        ];
        let items = after(&files, "local x = 1\n");
        assert!(
            items.iter().all(|i| i.additional_text_edits.is_none()),
            "{:?}",
            labels(&items)
        );
    }

    #[test]
    fn auto_require_offers_an_unimported_module_export() {
        let files = [
            ("main.lua", "local x = 1\nfrob\n"),
            (
                "lib.lua",
                "local M = {}\nfunction M.frobnicate() end\nreturn M\n",
            ),
        ];
        let items = after(&files, "frob");
        let offered = item(&items, "frobnicate");
        assert_eq!(offered.kind, Some(CompletionItemKind::FUNCTION));
        assert_eq!(offered.detail.as_deref(), Some("Auto import from \"lib\""));
        let edits = offered
            .additional_text_edits
            .as_ref()
            .expect("an import edit");
        assert_eq!(edits.len(), 1);
        assert!(
            edits[0]
                .new_text
                .contains("local frobnicate = require(\"lib\").frobnicate"),
            "{edits:?}"
        );
    }

    /// N19: `auto_require_fields`'s kind decision is the third of the three
    /// sites that used to read `matches!(field.ty, Ty::Function(_))` flat —
    /// an optional function-typed export field must still report `FUNCTION`,
    /// not `VARIABLE`.
    #[test]
    fn auto_require_reports_the_function_kind_for_an_optional_function_export() {
        let files = [
            ("main.lua", "local x = 1\nfrob\n"),
            (
                "lib.lua",
                "local M = {}\n---@type fun()?\nM.frobnicate = nil\nreturn M\n",
            ),
        ];
        let items = after(&files, "frob");
        let offered = item(&items, "frobnicate");
        assert_eq!(offered.kind, Some(CompletionItemKind::FUNCTION));
    }

    #[test]
    fn auto_require_skips_a_module_that_is_already_required() {
        let files = [
            ("main.lua", "local lib = require(\"lib\")\nfrob\n"),
            (
                "lib.lua",
                "local M = {}\nfunction M.frobnicate() end\nreturn M\n",
            ),
        ];
        let items = after(&files, "frob");
        assert!(
            !labels(&items).contains(&"frobnicate"),
            "{:?}",
            labels(&items)
        );
    }

    #[test]
    fn auto_require_skips_a_name_already_visible_in_scope() {
        let files = [
            ("main.lua", "local frobnicate = 1\nfrob\n"),
            (
                "lib.lua",
                "local M = {}\nfunction M.frobnicate() end\nreturn M\n",
            ),
        ];
        // Anchor past the declaration: the bare `frob` prefix on line 1.
        let items = after(&files, "1\nfrob");
        assert!(
            item(&items, "frobnicate").additional_text_edits.is_none(),
            "the in-scope local wins"
        );
    }

    #[test]
    fn auto_require_skips_a_module_with_no_table_export() {
        let files = [
            ("main.lua", "local x = 1\nfrob\n"),
            ("lib.lua", "return 42\n"),
        ];
        let items = after(&files, "frob");
        assert!(
            items.iter().all(|i| i.additional_text_edits.is_none()),
            "{:?}",
            labels(&items)
        );
    }

    /// #49: `module_export` reifies a declared carrier's export to
    /// [`super::Ty::Named`] rather than a structural table (#56), and
    /// `auto_require_items` used to match on [`super::Ty::Table`] alone —
    /// so a carrier module's names silently stopped being offered. Measured
    /// before the fix: typing `mak` in another file offered nothing for
    /// `Widget.make`, though it did before the PR that introduced the
    /// reification.
    #[test]
    fn auto_require_offers_a_class_carrier_modules_members() {
        let files = [
            ("main.lua", "local x = 1\nmak\n"),
            (
                "widget.lua",
                "---@class Widget\nlocal W = {}\nfunction W.make() end\nreturn W\n",
            ),
        ];
        let items = after(&files, "mak");
        let offered = item(&items, "make");
        assert_eq!(offered.kind, Some(CompletionItemKind::FUNCTION));
        assert_eq!(
            offered.detail.as_deref(),
            Some("Auto import from \"widget\"")
        );
        let edits = offered
            .additional_text_edits
            .as_ref()
            .expect("an import edit");
        assert!(
            edits[0]
                .new_text
                .contains("local make = require(\"widget\").make"),
            "{edits:?}"
        );
    }

    /// Probing the other direction: a class carrier's field the prefix does
    /// not match stays unoffered — the fix reaches the right shape without
    /// suppressing the prefix filter.
    #[test]
    fn auto_require_on_a_class_carrier_module_still_respects_the_prefix() {
        let files = [
            ("main.lua", "local x = 1\nmak\n"),
            (
                "widget.lua",
                "---@class Widget\n---@field label string\nlocal W = {}\nfunction W.make() end\nreturn W\n",
            ),
        ];
        let items = after(&files, "mak");
        assert!(!labels(&items).contains(&"label"), "{:?}", labels(&items));
    }

    #[test]
    fn a_cursor_past_the_end_of_the_file_is_clamped() {
        let src = "local x = 1\n";
        let (analysis, path) = analyze(&[("main.lua", src)]);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let exports = RequireExports::resolve(&analysis, &path, &RockSurfaces::default());
        let base = luabox_types::stdlib_defs(luabox_syntax::lua::Dialect::Lua54);
        let ambient = MergedAmbient::build(base, &analysis.project_types(), &[]);
        let items = completion(
            &sema,
            src.len() + 500,
            &analysis,
            &root(),
            &exports,
            &ambient,
        );
        assert!(labels(&items).contains(&"x"), "{:?}", labels(&items));
    }

    #[test]
    fn module_path_reverses_require_resolution() {
        let root = Path::new("/proj");
        assert_eq!(
            module_path(root, Path::new("/proj/a/b/c.lua")).as_deref(),
            Some("a.b.c")
        );
        assert_eq!(
            module_path(root, Path::new("/proj/geometry.lua")).as_deref(),
            Some("geometry")
        );
    }

    #[test]
    fn module_path_handles_init_lua() {
        let root = Path::new("/proj");
        // `<dir>/init.lua` is the module `<dir>`, not `<dir>.init`.
        assert_eq!(
            module_path(root, Path::new("/proj/foo/init.lua")).as_deref(),
            Some("foo")
        );
        assert_eq!(
            module_path(root, Path::new("/proj/a/b/init.lua")).as_deref(),
            Some("a.b")
        );
        // A bare root `init.lua` reverses to an empty module: not offerable.
        assert_eq!(module_path(root, Path::new("/proj/init.lua")), None);
    }

    #[test]
    fn module_path_rejects_files_outside_root_and_non_lua() {
        let root = Path::new("/proj");
        assert_eq!(module_path(root, Path::new("/other/x.lua")), None);
        assert_eq!(module_path(root, Path::new("/proj/x.txt")), None);
    }

    // === `require` bindings (#54) =========================================
    //
    // The hover half of #54 was measured; completion was not, so it is pinned
    // here too — members of a `require` binding come from the same shared
    // export map, so the two surfaces cannot drift apart again.

    #[test]
    fn a_require_binding_offers_its_module_members() {
        let files = [
            ("main.lua", "local m = require(\"other\")\nm.\n"),
            ("other.lua", OTHER),
        ];
        let items = after(&files, "m.");
        assert_eq!(
            labels(&items),
            vec!["helper", "version"],
            "{:?}",
            labels(&items)
        );
        assert_eq!(
            item(&items, "helper").kind,
            Some(CompletionItemKind::FUNCTION)
        );
        assert_eq!(
            item(&items, "version").kind,
            Some(CompletionItemKind::FIELD)
        );
        assert_eq!(
            item(&items, "version").detail.as_deref(),
            Some("other.version: \"1.0\"")
        );
    }

    /// R7: an explicit `---@type` on a `require` binding must win over the
    /// module's plain structural table export in completion too — the same
    /// precedence hover uses (`an_explicit_annotation_on_a_require_binding_wins_a_member_hover_too`).
    /// Before the fix, `require_member_items` ran unconditionally and
    /// inserted `x` from the table shape first; `ambient_member_items`'s
    /// `or_insert_with` could then never override it with `Point.x`.
    #[test]
    fn an_explicit_annotation_on_a_require_binding_wins_member_completion_too() {
        let files = [
            (
                "main.lua",
                "---@class Point\n---@field x number\n\n---@type Point\nlocal m = require(\"m\")\nm.\n",
            ),
            ("m.lua", "local M = {}\nM.x = 42\nreturn M\n"),
        ];
        let items = after(&files, "m.");
        assert_eq!(
            item(&items, "x").detail.as_deref(),
            Some("Point.x: number"),
            "{:?}",
            labels(&items)
        );
    }

    /// The one-variable control: with no `---@type` at all, the same module
    /// still offers its structural export exactly as before.
    #[test]
    fn an_unannotated_require_bindings_member_completion_still_offers_the_structural_export() {
        let files = [
            ("main.lua", "local m = require(\"m\")\nm.\n"),
            ("m.lua", "local M = {}\nM.x = 42\nreturn M\n"),
        ];
        let items = after(&files, "m.");
        assert_eq!(
            item(&items, "x").detail.as_deref(),
            Some("m.x: 42"),
            "{:?}",
            labels(&items)
        );
    }

    /// N16: `require_struct_fields`'s gate must be a *resolves* check, not a
    /// *presence* check. `---@type table` is a real annotation but `table`
    /// is not a class the ambient can resolve members from, so the
    /// structural fallback must still offer `x` — before the fix, any
    /// `---@type` at all suppressed `require_member_items` regardless of
    /// whether the class arm (`ambient_member_items`) could resolve
    /// anything, and the list came back empty.
    #[test]
    fn an_annotation_that_does_not_resolve_to_a_class_falls_back_to_the_structural_export_completion()
     {
        let files = [
            ("main.lua", "---@type table\nlocal m = require(\"m\")\nm.\n"),
            ("m.lua", "local M = {}\nM.x = 42\nreturn M\n"),
        ];
        let items = after(&files, "m.");
        assert_eq!(
            item(&items, "x").detail.as_deref(),
            Some("m.x: 42"),
            "{:?}",
            labels(&items)
        );
    }

    /// Same shape with an annotation naming a class that does not exist —
    /// the mid-edit case.
    #[test]
    fn an_annotation_naming_an_undeclared_class_falls_back_to_the_structural_export_completion() {
        let files = [
            ("main.lua", "---@type Bogus\nlocal m = require(\"m\")\nm.\n"),
            ("m.lua", "local M = {}\nM.x = 42\nreturn M\n"),
        ];
        let items = after(&files, "m.");
        assert_eq!(
            item(&items, "x").detail.as_deref(),
            Some("m.x: 42"),
            "{:?}",
            labels(&items)
        );
    }

    #[test]
    fn a_colon_trigger_on_a_require_binding_offers_only_functions() {
        let files = [
            ("main.lua", "local m = require(\"other\")\nm:\n"),
            ("other.lua", OTHER),
        ];
        let items = after(&files, "m:");
        assert_eq!(labels(&items), vec!["helper"], "{:?}", labels(&items));
        assert_eq!(
            item(&items, "helper").kind,
            Some(CompletionItemKind::METHOD)
        );
    }

    #[test]
    fn a_rock_require_binding_offers_the_harvested_members() {
        let source = luabox_types::RockModule {
            module: "mylib".to_string(),
            label: "lua_modules/share/lua/5.4/mylib/init.lua".to_string(),
            path: root().join("lua_modules/share/lua/5.4/mylib/init.lua"),
            text: "\
local M = {}

---@param who string
---@return string
function M.greet(who) return \"hi \" .. who end

return M
"
            .to_string(),
        };
        let ambient = luabox_types::build_ambient(Dialect::Lua54, &[]);
        let rocks = luabox_types::rocks::harvest(&ambient, &[source]);
        let files = [("main.lua", "local mylib = require(\"mylib\")\nmylib.\n")];
        let items = after_with(&files, "mylib.", &rocks);
        assert_eq!(labels(&items), vec!["greet"], "{:?}", labels(&items));
    }

    #[test]
    fn a_require_binding_for_an_absent_module_offers_nothing() {
        let files = [("main.lua", "local m = require(\"absent\")\nm.\n")];
        assert!(after(&files, "m.").is_empty());
    }

    #[test]
    fn a_dynamic_require_binding_offers_nothing_by_design() {
        let files = [
            (
                "main.lua",
                "local name = \"other\"\nlocal m = require(name)\nm.\n",
            ),
            ("other.lua", OTHER),
        ];
        assert!(after(&files, "m.").is_empty());
    }

    #[test]
    fn header_end_skips_leading_comments_and_blanks() {
        // Insert after a `---@meta`/comment header and its blank lines.
        let text = "---@meta\n-- a note\n\nlocal x = 1\n";
        let offset = header_end(text);
        assert_eq!(&text[..offset], "---@meta\n-- a note\n\n");
        // No header: insertion is the very top.
        assert_eq!(header_end("local x = 1\n"), 0);
        // All comments: insertion is the end of the file.
        let all = "-- one\n-- two\n";
        assert_eq!(header_end(all), all.len());
    }
}

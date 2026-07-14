// test code — panics document assumptions
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::string_slice
)]
//! Boundary tests for `luabox_hir::lower`: desugarings, name resolution,
//! `require` extraction, literal decoding, goto/label resolution, and the
//! source-map roundtrip — all through the public API only (no token kinds).

use luabox_hir::{
    Attrib, BindingKind, Block, BodyId, Expr, ExprId, HirId, Literal, LoweredFile, Number,
    Resolution, Stmt, StmtId, TableEntry,
};
use luabox_syntax::lua::{Dialect, parse};

/// Parse (asserting no syntax errors) and lower.
fn lowered(src: &str) -> LoweredFile {
    lowered_in(src, Dialect::Lua54)
}

fn lowered_in(src: &str, dialect: Dialect) -> LoweredFile {
    let parse = parse(src, dialect);
    assert_eq!(parse.errors(), &[], "fixture must parse cleanly: {src}");
    luabox_hir::lower(&parse)
}

/// The chunk's top-level statements.
fn chunk_stmts(file: &LoweredFile) -> Vec<StmtId> {
    file.body(file.chunk()).block.stmts.clone()
}

/// All (body, expr) pairs whose expr is `Name(name)`, in allocation order.
fn name_exprs(file: &LoweredFile, name: &str) -> Vec<(BodyId, ExprId)> {
    let mut out = Vec::new();
    for (body_id, body) in file.bodies() {
        for (expr_id, expr) in body.exprs() {
            if matches!(expr, Expr::Name(n) if n == name) {
                out.push((body_id, expr_id));
            }
        }
    }
    out
}

/// The resolutions of every `Name(name)` expr in the file, allocation order.
fn resolutions_of(file: &LoweredFile, name: &str) -> Vec<Resolution> {
    name_exprs(file, name)
        .into_iter()
        .map(|(body, expr)| {
            file.resolution(HirId::expr(body, expr))
                .expect("name exprs always have a resolution")
                .clone()
        })
        .collect()
}

fn source_text(src: &str, file: &LoweredFile, id: HirId) -> String {
    let range = file.source_map().range(id).expect("mapped node");
    src[usize::from(range.start())..usize::from(range.end())].to_string()
}

// === Desugaring: function declarations ===

#[test]
fn method_decl_desugars_to_assign_with_self_param() {
    let file = lowered("function a.b:c(x) return self, x end");
    let chunk = file.chunk();
    let [stmt] = chunk_stmts(&file)[..] else {
        panic!("expected one statement");
    };
    // `function a.b:c(x)` == `a.b.c = function(self, x) ... end`.
    let Stmt::Assign { targets, values } = file.body(chunk).stmt(stmt) else {
        panic!("method decl must lower to an assignment");
    };
    let [target] = targets[..] else {
        panic!("one target");
    };
    let [value] = values[..] else {
        panic!("one value");
    };

    // Target is a.b.c: Index(Index(Name a, "b"), "c") with from_field.
    let Expr::Index {
        base,
        index,
        from_field: true,
    } = file.body(chunk).expr(target)
    else {
        panic!("target must be a field-style index");
    };
    assert!(matches!(
        file.body(chunk).expr(*index),
        Expr::Literal(Literal::String(s)) if s.as_str() == Some("c")
    ));
    let Expr::Index {
        base: inner_base,
        index: inner_index,
        from_field: true,
    } = file.body(chunk).expr(*base)
    else {
        panic!("middle segment must be a field-style index");
    };
    assert!(matches!(
        file.body(chunk).expr(*inner_index),
        Expr::Literal(Literal::String(s)) if s.as_str() == Some("b")
    ));
    assert!(matches!(
        file.body(chunk).expr(*inner_base),
        Expr::Name(n) if n == "a"
    ));

    // Value is a function with implicit `self` then `x`.
    let Expr::Function(func_body) = file.body(chunk).expr(value) else {
        panic!("value must be a function");
    };
    let func = file.body(*func_body);
    let param_names: Vec<_> = func
        .params
        .iter()
        .map(|&p| file.binding(p).name.clone())
        .collect();
    assert_eq!(param_names, ["self", "x"]);
    assert_eq!(file.binding(func.params[0]).kind, BindingKind::SelfParam);
    assert_eq!(file.binding(func.params[1]).kind, BindingKind::Param);

    // `self` inside the body resolves to the implicit param.
    let [res] = &resolutions_of(&file, "self")[..] else {
        panic!("one `self` reference");
    };
    assert_eq!(*res, Resolution::Local(func.params[0]));
}

#[test]
fn plain_function_decl_has_no_self() {
    let file = lowered("function f(x) end");
    let chunk = file.chunk();
    let [stmt] = chunk_stmts(&file)[..] else {
        panic!("one statement");
    };
    let Stmt::Assign { targets, values } = file.body(chunk).stmt(stmt) else {
        panic!("function decl lowers to assignment");
    };
    assert!(matches!(
        file.body(chunk).expr(targets[0]),
        Expr::Name(n) if n == "f"
    ));
    let Expr::Function(func_body) = file.body(chunk).expr(values[0]) else {
        panic!("function value");
    };
    let func = file.body(*func_body);
    assert_eq!(func.params.len(), 1);
    assert_eq!(file.binding(func.params[0]).name, "x");
    assert_eq!(file.binding(func.params[0]).kind, BindingKind::Param);
}

// === Desugaring: method calls ===

#[test]
fn method_call_records_receiver_and_stays_distinct() {
    let file = lowered("o:m(1, 2)");
    let chunk = file.chunk();
    let [stmt] = chunk_stmts(&file)[..] else {
        panic!("one statement");
    };
    let Stmt::ExprStmt(call) = file.body(chunk).stmt(stmt) else {
        panic!("call statement");
    };
    let Expr::MethodCall {
        receiver,
        method,
        args,
    } = file.body(chunk).expr(*call)
    else {
        panic!("method call must stay a MethodCall (receiver evaluated once)");
    };
    assert_eq!(method, "m");
    assert_eq!(args.len(), 2);
    assert!(matches!(
        file.body(chunk).expr(*receiver),
        Expr::Name(n) if n == "o"
    ));
}

// === Desugaring: field access & parens ===

#[test]
fn field_access_is_index_with_string_key() {
    let file = lowered("return a.b, a[1]");
    let chunk = file.chunk();
    let [stmt] = chunk_stmts(&file)[..] else {
        panic!("one statement");
    };
    let Stmt::Return(exprs) = file.body(chunk).stmt(stmt) else {
        panic!("return");
    };
    let Expr::Index {
        index, from_field, ..
    } = file.body(chunk).expr(exprs[0])
    else {
        panic!("a.b is an index");
    };
    assert!(from_field, "a.b must record its field-sugar origin");
    assert!(matches!(
        file.body(chunk).expr(*index),
        Expr::Literal(Literal::String(s)) if s.as_str() == Some("b")
    ));
    let Expr::Index { from_field, .. } = file.body(chunk).expr(exprs[1]) else {
        panic!("a[1] is an index");
    };
    assert!(!from_field);
}

#[test]
fn paren_around_call_becomes_truncate() {
    let file = lowered("local x = (f())");
    let chunk = file.chunk();
    let [stmt] = chunk_stmts(&file)[..] else {
        panic!("one statement");
    };
    let Stmt::Local { init, .. } = file.body(chunk).stmt(stmt) else {
        panic!("local");
    };
    let Expr::Truncate(inner) = file.body(chunk).expr(init[0]) else {
        panic!("(f()) truncates the call's multiple values to one");
    };
    assert!(matches!(file.body(chunk).expr(*inner), Expr::Call { .. }));
}

#[test]
fn paren_around_vararg_becomes_truncate() {
    let file = lowered("return (...)");
    let chunk = file.chunk();
    let [stmt] = chunk_stmts(&file)[..] else {
        panic!("one statement");
    };
    let Stmt::Return(exprs) = file.body(chunk).stmt(stmt) else {
        panic!("return");
    };
    let Expr::Truncate(inner) = file.body(chunk).expr(exprs[0]) else {
        panic!("(...) truncates");
    };
    assert!(matches!(file.body(chunk).expr(*inner), Expr::Vararg));
}

#[test]
fn single_value_parens_are_erased() {
    let file = lowered("local x = (1 + 2)");
    let chunk = file.chunk();
    let [stmt] = chunk_stmts(&file)[..] else {
        panic!("one statement");
    };
    let Stmt::Local { init, .. } = file.body(chunk).stmt(stmt) else {
        panic!("local");
    };
    assert!(
        matches!(file.body(chunk).expr(init[0]), Expr::Binary { .. }),
        "single-value parens must vanish in HIR"
    );
}

#[test]
fn nested_parens_truncate_only_once() {
    let file = lowered("local x = ((f()))");
    let chunk = file.chunk();
    let truncates = file
        .body(chunk)
        .exprs()
        .filter(|(_, e)| matches!(e, Expr::Truncate(_)))
        .count();
    assert_eq!(truncates, 1, "already-truncated values are single-value");
}

// === Desugaring: if flattening ===

#[test]
fn elseif_chain_flattens_to_branches() {
    let file = lowered("if a then x = 1 elseif b then x = 2 elseif c then x = 3 else x = 4 end");
    let chunk = file.chunk();
    let [stmt] = chunk_stmts(&file)[..] else {
        panic!("one statement");
    };
    let Stmt::If {
        branches,
        else_block,
    } = file.body(chunk).stmt(stmt)
    else {
        panic!("if");
    };
    assert_eq!(branches.len(), 3, "if + two elseifs = three branches");
    assert!(else_block.is_some());
}

// === Name resolution: shadowing ===

#[test]
fn second_local_in_same_block_is_a_new_binding() {
    let file = lowered("local x = 1 local x = 2 return x");
    let locals: Vec<_> = file
        .bindings()
        .filter(|(_, b)| b.name == "x")
        .map(|(id, _)| id)
        .collect();
    assert_eq!(locals.len(), 2, "two `local x` = two distinct bindings");
    let [res] = &resolutions_of(&file, "x")[..] else {
        panic!("one `x` reference");
    };
    assert_eq!(
        *res,
        Resolution::Local(locals[1]),
        "the later binding shadows"
    );
}

#[test]
fn local_initializer_sees_the_previous_binding() {
    // In `local x = x`, the right-hand `x` is the OUTER x (or global):
    // initializers evaluate before the new names come into scope.
    let file = lowered("local x = 1 local x = x");
    let locals: Vec<_> = file
        .bindings()
        .filter(|(_, b)| b.name == "x")
        .map(|(id, _)| id)
        .collect();
    let [res] = &resolutions_of(&file, "x")[..] else {
        panic!("one `x` reference (the initializer)");
    };
    assert_eq!(*res, Resolution::Local(locals[0]));
}

#[test]
fn unbound_names_resolve_to_global() {
    let file = lowered("print(x)");
    assert_eq!(
        resolutions_of(&file, "print"),
        [Resolution::Global("print".to_string())]
    );
    assert_eq!(
        resolutions_of(&file, "x"),
        [Resolution::Global("x".to_string())]
    );
}

#[test]
fn block_scope_ends_with_the_block() {
    let file = lowered("do local x = 1 end return x");
    let resolutions = resolutions_of(&file, "x");
    assert_eq!(
        resolutions,
        [Resolution::Global("x".to_string())],
        "a do-block local must not leak"
    );
}

// === Name resolution: upvalues ===

#[test]
fn upvalue_capture_across_nested_functions() {
    let file = lowered(
        "local a = 1\n\
         local function outer()\n\
           local b = 2\n\
           local function inner()\n\
             return a, b\n\
           end\n\
           return inner\n\
         end",
    );
    let a_binding = file
        .bindings()
        .find(|(_, b)| b.name == "a")
        .map(|(id, _)| id)
        .expect("binding a");
    let b_binding = file
        .bindings()
        .find(|(_, b)| b.name == "b")
        .map(|(id, _)| id)
        .expect("binding b");
    assert_eq!(
        resolutions_of(&file, "a"),
        [Resolution::Upvalue {
            binding: a_binding,
            depth: 2
        }],
        "chunk-level `a` is two function boundaries up from `inner`"
    );
    assert_eq!(
        resolutions_of(&file, "b"),
        [Resolution::Upvalue {
            binding: b_binding,
            depth: 1
        }],
        "`b` lives in the immediately enclosing function"
    );
}

#[test]
fn params_are_captured_as_upvalues() {
    let file = lowered("local f = function(p) return function() return p end end");
    let p_binding = file
        .bindings()
        .find(|(_, b)| b.name == "p")
        .map(|(id, _)| id)
        .expect("binding p");
    assert_eq!(
        resolutions_of(&file, "p"),
        [Resolution::Upvalue {
            binding: p_binding,
            depth: 1
        }]
    );
}

// === Name resolution: repeat-until quirk ===

#[test]
fn repeat_until_condition_sees_body_locals() {
    let file = lowered("repeat local done = f() until done");
    let done_binding = file
        .bindings()
        .find(|(_, b)| b.name == "done")
        .map(|(id, _)| id)
        .expect("binding done");
    assert_eq!(
        resolutions_of(&file, "done"),
        [Resolution::Local(done_binding)],
        "`until` shares the loop body's scope"
    );
}

#[test]
fn while_condition_does_not_see_body_locals() {
    let file = lowered("while go do local go = false end");
    let resolutions = resolutions_of(&file, "go");
    assert_eq!(
        resolutions,
        [Resolution::Global("go".to_string())],
        "`while` evaluates its condition outside the body scope"
    );
}

// === Name resolution: for loops ===

#[test]
fn numeric_for_var_is_scoped_to_the_body() {
    let file = lowered("for i = 1, 10 do print(i) end return i");
    let i_binding = file
        .bindings()
        .find(|(_, b)| b.name == "i")
        .map(|(id, _)| id)
        .expect("loop var binding");
    assert_eq!(file.binding(i_binding).kind, BindingKind::ForVar);
    assert_eq!(
        resolutions_of(&file, "i"),
        [
            Resolution::Local(i_binding),
            Resolution::Global("i".to_string()),
        ],
        "inside: the control var; after the loop: global"
    );
}

#[test]
fn numeric_for_range_exprs_evaluate_outside_the_loop_scope() {
    let file = lowered("local i = 5 for i = i, 10 do end");
    let locals: Vec<_> = file
        .bindings()
        .filter(|(_, b)| b.name == "i")
        .map(|(id, _)| id)
        .collect();
    assert_eq!(locals.len(), 2, "outer local + loop var");
    let [res] = &resolutions_of(&file, "i")[..] else {
        panic!("one `i` reference (the start expr)");
    };
    assert_eq!(
        *res,
        Resolution::Local(locals[0]),
        "`for i = i, …` reads the OUTER i for the start value"
    );
}

#[test]
fn generic_for_vars_are_fresh_and_iterator_exprs_are_outside() {
    let file = lowered("for k, v in pairs(t) do return k, v end");
    let chunk = file.chunk();
    let [stmt] = chunk_stmts(&file)[..] else {
        panic!("one statement");
    };
    let Stmt::GenericFor { vars, exprs, .. } = file.body(chunk).stmt(stmt) else {
        panic!("generic for");
    };
    assert_eq!(vars.len(), 2);
    assert_eq!(exprs.len(), 1);
    assert_eq!(file.binding(vars[0]).name, "k");
    assert_eq!(file.binding(vars[0]).kind, BindingKind::ForVar);
    assert_eq!(
        resolutions_of(&file, "k"),
        [Resolution::Local(vars[0])],
        "loop body sees the control var"
    );
    assert_eq!(
        resolutions_of(&file, "t"),
        [Resolution::Global("t".to_string())],
        "iterator exprs resolve in the enclosing scope"
    );
}

// === Name resolution: local function recursion ===

#[test]
fn local_function_sees_itself() {
    let file = lowered("local function f() return f() end");
    let f_binding = file
        .bindings()
        .find(|(_, b)| b.name == "f")
        .map(|(id, _)| id)
        .expect("binding f");
    assert_eq!(file.binding(f_binding).kind, BindingKind::LocalFunction);
    assert_eq!(
        resolutions_of(&file, "f"),
        [Resolution::Upvalue {
            binding: f_binding,
            depth: 1
        }],
        "`local function` is in scope inside its own body"
    );
}

#[test]
fn plain_local_with_function_value_does_not_see_itself() {
    let file = lowered("local g = function() return g() end");
    assert_eq!(
        resolutions_of(&file, "g"),
        [Resolution::Global("g".to_string())],
        "`local g = function…` — the initializer precedes the binding"
    );
}

// === Local attribs ===

#[test]
fn local_attribs_are_lowered() {
    let file = lowered("local a <const>, b <close>, c = 1, 2, 3");
    let chunk = file.chunk();
    let [stmt] = chunk_stmts(&file)[..] else {
        panic!("one statement");
    };
    let Stmt::Local { names, init } = file.body(chunk).stmt(stmt) else {
        panic!("local");
    };
    assert_eq!(names.len(), 3);
    assert_eq!(init.len(), 3);
    assert_eq!(names[0].attrib, Some(Attrib::Const));
    assert_eq!(names[1].attrib, Some(Attrib::Close));
    assert_eq!(names[2].attrib, None);
}

// === Tables ===

#[test]
fn table_entries_are_classified() {
    let file = lowered("t = { 1, x = 2, [k] = 3 }");
    let chunk = file.chunk();
    let (_, table) = file
        .body(chunk)
        .exprs()
        .find(|(_, e)| matches!(e, Expr::Table { .. }))
        .expect("table expr");
    let Expr::Table { entries } = table else {
        unreachable!();
    };
    assert_eq!(entries.len(), 3);
    assert!(matches!(entries[0], TableEntry::Positional(_)));
    assert!(matches!(&entries[1], TableEntry::Named { name, .. } if name == "x"));
    assert!(matches!(entries[2], TableEntry::Keyed { .. }));
}

// === Varargs ===

#[test]
fn vararg_functions_are_flagged() {
    let file = lowered("local f = function(a, ...) return a, ... end");
    let (_, func) = file
        .bodies()
        .find(|(id, _)| *id != file.chunk())
        .expect("function body");
    assert!(func.is_vararg);
    assert_eq!(func.params.len(), 1, "`...` is not a named param");
}

// === require extraction ===

#[test]
fn require_edges_from_all_literal_forms() {
    let file = lowered(
        "local a = require(\"foo\")\n\
         local b = require 'bar.baz'\n\
         local c = require [[long.mod]]\n\
         local d = require(\"a.b.c\")",
    );
    let modules: Vec<_> = file.requires().iter().map(|e| e.module.as_str()).collect();
    assert_eq!(modules, ["foo", "bar.baz", "long.mod", "a.b.c"]);
    assert!(file.dynamic_requires().is_empty());
}

#[test]
fn dynamic_require_is_flagged_separately() {
    let src = "local m = require(name) local n = require('static')";
    let file = lowered(src);
    let modules: Vec<_> = file.requires().iter().map(|e| e.module.as_str()).collect();
    assert_eq!(modules, ["static"]);
    let [dynamic] = file.dynamic_requires() else {
        panic!("one dynamic require");
    };
    let range = dynamic.range;
    assert_eq!(
        &src[usize::from(range.start())..usize::from(range.end())],
        "require(name)"
    );
}

#[test]
fn shadowed_require_is_not_an_edge() {
    let file = lowered("local require = mock require('not.a.module')");
    assert!(
        file.requires().is_empty(),
        "a local `require` is not the module loader"
    );
    assert!(file.dynamic_requires().is_empty());
}

#[test]
fn require_edge_range_covers_the_call() {
    let src = "local x = require(\"mod\")";
    let file = lowered(src);
    let [edge] = file.requires() else {
        panic!("one edge");
    };
    assert_eq!(
        &src[usize::from(edge.range.start())..usize::from(edge.range.end())],
        "require(\"mod\")"
    );
}

// === Literal decoding through lowering ===

#[test]
fn numeric_literals_decode_in_hir() {
    let file = lowered("return 0xff, 1e3, 42, 3.5");
    let chunk = file.chunk();
    let [stmt] = chunk_stmts(&file)[..] else {
        panic!("one statement");
    };
    let Stmt::Return(exprs) = file.body(chunk).stmt(stmt) else {
        panic!("return");
    };
    let numbers: Vec<_> = exprs
        .iter()
        .map(|&e| match file.body(chunk).expr(e) {
            Expr::Literal(Literal::Number(n)) => *n,
            other => panic!("expected number literal, got {other:?}"),
        })
        .collect();
    assert_eq!(
        numbers,
        [
            Number::Int(255),
            Number::Float(1000.0),
            Number::Int(42),
            Number::Float(3.5),
        ]
    );
}

#[test]
fn luajit_suffix_literals_decode_under_luajit_dialect() {
    let file = lowered_in("return 42ULL, 7LL, 2i", Dialect::LuaJit);
    let chunk = file.chunk();
    let [stmt] = chunk_stmts(&file)[..] else {
        panic!("one statement");
    };
    let Stmt::Return(exprs) = file.body(chunk).stmt(stmt) else {
        panic!("return");
    };
    let numbers: Vec<_> = exprs
        .iter()
        .map(|&e| match file.body(chunk).expr(e) {
            Expr::Literal(Literal::Number(n)) => *n,
            other => panic!("expected number literal, got {other:?}"),
        })
        .collect();
    assert_eq!(
        numbers,
        [Number::U64(42), Number::I64(7), Number::Imaginary(2.0)]
    );
}

#[test]
fn string_literals_decode_in_hir() {
    let file = lowered(r#"return "a\nb", 'plain', [[long]]"#);
    let chunk = file.chunk();
    let [stmt] = chunk_stmts(&file)[..] else {
        panic!("one statement");
    };
    let Stmt::Return(exprs) = file.body(chunk).stmt(stmt) else {
        panic!("return");
    };
    let strings: Vec<_> = exprs
        .iter()
        .map(|&e| match file.body(chunk).expr(e) {
            Expr::Literal(Literal::String(s)) => (s.as_str().map(String::from), s.is_long),
            other => panic!("expected string literal, got {other:?}"),
        })
        .collect();
    assert_eq!(
        strings,
        [
            (Some("a\nb".to_string()), false),
            (Some("plain".to_string()), false),
            (Some("long".to_string()), true),
        ]
    );
}

#[test]
fn boolean_and_nil_literals() {
    let file = lowered("return nil, true, false");
    let chunk = file.chunk();
    let [stmt] = chunk_stmts(&file)[..] else {
        panic!("one statement");
    };
    let Stmt::Return(exprs) = file.body(chunk).stmt(stmt) else {
        panic!("return");
    };
    let literals: Vec<_> = exprs
        .iter()
        .map(|&e| match file.body(chunk).expr(e) {
            Expr::Literal(l) => l.clone(),
            other => panic!("expected literal, got {other:?}"),
        })
        .collect();
    assert_eq!(
        literals,
        [Literal::Nil, Literal::Bool(true), Literal::Bool(false)]
    );
}

// === Source map ===

#[test]
fn source_map_roundtrips_exprs_to_their_text() {
    let src = "local x = 1 + 2 * 3\nprint(x)";
    let file = lowered(src);
    let chunk = file.chunk();
    let (bin_id, _) = file
        .body(chunk)
        .exprs()
        .find(|(_, e)| matches!(e, Expr::Binary { op, .. } if *op == luabox_hir::BinOp::Add))
        .expect("the + expr");
    assert_eq!(
        source_text(src, &file, HirId::expr(chunk, bin_id)),
        "1 + 2 * 3"
    );
    let (call_id, _) = file
        .body(chunk)
        .exprs()
        .find(|(_, e)| matches!(e, Expr::Call { .. }))
        .expect("the call");
    assert_eq!(
        source_text(src, &file, HirId::expr(chunk, call_id)),
        "print(x)"
    );
}

#[test]
fn source_map_covers_stmts_and_nested_bodies() {
    let src = "local function f(a)\n  return a + 1\nend";
    let file = lowered(src);
    let chunk = file.chunk();
    let [stmt] = chunk_stmts(&file)[..] else {
        panic!("one statement");
    };
    assert_eq!(source_text(src, &file, HirId::stmt(chunk, stmt)), src);
    // The return statement inside the function body maps too.
    let (func_id, func) = file
        .bodies()
        .find(|(id, _)| *id != chunk)
        .expect("function body");
    let [ret] = func.block.stmts[..] else {
        panic!("one body statement");
    };
    assert_eq!(
        source_text(src, &file, HirId::stmt(func_id, ret)),
        "return a + 1"
    );
}

#[test]
fn every_expr_and_stmt_is_source_mapped() {
    let src = "function t.ns:m(x, ...)\n\
               local y <const> = { x, k = (f()), [1] = a.b }\n\
               for i = 1, #y do y[i] = -i end\n\
               repeat y = y and y or nil until done\n\
               goto out\n\
               ::out::\n\
               return (...), t:m(1)\n\
               end";
    let file = lowered(src);
    for (body_id, body) in file.bodies() {
        for (expr_id, _) in body.exprs() {
            assert!(
                file.source_map()
                    .range(HirId::expr(body_id, expr_id))
                    .is_some(),
                "unmapped expr {expr_id:?} in {body_id:?}"
            );
        }
        for (stmt_id, _) in body.stmts() {
            assert!(
                file.source_map()
                    .range(HirId::stmt(body_id, stmt_id))
                    .is_some(),
                "unmapped stmt {stmt_id:?} in {body_id:?}"
            );
        }
    }
}

// === goto / label resolution ===

/// Every `Goto` stmt in `body`, as `(name, resolved)` pairs.
fn gotos(file: &LoweredFile, body: BodyId) -> Vec<(String, bool)> {
    fn walk(file: &LoweredFile, body: BodyId, block: &Block, out: &mut Vec<(String, bool)>) {
        for &stmt in &block.stmts {
            match file.body(body).stmt(stmt) {
                Stmt::Goto { name, target } => out.push((name.clone(), target.is_some())),
                Stmt::Do { body: b }
                | Stmt::While { body: b, .. }
                | Stmt::Repeat { body: b, .. }
                | Stmt::NumericFor { body: b, .. }
                | Stmt::GenericFor { body: b, .. } => walk(file, body, b, out),
                Stmt::If {
                    branches,
                    else_block,
                } => {
                    for branch in branches {
                        walk(file, body, &branch.block, out);
                    }
                    if let Some(e) = else_block {
                        walk(file, body, e, out);
                    }
                }
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(file, body, &file.body(body).block, &mut out);
    out
}

#[test]
fn backward_and_forward_gotos_resolve() {
    let file = lowered("::top:: goto top goto fwd ::fwd::");
    assert_eq!(
        gotos(&file, file.chunk()),
        [("top".to_string(), true), ("fwd".to_string(), true)]
    );
}

#[test]
fn label_is_visible_in_nested_blocks() {
    let file = lowered("do do goto done end end ::done::");
    assert_eq!(gotos(&file, file.chunk()), [("done".to_string(), true)]);
}

#[test]
fn label_in_nested_block_is_not_visible_outside() {
    let file = lowered("do ::inner:: end goto inner");
    assert_eq!(gotos(&file, file.chunk()), [("inner".to_string(), false)]);
}

#[test]
fn goto_resolves_to_innermost_label() {
    let src = "::l:: do ::l:: goto l end";
    let file = lowered(src);
    let chunk = file.chunk();
    // Find the goto and the inner label.
    let mut goto_target = None;
    let mut labels = Vec::new();
    for (_, stmt) in file.body(chunk).stmts() {
        match stmt {
            Stmt::Goto { target, .. } => goto_target = *target,
            Stmt::Label { label, .. } => labels.push(*label),
            _ => {}
        }
    }
    let target = goto_target.expect("goto resolved");
    // The inner `::l::` starts later in the source than the outer one.
    let target_range = file.label(target).range;
    let max_start = labels
        .iter()
        .map(|&l| file.label(l).range.start())
        .max()
        .unwrap();
    assert_eq!(
        target_range.start(),
        max_start,
        "shadowing label wins for the nested goto"
    );
}

#[test]
fn goto_does_not_cross_function_boundaries() {
    let file = lowered("::top::\nlocal f = function() goto top end");
    let (func_id, _) = file
        .bodies()
        .find(|(id, _)| *id != file.chunk())
        .expect("function body");
    assert_eq!(
        gotos(&file, func_id),
        [("top".to_string(), false)],
        "labels are function-local"
    );
}

#[test]
fn goto_without_label_is_unresolved() {
    let file = lowered("goto nowhere");
    assert_eq!(gotos(&file, file.chunk()), [("nowhere".to_string(), false)]);
}

// === Structure: bodies & boundary ===

#[test]
fn one_body_per_function_plus_chunk() {
    let file = lowered("local a = function() end function b() end return function() end");
    assert_eq!(file.bodies().count(), 4, "chunk + three functions");
    let chunk = file.body(file.chunk());
    assert!(chunk.parent.is_none());
    assert!(
        file.bodies()
            .filter(|(id, _)| *id != file.chunk())
            .all(|(_, b)| b.parent == Some(file.chunk())),
        "all three functions nest directly in the chunk"
    );
}

mod properties {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// Lowering total-functions over arbitrary input: never panics, and
        /// every name expression gets a resolution entry.
        #[test]
        fn lowering_never_panics_and_resolves_every_name(text in any::<String>()) {
            for dialect in Dialect::ALL {
                let parse = parse(&text, dialect);
                let file = luabox_hir::lower(&parse);
                for (body_id, body) in file.bodies() {
                    for (expr_id, expr) in body.exprs() {
                        if matches!(expr, Expr::Name(_)) {
                            prop_assert!(
                                file.resolution(HirId::expr(body_id, expr_id)).is_some(),
                                "unresolved name in {text:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn broken_parses_still_lower() {
    // Error resilience: lowering never panics on recovered trees.
    for src in [
        "local = 5",
        "if x then",
        "f(",
        "x = ",
        "function f( end",
        "t = {1, 2",
        "goto",
        "::",
        "local x <",
        "return 1 return 2",
    ] {
        let parse = parse(src, Dialect::Lua54);
        assert!(!parse.errors().is_empty(), "fixture should be broken");
        let _file = luabox_hir::lower(&parse);
    }
}

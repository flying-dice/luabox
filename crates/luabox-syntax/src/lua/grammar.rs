//! Recursive-descent rules for the union grammar of Lua 5.1–5.4 + LuaJIT.
//!
//! Everything the union allows parses here — `goto`/labels, `//`, bitops,
//! `<const>`/`<close>` attribs, hex floats, etc.; per-dialect legality is a
//! later validation pass over the tree.
//!
//! Recovery contract: every loop either consumes at least one token or
//! breaks, and statement-level junk is swallowed into `ERROR_NODE`s until a
//! token that can start a statement (or close a block) comes up.

#[allow(clippy::enum_glob_use)]
use super::SyntaxKind::{self, *};
use super::parser::{MAX_DEPTH, Parser};

/// `chunk ::= block` — the root; consumes the entire input.
pub(super) fn source_file(p: &mut Parser) {
    p.start_node(SOURCE_FILE);
    block(p, true);
    // Only trivia can remain; attach it to the root.
    p.bump_remaining_trivia();
    p.finish_node();
}

/// Tokens that terminate a (non-top-level) block.
fn is_block_end(kind: SyntaxKind) -> bool {
    matches!(kind, END_KW | ELSE_KW | ELSEIF_KW | UNTIL_KW)
}

/// Tokens that can begin an expression.
fn can_start_expr(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        IDENT
            | NUMBER
            | STRING
            | NIL_KW
            | TRUE_KW
            | FALSE_KW
            | DOT_DOT_DOT
            | FUNCTION_KW
            | L_PAREN
            | L_BRACE
            | NOT_KW
            | MINUS
            | HASH
            | TILDE
    )
}

/// Tokens that can begin a statement — the recovery synchronization set.
fn can_start_stmt(kind: SyntaxKind) -> bool {
    can_start_expr(kind)
        || matches!(
            kind,
            LOCAL_KW
                | IF_KW
                | WHILE_KW
                | DO_KW
                | FOR_KW
                | REPEAT_KW
                | RETURN_KW
                | BREAK_KW
                | GOTO_KW
                | COLON_COLON
                | SEMICOLON
        )
}

/// `block ::= {stat} [retstat]` — `return` is parsed as an ordinary
/// statement anywhere; a non-final position is reported but recovered.
///
/// At top level, stray block terminators (`end` etc.) are consumed into
/// `ERROR_NODE`s so the whole input always lands in one `BLOCK`.
fn block(p: &mut Parser, top_level: bool) {
    p.start_node(BLOCK);
    let mut after_return = false;
    while let Some(kind) = p.current() {
        if is_block_end(kind) {
            if !top_level {
                break;
            }
            p.error_and_bump(format!("unexpected '{}'", p.current_text()));
            continue;
        }
        if kind == SEMICOLON {
            // Statement separator (5.1) / empty statement (5.2+): a bare
            // token child of the block.
            p.bump();
            continue;
        }
        if after_return {
            p.error("statement after 'return'");
            after_return = false;
        }
        after_return |= statement(p);
    }
    p.finish_node();
}

/// Returns `true` when the parsed statement was a `return`.
fn statement(p: &mut Parser) -> bool {
    p.depth += 1;
    let is_return = if p.depth > MAX_DEPTH {
        p.depth_error();
        false
    } else {
        statement_inner(p)
    };
    p.depth -= 1;
    is_return
}

fn statement_inner(p: &mut Parser) -> bool {
    match p.current() {
        Some(LOCAL_KW) => local_stmt(p),
        Some(IF_KW) => if_stmt(p),
        Some(WHILE_KW) => while_stmt(p),
        Some(DO_KW) => do_stmt(p),
        Some(FOR_KW) => for_stmt(p),
        Some(REPEAT_KW) => repeat_stmt(p),
        Some(FUNCTION_KW) => function_decl_stmt(p),
        Some(RETURN_KW) => {
            return_stmt(p);
            return true;
        }
        Some(BREAK_KW) => {
            p.start_node(BREAK_STMT);
            p.bump();
            p.finish_node();
        }
        Some(GOTO_KW) => goto_stmt(p),
        Some(COLON_COLON) => label_stmt(p),
        Some(kind) if can_start_expr(kind) => expr_stmt(p),
        Some(_) => recover_stmt(p),
        None => {}
    }
    false
}

/// Unparseable statement start: swallow tokens into an `ERROR_NODE` until a
/// statement can start again or the enclosing block can close.
fn recover_stmt(p: &mut Parser) {
    p.error(format!("unexpected '{}'", p.current_text()));
    p.start_node(ERROR_NODE);
    while let Some(kind) = p.current() {
        if can_start_stmt(kind) || is_block_end(kind) {
            break;
        }
        p.bump();
    }
    p.finish_node();
}

/// `local function Name funcbody` | `local attnamelist ['=' explist]`.
fn local_stmt(p: &mut Parser) {
    if p.nth(1) == Some(FUNCTION_KW) {
        p.start_node(LOCAL_FUNCTION_STMT);
        p.bump(); // local
        p.bump(); // function
        p.expect(IDENT);
        function_body(p);
        p.finish_node();
        return;
    }
    p.start_node(LOCAL_STMT);
    p.bump(); // local
    loop {
        if p.at(IDENT) {
            local_name(p);
        } else {
            p.error("expected a name");
            break;
        }
        if !p.eat(COMMA) {
            break;
        }
    }
    if p.eat(EQ) {
        expr_list(p);
    }
    p.finish_node();
}

/// `Name ['<' Name '>']` (5.4 attribs; union grammar).
fn local_name(p: &mut Parser) {
    p.start_node(LOCAL_NAME);
    p.bump(); // IDENT
    if p.at(LT) {
        p.start_node(NAME_ATTRIB);
        p.bump(); // <
        p.expect(IDENT);
        p.expect(GT);
        p.finish_node();
    }
    p.finish_node();
}

/// `if exp then block {elseif exp then block} [else block] end`.
fn if_stmt(p: &mut Parser) {
    p.start_node(IF_STMT);
    p.bump(); // if
    expr(p);
    p.expect(THEN_KW);
    block(p, false);
    let mut seen_else = false;
    loop {
        match p.current() {
            Some(ELSEIF_KW) => {
                p.start_node(ELSEIF_CLAUSE);
                p.bump();
                expr(p);
                p.expect(THEN_KW);
                block(p, false);
                p.finish_node();
            }
            Some(ELSE_KW) => {
                if seen_else {
                    p.error("duplicate 'else'");
                }
                seen_else = true;
                p.start_node(ELSE_CLAUSE);
                p.bump();
                block(p, false);
                p.finish_node();
            }
            _ => break,
        }
    }
    p.expect(END_KW);
    p.finish_node();
}

/// `while exp do block end`.
fn while_stmt(p: &mut Parser) {
    p.start_node(WHILE_STMT);
    p.bump(); // while
    expr(p);
    p.expect(DO_KW);
    block(p, false);
    p.expect(END_KW);
    p.finish_node();
}

/// `do block end`.
fn do_stmt(p: &mut Parser) {
    p.start_node(DO_STMT);
    p.bump(); // do
    block(p, false);
    p.expect(END_KW);
    p.finish_node();
}

/// `repeat block until exp`.
fn repeat_stmt(p: &mut Parser) {
    p.start_node(REPEAT_STMT);
    p.bump(); // repeat
    block(p, false);
    p.expect(UNTIL_KW);
    expr(p);
    p.finish_node();
}

/// Numeric `for Name '=' exp ',' exp [',' exp] do block end` or generic
/// `for namelist in explist do block end`, split by the token after the
/// first name.
fn for_stmt(p: &mut Parser) {
    if p.nth(1) == Some(IDENT) && p.nth(2) == Some(EQ) {
        p.start_node(NUMERIC_FOR_STMT);
        p.bump(); // for
        p.bump(); // Name
        p.bump(); // =
        expr(p);
        p.expect(COMMA);
        expr(p);
        if p.eat(COMMA) {
            expr(p);
        }
    } else {
        p.start_node(GENERIC_FOR_STMT);
        p.bump(); // for
        p.expect(IDENT);
        while p.eat(COMMA) {
            p.expect(IDENT);
        }
        p.expect(IN_KW);
        expr_list(p);
    }
    p.expect(DO_KW);
    block(p, false);
    p.expect(END_KW);
    p.finish_node();
}

/// `function funcname funcbody` with `funcname ::= Name {'.' Name} [':' Name]`.
fn function_decl_stmt(p: &mut Parser) {
    p.start_node(FUNCTION_DECL_STMT);
    p.bump(); // function
    p.start_node(FUNCTION_NAME);
    p.expect(IDENT);
    while p.eat(DOT) {
        p.expect(IDENT);
    }
    if p.eat(COLON) {
        p.expect(IDENT);
    }
    p.finish_node();
    function_body(p);
    p.finish_node();
}

/// `funcbody ::= '(' [parlist] ')' block end` — inlined into the enclosing
/// function node (`PARAM_LIST` + `BLOCK` + `end` become direct children).
fn function_body(p: &mut Parser) {
    param_list(p);
    block(p, false);
    p.expect(END_KW);
}

/// `'(' [Name {',' Name} [',' '...'] | '...'] ')'`; `...`-position legality
/// is left to validation.
fn param_list(p: &mut Parser) {
    p.start_node(PARAM_LIST);
    if !p.expect(L_PAREN) {
        p.finish_node();
        return;
    }
    loop {
        match p.current() {
            Some(R_PAREN) => {
                p.bump();
                break;
            }
            Some(IDENT | DOT_DOT_DOT) => {
                p.start_node(PARAM);
                p.bump();
                p.finish_node();
                if !p.eat(COMMA) {
                    p.expect(R_PAREN);
                    break;
                }
            }
            _ => {
                p.error("expected a parameter");
                break;
            }
        }
    }
    p.finish_node();
}

/// `return [explist] [';']` — block-final position is checked by [`block`].
fn return_stmt(p: &mut Parser) {
    p.start_node(RETURN_STMT);
    p.bump(); // return
    if p.current().is_some_and(can_start_expr) {
        expr_list(p);
    }
    p.eat(SEMICOLON);
    p.finish_node();
}

/// `goto Name` (the lexer only emits `GOTO_KW` where the dialect has goto).
fn goto_stmt(p: &mut Parser) {
    p.start_node(GOTO_STMT);
    p.bump(); // goto
    p.expect(IDENT);
    p.finish_node();
}

/// `'::' Name '::'`.
fn label_stmt(p: &mut Parser) {
    p.start_node(LABEL_STMT);
    p.bump(); // ::
    p.expect(IDENT);
    p.expect(COLON_COLON);
    p.finish_node();
}

/// Expression in statement position: an assignment (`=`/`,` follows the
/// first expression) or a call statement (anything else; non-calls are
/// reported but kept in the tree).
fn expr_stmt(p: &mut Parser) {
    let checkpoint = p.checkpoint();
    let kind = expr_bp(p, 0);
    if p.at(EQ) || p.at(COMMA) {
        p.start_node_at(checkpoint, ASSIGN_STMT);
        p.start_node_at(checkpoint, EXPR_LIST);
        while p.eat(COMMA) {
            if expr_bp(p, 0).is_none() {
                p.error("expected expression");
                break;
            }
        }
        p.finish_node(); // EXPR_LIST (targets)
        p.expect(EQ);
        expr_list(p);
        p.finish_node(); // ASSIGN_STMT
        return;
    }
    p.start_node_at(checkpoint, CALL_STMT);
    if !matches!(kind, Some(CALL_EXPR | METHOD_CALL_EXPR)) {
        p.error("expected assignment or function call");
    }
    p.finish_node();
}

/// `explist ::= exp {',' exp}` — always produces an `EXPR_LIST` node, even
/// when empty after recovery.
fn expr_list(p: &mut Parser) {
    p.start_node(EXPR_LIST);
    loop {
        if expr_bp(p, 0).is_none() {
            p.error("expected expression");
            break;
        }
        if !p.eat(COMMA) {
            break;
        }
    }
    p.finish_node();
}

/// Required expression; reports when none can start here.
fn expr(p: &mut Parser) {
    if expr_bp(p, 0).is_none() {
        p.error("expected expression");
    }
}

/// Binding powers straight from the reference implementation (lparser.c):
/// `(left, right)`; right-associative operators have `right < left`.
fn bin_op_power(kind: SyntaxKind) -> Option<(u8, u8)> {
    Some(match kind {
        OR_KW => (1, 1),
        AND_KW => (2, 2),
        LT | GT | LT_EQ | GT_EQ | TILDE_EQ | EQ_EQ => (3, 3),
        PIPE => (4, 4),
        TILDE => (5, 5),
        AMP => (6, 6),
        LT_LT | GT_GT => (7, 7),
        DOT_DOT => (9, 8), // right-associative
        PLUS | MINUS => (10, 10),
        STAR | SLASH | SLASH_SLASH | PERCENT => (11, 11),
        CARET => (14, 13), // right-associative, above unary
        _ => return None,
    })
}

/// Unary `not # - ~` bind at 12: tighter than any binary operator except
/// `^`, whose left power (14) still captures `-2^2` as `-(2^2)`.
const UNARY_POWER: u8 = 12;

fn is_unary_op(kind: SyntaxKind) -> bool {
    matches!(kind, NOT_KW | HASH | MINUS | TILDE)
}

/// Precedence-climbing expression parser. Returns the kind of the completed
/// expression node, or `None` (nothing consumed, nothing reported) when no
/// expression can start here — callers decide whether that is an error.
fn expr_bp(p: &mut Parser, limit: u8) -> Option<SyntaxKind> {
    p.depth += 1;
    let result = if p.depth > MAX_DEPTH {
        p.depth_error();
        None
    } else {
        expr_bp_inner(p, limit)
    };
    p.depth -= 1;
    result
}

fn expr_bp_inner(p: &mut Parser, limit: u8) -> Option<SyntaxKind> {
    let checkpoint = p.checkpoint();
    let mut kind = if p.current().is_some_and(is_unary_op) {
        p.start_node(PREFIX_EXPR);
        p.bump();
        if expr_bp(p, UNARY_POWER).is_none() {
            p.error("expected expression");
        }
        p.finish_node();
        PREFIX_EXPR
    } else {
        simple_expr(p)?
    };
    // Once exceeded, the height budget never recovers for this marker (the
    // over-tall child stays in place), so the answer is cached.
    let mut budget_ok = true;
    while let Some((left, right)) = p.current().and_then(bin_op_power) {
        if left <= limit {
            break;
        }
        // Past the height budget the operator and operand still parse, but
        // flat into the enclosing node instead of a new `BIN_EXPR` level.
        let wrap = budget_ok && {
            budget_ok = p.can_wrap(checkpoint);
            budget_ok
        };
        if wrap {
            p.start_node_at(checkpoint, BIN_EXPR);
        }
        p.bump(); // the operator
        if expr_bp(p, right).is_none() {
            p.error("expected expression");
        }
        if wrap {
            p.finish_node();
            kind = BIN_EXPR;
        }
    }
    Some(kind)
}

/// `nil | false | true | Number | String | '...' | functiondef |
/// tableconstructor | suffixedexp`.
fn simple_expr(p: &mut Parser) -> Option<SyntaxKind> {
    match p.current()? {
        NUMBER | STRING | NIL_KW | TRUE_KW | FALSE_KW => {
            p.start_node(LITERAL_EXPR);
            p.bump();
            p.finish_node();
            Some(LITERAL_EXPR)
        }
        DOT_DOT_DOT => {
            p.start_node(VARARG_EXPR);
            p.bump();
            p.finish_node();
            Some(VARARG_EXPR)
        }
        FUNCTION_KW => {
            p.start_node(FUNCTION_EXPR);
            p.bump();
            function_body(p);
            p.finish_node();
            Some(FUNCTION_EXPR)
        }
        L_BRACE => {
            table_expr(p);
            Some(TABLE_EXPR)
        }
        IDENT | L_PAREN => suffixed_expr(p),
        _ => None,
    }
}

/// `suffixedexp ::= primaryexp {'.' Name | '[' exp ']' | ':' Name args | args}`
/// with `primaryexp ::= Name | '(' exp ')'`.
fn suffixed_expr(p: &mut Parser) -> Option<SyntaxKind> {
    let checkpoint = p.checkpoint();
    let mut kind = match p.current()? {
        IDENT => {
            p.start_node(NAME_EXPR);
            p.bump();
            p.finish_node();
            NAME_EXPR
        }
        L_PAREN => {
            p.start_node(PAREN_EXPR);
            p.bump();
            expr(p);
            p.expect(R_PAREN);
            p.finish_node();
            PAREN_EXPR
        }
        _ => return None,
    };
    // See `expr_bp_inner` for the budget caching rationale.
    let mut budget_ok = true;
    loop {
        let suffix = match p.current() {
            Some(DOT) => FIELD_EXPR,
            Some(L_BRACKET) => INDEX_EXPR,
            Some(COLON) => METHOD_CALL_EXPR,
            Some(L_PAREN | STRING | L_BRACE) => CALL_EXPR,
            _ => break,
        };
        // Past the height budget the suffix still parses, but flat into the
        // enclosing node instead of a new expression level.
        let wrap = budget_ok && {
            budget_ok = p.can_wrap(checkpoint);
            budget_ok
        };
        if wrap {
            p.start_node_at(checkpoint, suffix);
        }
        match suffix {
            FIELD_EXPR => {
                p.bump(); // .
                p.expect(IDENT);
            }
            INDEX_EXPR => {
                p.bump(); // [
                expr(p);
                p.expect(R_BRACKET);
            }
            METHOD_CALL_EXPR => {
                p.bump(); // :
                p.expect(IDENT);
                call_args(p);
            }
            _ => call_args(p),
        }
        if wrap {
            p.finish_node();
            kind = suffix;
        }
    }
    Some(kind)
}

/// `args ::= '(' [explist] ')' | tableconstructor | String`.
fn call_args(p: &mut Parser) {
    p.start_node(ARG_LIST);
    match p.current() {
        Some(STRING) => p.bump(),
        Some(L_BRACE) => table_expr(p),
        Some(L_PAREN) => {
            p.bump();
            if p.current().is_some_and(can_start_expr) {
                expr_list(p);
            }
            p.expect(R_PAREN);
        }
        _ => p.error("expected arguments"),
    }
    p.finish_node();
}

/// `tableconstructor ::= '{' [fieldlist] '}'`; tolerant of stray separators
/// and junk between fields.
fn table_expr(p: &mut Parser) {
    p.start_node(TABLE_EXPR);
    p.bump(); // {
    loop {
        match p.current() {
            None => {
                p.error("expected '}'");
                break;
            }
            Some(R_BRACE) => {
                p.bump();
                break;
            }
            Some(COMMA | SEMICOLON) => p.bump(),
            Some(L_BRACKET) => table_key_field(p),
            Some(IDENT) if p.nth(1) == Some(EQ) => {
                p.start_node(TABLE_NAME_FIELD);
                p.bump(); // Name
                p.bump(); // =
                expr(p);
                p.finish_node();
            }
            Some(kind) if can_start_expr(kind) => {
                p.start_node(TABLE_ITEM_FIELD);
                expr(p);
                p.finish_node();
            }
            Some(kind) if is_block_end(kind) || can_start_stmt(kind) => {
                // Likely an unclosed table: hand the token back to the
                // enclosing statement recovery.
                p.error("expected '}'");
                break;
            }
            Some(_) => p.error_and_bump(format!("unexpected '{}'", p.current_text())),
        }
    }
    p.finish_node();
}

/// `'[' exp ']' '=' exp`.
fn table_key_field(p: &mut Parser) {
    p.start_node(TABLE_KEY_FIELD);
    p.bump(); // [
    expr(p);
    p.expect(R_BRACKET);
    p.expect(EQ);
    expr(p);
    p.finish_node();
}

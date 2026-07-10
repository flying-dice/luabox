//! Minimal `.luab` shape-file support: parse diagnostics (see
//! [`crate::diagnostics::lb_diagnostics`]), plus same-file hover and goto
//! definition for type names.
//!
//! `.luab` files never enter the Lua analysis host — they are parsed directly
//! with [`shape::parse`] from the text the server tracks (overlay over disk).

use luabox_syntax::shape::{
    self, ShapeSyntaxKind, ShapeSyntaxNode, ShapeSyntaxToken,
    ast::{AstNode, ShapeFile},
};
use rowan::{TextRange, TextSize, TokenAtOffset};

/// The declaration of the type named under the cursor:
/// `(name token range of the declaration, the declaration's source text)`.
#[must_use]
pub fn definition(text: &str, offset: usize) -> Option<(TextRange, String)> {
    let parse = shape::parse(text);
    let root = parse.syntax();
    let token = ident_at(&root, offset)?;
    let name = token.text();

    let file = ShapeFile::cast(root)?;
    for item in file.items() {
        if item.name().as_deref() == Some(name) {
            let node = item.syntax();
            let name_token = first_ident(node)?;
            return Some((name_token.text_range(), node.text().to_string()));
        }
    }
    None
}

fn ident_at(root: &ShapeSyntaxNode, offset: usize) -> Option<ShapeSyntaxToken> {
    let offset = TextSize::new(u32::try_from(offset).ok()?);
    if offset > root.text_range().end() {
        return None;
    }
    let pick = |t: ShapeSyntaxToken| (t.kind() == ShapeSyntaxKind::IDENT).then_some(t);
    match root.token_at_offset(offset) {
        TokenAtOffset::None => None,
        TokenAtOffset::Single(t) => pick(t),
        TokenAtOffset::Between(l, r) => pick(l).or_else(|| pick(r)),
    }
}

fn first_ident(node: &ShapeSyntaxNode) -> Option<ShapeSyntaxToken> {
    node.children_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
        .find(|t| t.kind() == ShapeSyntaxKind::IDENT)
}

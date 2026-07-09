---
name: idiomatic-rust
description: Write idiomatic, ownership-clean Rust. ALWAYS use this skill when writing or editing Rust — any .rs file, anything under crates/, Cargo.toml/Cargo manifests, or when the user asks to add a function, type, trait, test, or module in Rust. Grounds the model in established Rust idioms: error handling, ownership/borrowing, iterators, the type system, API design, and the clippy/rustfmt baseline. Trigger even when the user doesn't say "idiomatic" — any Rust authoring task qualifies.
---

# idiomatic-rust

Write Rust the way the ecosystem writes it. The goal: code that a seasoned Rust dev reads without flinching — ownership that doesn't fight the borrow checker, errors that propagate cleanly, types that make illegal states unrepresentable, and zero `clippy` warnings.

These are defaults, not dogma. Break one when you can name the reason. When you can't, follow it.

## The baseline (non-negotiable)

- **`rustfmt` formats; you don't.** Never hand-align or hand-wrap. Assume `cargo fmt` runs.
- **`clippy` is the linter, not your taste.** Write to pass `cargo clippy -- -D warnings`. If a lint fires, fix the code, don't `#[allow]` it — unless you can justify the allow in a comment.
- **No `unwrap()` / `expect()` / `panic!` in library code.** They're acceptable in `main`, tests, and prototypes, or when a panic is the *correct* semantic (invariant truly cannot fail — then `expect("why this can't fail")` documents the invariant). Everywhere else, return `Result`.
- **No `unsafe` unless there is no safe alternative.** When unavoidable, wrap it in a minimal safe abstraction and write a `// SAFETY:` comment stating the invariant that makes it sound.

## Error handling

This is where most non-idiomatic Rust shows. Get it right.

- **Propagate with `?`, don't `match` to rewrap.** `let x = thing()?;` not a `match` that returns the `Err` arm unchanged.
- **Libraries define error *types*; binaries use `anyhow`.**
    - In a **library**, define a concrete error enum with [`thiserror`](https://docs.rs/thiserror). Each variant carries context; `#[from]` enables `?` across error types. Callers can match on your variants.
    - In a **binary / application**, use [`anyhow::Result`](https://docs.rs/anyhow) and `.context("doing X")` / `.with_context(|| ...)` to attach a breadcrumb at each `?`. Don't make callers match — they won't.
- **Never stringly-type errors** (`Result<T, String>`) in code that outlives a prototype. It throws away the ability to match and the source chain.
- **`Option` is not an error.** Use `Option<T>` for "absent is normal", `Result<T, E>` for "this failed". Convert with `.ok_or(err)?` / `.ok_or_else(|| ...)?`.

```rust
// library error
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("unexpected end of input")]
    UnexpectedEof,
    #[error("invalid token at byte {offset}")]
    InvalidToken { offset: usize },
    #[error("io error")]
    Io(#[from] std::io::Error),
}
```

## Ownership & borrowing

- **Borrow in arguments, own in returns** (usually). Take `&str` / `&[T]` / `&T`, not `String` / `Vec<T>` / `T`, unless you need ownership. Return owned values.
- **Accept the most general type.** `&str` over `&String`, `&[T]` over `&Vec<T>`. For "string-like or path-like" params, take `impl AsRef<str>` / `impl AsRef<Path>` or `impl Into<String>` when you'll store it.
- **Don't `.clone()` to dodge the borrow checker.** A clone to silence E0502 is a smell. Restructure: narrow the borrow's scope, split the borrow, take an index, or use `Rc`/`Arc` deliberately. A clone is fine when it's genuinely the cheapest correct thing — just mean it.
- **`Cow<'_, str>`** when a value is *usually* borrowed but *sometimes* must be owned (e.g. escape/normalize only when needed).
- **Lifetimes: elide first.** Only write explicit lifetimes when the compiler demands them. Don't decorate.

## Prefer iterators over index loops

Idiomatic Rust expresses transformations as iterator chains, not manual `for i in 0..n` with indexing.

```rust
// not idiomatic
let mut out = Vec::new();
for i in 0..items.len() {
    if items[i].active { out.push(items[i].id); }
}

// idiomatic
let out: Vec<_> = items.iter().filter(|i| i.active).map(|i| i.id).collect();
```

- Reach for `map`, `filter`, `filter_map`, `flat_map`, `fold`, `any`, `all`, `find`, `position`, `enumerate`, `zip`, `take`/`skip`, `chain`, `collect`.
- `collect::<Result<Vec<_>, _>>()` to turn an iterator of `Result`s into a `Result<Vec>` — short-circuits on first error.
- Keep an explicit `for` loop when the body has side effects, early returns, `?`, or `break`/`continue` logic that a chain would obscure. Readability wins; don't force a one-liner.

## Use the type system to make illegal states unrepresentable

- **Newtypes over primitives.** `struct UserId(u64)` not bare `u64`; `struct Celsius(f64)`. Prevents arg-swap bugs and gives a home for methods.
- **Enums over boolean soup / sentinel values.** Model states as an enum; let the compiler force exhaustive `match`. No `-1 means missing`.
- **`match` exhaustively, no catch-all `_` on your own enums** when you'd rather get a compile error the day you add a variant. Use `_` only for genuinely open/foreign enums.
- **Type-state / builder** for "must configure before use" APIs: encode the stage in the type so misuse won't compile.
- Prefer `&[T]` slices and `impl Iterator` over committing to a concrete container in signatures.

## API & trait conventions

- **Implement `From`, not `Into`** — you get `Into` free. Implement `TryFrom` for fallible conversions.
- **Derive liberally:** `#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]` where they make sense. `Debug` on nearly everything public.
- **`Default` instead of a `new()` with no args**, or both (`new()` calls `Default`).
- **Builder pattern** for structs with many optional fields, rather than a telescoping `new(a, b, c, d, ...)`.
- **Return `impl Trait`** (`impl Iterator<Item = T>`, `impl Fn(...)`) to hide concrete types and avoid boxing; box (`Box<dyn Trait>`) only when you need a single named type or heterogeneous storage.
- **Naming (RFC 430):** `snake_case` fns/vars, `CamelCase` types/traits, `SCREAMING_SNAKE_CASE` consts. Conversions: `as_` (cheap ref→ref), `to_` (expensive, owned), `into_` (consuming). Getters are `field()` not `get_field()`.
- **Module privacy:** start private, expose with `pub` deliberately. Use `pub(crate)` for internal-but-cross-module.

## Common smells → fixes

| Smell | Idiomatic |
| --- | --- |
| `match opt { Some(x) => x, None => return Err(..) }` | `opt.ok_or(..)?` |
| `if x.is_some() { x.unwrap() }` | `if let Some(v) = x` |
| `.len() == 0` | `.is_empty()` |
| `v.iter().map(...).collect::<Vec<_>>().iter()` | chain without the intermediate `collect` |
| `return x;` as last expr | `x` (tail expression, no `return`) |
| `String` param you only read | `&str` |
| `vec.get(0)` | `vec.first()` |
| manual `Drop` to free Rust memory | let it drop; `Drop` is for external resources |
| `&Vec<T>` / `&String` params | `&[T]` / `&str` |
| nested `match` on `Result`/`Option` | combinators (`map`, `and_then`, `?`) |

## Tests & docs

- Unit tests in a `#[cfg(test)] mod tests` at the bottom of the file; integration tests in `tests/`.
- Public items get a `///` doc comment with a `# Examples` block where it helps — doc examples are compiled and run.
- Use `#[should_panic]`, `#[ignore]` deliberately; prefer asserting on `Result` with `?` in test fns (`fn x() -> anyhow::Result<()>`).

## Procedure when writing Rust here

1. **Match the surrounding code.** Read neighboring files first — error strategy (`thiserror` vs `anyhow`), naming, module layout. Consistency beats these defaults.
2. **Check `Cargo.toml`** for which crates are already in play (don't add `anyhow` to a no-dep crate; don't reinvent what a present dep does).
3. Write to the idioms above.
4. **Verify**: `cargo fmt`, then `cargo clippy --all-targets -- -D warnings`, then `cargo test`. Fix what they flag before reporting done.
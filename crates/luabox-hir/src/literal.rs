//! Decoding of numeric and string literal *tokens* into HIR literal values.
//!
//! The parser hands us the raw source text of each literal; the HIR stores the
//! decoded value so downstream inference and lowering never re-tokenize.
//!
//! Numbers cover every dialect form: decimal/hex integers, decimal and hex
//! (`0x1p4`) floats, and the LuaJIT 64-bit box / imaginary suffixes
//! (`LL`/`ULL`/`i`). Strings decode short-string escapes and long-bracket
//! (`[[…]]`, `[==[…]==]`) content; anything with an escape outside the union
//! of dialect escape sets yields `value: None` (callers fall back to the raw
//! source range).

/// A decoded numeric literal.
///
/// Integer vs float is a semantic distinction from Lua 5.3 on; the LuaJIT
/// variants preserve the cdata box / complex forms so lowering can reproduce
/// them exactly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Number {
    /// A plain integer (decimal or hex) that fits `i64`. Hex integers wrap
    /// modulo 2^64 into the signed range, matching Lua 5.3+.
    Int(i64),
    /// A floating-point literal (`3.14`, `1e3`, `0x1.8p3`), or a decimal
    /// integer too large for `i64` (Lua promotes those to float).
    Float(f64),
    /// LuaJIT `LL` — a signed 64-bit cdata box literal.
    I64(i64),
    /// LuaJIT `ULL` — an unsigned 64-bit cdata box literal.
    U64(u64),
    /// LuaJIT `i` — an imaginary (complex) literal.
    Imaginary(f64),
}

/// A decoded string literal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LitStr {
    /// The decoded byte value, or `None` when an escape fell outside the
    /// recognized set (the raw source, via the source map, is authoritative
    /// then).
    pub value: Option<Vec<u8>>,
    /// Whether this came from a long-bracket literal (`[[…]]`).
    pub is_long: bool,
}

impl LitStr {
    /// The decoded value as UTF-8, when it decoded and is valid UTF-8.
    pub fn as_str(&self) -> Option<&str> {
        self.value
            .as_deref()
            .and_then(|b| std::str::from_utf8(b).ok())
    }
}

/// Parse a `NUMBER` token's text into a [`Number`].
///
/// Best-effort: the lexer only emits well-formed number tokens, but on any
/// residual oddity this degrades to `Int(0)` rather than panicking.
#[must_use]
pub fn parse_number(text: &str) -> Number {
    let lower = text.trim().to_ascii_lowercase();

    // LuaJIT suffixes first (they never appear on decimal floats).
    if let Some(core) = strip_suffix_ci(&lower, "ull").or_else(|| strip_suffix_ci(&lower, "llu")) {
        return Number::U64(parse_uint(core));
    }
    if let Some(core) = strip_suffix_ci(&lower, "ll") {
        return Number::I64(parse_uint(core).cast_signed());
    }
    if let Some(core) = lower.strip_suffix('i') {
        return Number::Imaginary(parse_float(core));
    }

    if let Some(hex) = lower.strip_prefix("0x") {
        if hex.contains('.') || hex.contains('p') {
            return Number::Float(parse_hex_float(hex));
        }
        return match u64::from_str_radix(hex, 16) {
            Ok(v) => Number::Int(v.cast_signed()),
            // Overflows 64 bits: fall back to a (lossy) float.
            Err(_) => Number::Float(parse_hex_float(hex)),
        };
    }

    if lower.contains('.') || lower.contains('e') {
        return Number::Float(parse_float(&lower));
    }

    match lower.parse::<i64>() {
        Ok(v) => Number::Int(v),
        // Too large for i64: Lua promotes an over-large decimal to a float.
        Err(_) => Number::Float(parse_float(&lower)),
    }
}

/// Value of a hex or decimal integer literal as `u64` (wrapping on overflow).
fn parse_uint(core: &str) -> u64 {
    if let Some(hex) = core.strip_prefix("0x") {
        u64::from_str_radix(hex, 16).unwrap_or(0)
    } else {
        core.parse::<u64>().unwrap_or(0)
    }
}

fn parse_float(core: &str) -> f64 {
    core.parse::<f64>().unwrap_or(f64::NAN)
}

/// Case-insensitive suffix strip (the input is already lowercased, so this is
/// a plain `strip_suffix`, kept as a helper for readability).
fn strip_suffix_ci<'a>(s: &'a str, suffix: &str) -> Option<&'a str> {
    s.strip_suffix(suffix)
}

/// Parse a hex-float mantissa/exponent (`hex` is the text after `0x`).
///
/// `std` cannot parse C99 hex floats, so we accumulate the mantissa by hand:
/// integer digits base-16, fractional digits scaled by 1/16^k, then a binary
/// `p`-exponent.
#[allow(
    clippy::cast_precision_loss,
    reason = "hex-float digits are < 16; the running value is the intended f64"
)]
fn parse_hex_float(hex: &str) -> f64 {
    let (mantissa, exp) = match hex.split_once('p') {
        Some((m, e)) => (m, e.parse::<i32>().unwrap_or(0)),
        None => (hex, 0),
    };
    let (int_part, frac_part) = match mantissa.split_once('.') {
        Some((i, f)) => (i, f),
        None => (mantissa, ""),
    };
    let mut value = 0.0_f64;
    for c in int_part.chars() {
        if let Some(d) = c.to_digit(16) {
            value = value * 16.0 + f64::from(d);
        }
    }
    let mut scale = 1.0 / 16.0;
    for c in frac_part.chars() {
        if let Some(d) = c.to_digit(16) {
            value += f64::from(d) * scale;
            scale /= 16.0;
        }
    }
    value * 2.0_f64.powi(exp)
}

/// Decode a `STRING` token's text into a [`LitStr`].
#[must_use]
pub fn decode_string(text: &str) -> LitStr {
    match text.as_bytes().first() {
        Some(b'\'' | b'"') => LitStr {
            value: decode_short_string(text.as_bytes()),
            is_long: false,
        },
        Some(b'[') => LitStr {
            value: decode_long_string(text.as_bytes()),
            is_long: true,
        },
        _ => LitStr {
            value: None,
            is_long: false,
        },
    }
}

/// Decode a long-bracket string body (`[[…]]` / `[=*[…]=*]`).
///
/// Long strings have no escapes; a single leading newline directly after the
/// opening bracket is dropped (Lua rule).
fn decode_long_string(bytes: &[u8]) -> Option<Vec<u8>> {
    // Opening: `[` `=`* `[`.
    let mut level = 0;
    while bytes.get(1 + level) == Some(&b'=') {
        level += 1;
    }
    if bytes.get(1 + level) != Some(&b'[') {
        return None;
    }
    let open = 2 + level;
    let close = open;
    if bytes.len() < open + close {
        return None;
    }
    let mut content = &bytes[open..bytes.len() - close];
    // Drop one leading newline (`\n`, `\r`, `\r\n`, or `\n\r`).
    if let Some((&first, rest)) = content.split_first()
        && (first == b'\n' || first == b'\r')
    {
        content = match rest.split_first() {
            Some((&second, rest2)) if (second == b'\n' || second == b'\r') && second != first => {
                rest2
            }
            _ => rest,
        };
    }
    Some(content.to_vec())
}

/// Decode a short-string body, resolving escapes. Returns `None` on any escape
/// outside the recognized union (the caller then keeps the raw text).
fn decode_short_string(bytes: &[u8]) -> Option<Vec<u8>> {
    let quote = *bytes.first()?;
    if bytes.len() < 2 || bytes[bytes.len() - 1] != quote {
        return None;
    }
    let inner = &bytes[1..bytes.len() - 1];
    let mut out = Vec::with_capacity(inner.len());
    let mut i = 0;
    while i < inner.len() {
        let b = inner[i];
        if b != b'\\' {
            out.push(b);
            i += 1;
            continue;
        }
        i += 1;
        let e = *inner.get(i)?;
        i += 1;
        match e {
            b'a' => out.push(7),
            b'b' => out.push(8),
            b'f' => out.push(12),
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'v' => out.push(11),
            b'\\' | b'"' | b'\'' => out.push(e),
            b'\n' | b'\r' => {
                out.push(b'\n');
                if inner
                    .get(i)
                    .is_some_and(|&n| (n == b'\n' || n == b'\r') && n != e)
                {
                    i += 1;
                }
            }
            b'x' => {
                let hi = hex_digit(*inner.get(i)?)?;
                let lo = hex_digit(*inner.get(i + 1)?)?;
                out.push(hi * 16 + lo);
                i += 2;
            }
            b'0'..=b'9' => {
                let mut value: u32 = u32::from(e - b'0');
                for _ in 0..2 {
                    match inner.get(i) {
                        Some(&d) if d.is_ascii_digit() => {
                            value = value * 10 + u32::from(d - b'0');
                            i += 1;
                        }
                        _ => break,
                    }
                }
                out.push(u8::try_from(value).ok()?);
            }
            b'z' => {
                while inner
                    .get(i)
                    .is_some_and(|&w| matches!(w, b' ' | b'\t' | b'\r' | b'\n' | 0x0B | 0x0C))
                {
                    i += 1;
                }
            }
            b'u' => {
                if *inner.get(i)? != b'{' {
                    return None;
                }
                i += 1;
                let mut value: u32 = 0;
                let mut digits = 0;
                while let Some(&d) = inner.get(i) {
                    if d == b'}' {
                        break;
                    }
                    value = value
                        .checked_mul(16)?
                        .checked_add(u32::from(hex_digit(d)?))?;
                    digits += 1;
                    i += 1;
                }
                if digits == 0 || inner.get(i) != Some(&b'}') {
                    return None;
                }
                i += 1;
                let c = char::from_u32(value)?;
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
            _ => return None,
        }
    }
    Some(out)
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_integer() {
        assert_eq!(parse_number("0"), Number::Int(0));
        assert_eq!(parse_number("42"), Number::Int(42));
    }

    #[test]
    fn hex_integer() {
        assert_eq!(parse_number("0x1F"), Number::Int(0x1F));
        assert_eq!(parse_number("0xff"), Number::Int(255));
        // Wraps into the signed range like Lua 5.3+.
        assert_eq!(parse_number("0xFFFFFFFFFFFFFFFF"), Number::Int(-1));
    }

    #[test]
    fn decimal_float() {
        assert_eq!(parse_number("1e3"), Number::Float(1000.0));
        assert_eq!(parse_number("3.5"), Number::Float(3.5));
        assert_eq!(parse_number("2.5E-1"), Number::Float(0.25));
    }

    #[test]
    fn hex_float() {
        assert_eq!(parse_number("0x1p4"), Number::Float(16.0));
        assert_eq!(parse_number("0x1.8p1"), Number::Float(3.0));
    }

    #[test]
    fn luajit_suffixes() {
        assert_eq!(parse_number("42LL"), Number::I64(42));
        assert_eq!(parse_number("42ULL"), Number::U64(42));
        assert_eq!(parse_number("0xffULL"), Number::U64(255));
        assert_eq!(parse_number("10i"), Number::Imaginary(10.0));
    }

    #[test]
    fn over_large_decimal_becomes_float() {
        let Number::Float(f) = parse_number("99999999999999999999999") else {
            panic!("expected float");
        };
        assert!(f > 1e22);
    }

    #[test]
    fn short_string_escapes() {
        assert_eq!(
            decode_string(r#""a\n\t\98""#).value.unwrap(),
            b"a\n\tb".to_vec()
        );
        assert_eq!(decode_string(r#""plain""#).as_str(), Some("plain"));
    }

    #[test]
    fn long_string_strips_leading_newline() {
        assert_eq!(decode_string("[[\nhi]]").as_str(), Some("hi"));
        assert_eq!(decode_string("[==[a]b]==]").as_str(), Some("a]b"));
        assert_eq!(decode_string("[[plain]]").as_str(), Some("plain"));
    }

    #[test]
    fn unknown_escape_is_undecodable() {
        assert_eq!(decode_string(r"'\q'").value, None);
    }
}

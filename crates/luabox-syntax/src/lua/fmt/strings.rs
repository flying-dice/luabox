//! Short-string quote normalization and decoding.
//!
//! Normalization is conservative by construction: it only ever swaps the
//! delimiters and unescapes the now-unneeded quote escape — both provably
//! value-preserving. Anything less clear-cut (the preferred quote appearing
//! in the content, long strings, malformed literals) passes through
//! untouched. The decoder exists so the [`super::format`] safety check can
//! verify value preservation *independently* of the normalizer.

use super::Quotes;

/// Normalize the delimiters of a short-string literal per `style`.
///
/// Long strings (`[[…]]`, any level) and strings already using the preferred
/// quote pass through untouched. A string whose content contains the
/// preferred quote (raw or escaped) keeps its original delimiters, so no
/// escape ever needs to be *added*.
#[expect(
    clippy::string_slice,
    reason = "text is verified to open and close with a single-byte ASCII quote (len >= 2), so index 1 and len-1 are char boundaries"
)]
pub(super) fn normalize_quotes(text: &str, style: Quotes) -> Option<String> {
    let (preferred, other) = match style {
        Quotes::AutoPreferDouble => ('"', '\''),
        Quotes::AutoPreferSingle => ('\'', '"'),
    };
    let mut chars = text.chars();
    if chars.next() != Some(other) {
        return None; // long string, or already the preferred quote
    }
    if !text.ends_with(other) || text.len() < 2 {
        return None; // unterminated (parse already failed upstream)
    }
    let inner = &text[1..text.len() - 1];

    let mut out = String::with_capacity(text.len());
    out.push(preferred);
    let mut it = inner.chars();
    while let Some(c) = it.next() {
        if c == preferred {
            return None; // content contains the preferred quote: keep as-is
        }
        if c == '\\' {
            match it.next() {
                None => return None, // malformed trailing backslash
                Some(e) if e == preferred => return None,
                Some(e) if e == other => out.push(other), // unescape
                Some(e) => {
                    out.push('\\');
                    out.push(e);
                }
            }
        } else {
            out.push(c);
        }
    }
    out.push(preferred);
    Some(out)
}

/// Decode a short-string literal to its runtime byte value.
///
/// Returns `None` for long strings and for any escape outside the union of
/// the dialects' escape sets — callers must then require exact text equality.
pub(super) fn decode_short_string(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    let quote = *bytes.first()?;
    if quote != b'\'' && quote != b'"' {
        return None;
    }
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
            // Escaped real line break: `\<newline>` embeds a newline;
            // `\r\n` / `\n\r` pairs count as one.
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
                if digits == 0 || *inner.get(i)? != b'}' {
                    return None;
                }
                i += 1;
                // Stick to Unicode scalar values; Lua's extended range
                // (up to 2^31) is out of scope for the conservative check.
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
    fn prefers_double_quotes() {
        assert_eq!(
            normalize_quotes("'hi'", Quotes::AutoPreferDouble),
            Some("\"hi\"".to_string())
        );
    }

    #[test]
    fn unescapes_single_quote_when_converting() {
        assert_eq!(
            normalize_quotes(r"'it\'s'", Quotes::AutoPreferDouble),
            Some(r#""it's""#.to_string())
        );
    }

    #[test]
    fn keeps_single_when_content_has_double_quote() {
        assert_eq!(
            normalize_quotes(r#"'say "hi"'"#, Quotes::AutoPreferDouble),
            None
        );
        assert_eq!(
            normalize_quotes(r#"'say \"hi\"'"#, Quotes::AutoPreferDouble),
            None
        );
    }

    #[test]
    fn passes_other_escapes_through() {
        assert_eq!(
            normalize_quotes(r"'a\nb\116'", Quotes::AutoPreferDouble),
            Some(r#""a\nb\116""#.to_string())
        );
    }

    #[test]
    fn leaves_long_strings_and_preferred_quotes_alone() {
        assert_eq!(normalize_quotes("[[x]]", Quotes::AutoPreferDouble), None);
        assert_eq!(normalize_quotes("[=[x]=]", Quotes::AutoPreferDouble), None);
        assert_eq!(normalize_quotes("\"x\"", Quotes::AutoPreferDouble), None);
    }

    #[test]
    fn prefer_single_is_symmetric() {
        assert_eq!(
            normalize_quotes("\"hi\"", Quotes::AutoPreferSingle),
            Some("'hi'".to_string())
        );
        assert_eq!(normalize_quotes("'hi'", Quotes::AutoPreferSingle), None);
        assert_eq!(
            normalize_quotes(r#""it's""#, Quotes::AutoPreferSingle),
            None
        );
    }

    #[test]
    fn decode_handles_standard_escapes() {
        assert_eq!(
            decode_short_string(r#""a\n\t\\\"\'\98""#).unwrap(),
            b"a\n\t\\\"'b".to_vec()
        );
        assert_eq!(decode_short_string(r#""\x41\u{1F600}""#).unwrap(), {
            let mut v = b"A".to_vec();
            v.extend_from_slice("\u{1F600}".as_bytes());
            v
        });
        assert_eq!(
            decode_short_string("\"a\\z \t b\"").unwrap(),
            b"ab".to_vec()
        );
    }

    #[test]
    fn decode_rejects_unknown_escapes_and_long_strings() {
        assert_eq!(decode_short_string(r"'\q'"), None);
        assert_eq!(decode_short_string("[[x]]"), None);
        assert_eq!(decode_short_string(r"'\256'"), None);
    }

    #[test]
    fn normalized_strings_decode_identically() {
        for src in [r"'it\'s'", "'plain'", r"'tab\there'", r"'\65\x42'"] {
            let converted = normalize_quotes(src, Quotes::AutoPreferDouble).unwrap();
            assert_eq!(
                decode_short_string(src).unwrap(),
                decode_short_string(&converted).unwrap(),
                "value changed for {src}"
            );
        }
    }
}

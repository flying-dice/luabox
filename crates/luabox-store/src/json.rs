//! A tiny, dependency-free JSON codec for on-disk package manifests.
//!
//! The store persists tree manifests as `.json` (SPEC.md §6 layout), but the
//! Distribution context is deliberately dependency-light — `serde_json` is not
//! wired here. This module implements exactly the JSON subset the manifest
//! schema needs: objects, arrays, strings, booleans, `null`, and **unsigned
//! integer** numbers (no floats, no negatives — manifests only carry sizes and
//! a schema version). Anything outside that subset is a parse error.
//!
//! It is `pub(crate)`: an implementation detail, never part of the boundary
//! contract.

use std::fmt::Write as _;

/// A parsed JSON value from the manifest subset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Json {
    Null,
    Bool(bool),
    Num(u64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    /// Borrow the string payload, if this is a string.
    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    /// The integer payload, if this is a number.
    pub(crate) fn as_u64(&self) -> Option<u64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }

    /// The boolean payload, if this is a bool.
    pub(crate) fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// The array payload, if this is an array.
    pub(crate) fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(v) => Some(v),
            _ => None,
        }
    }

    /// Look up a key, if this is an object.
    pub(crate) fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Render as compact JSON. Object key order is preserved, which — combined
    /// with the manifest's own deterministic entry ordering — makes the output
    /// byte-for-byte reproducible.
    pub(crate) fn to_json_string(&self) -> String {
        let mut out = String::new();
        self.write_into(&mut out);
        out
    }

    fn write_into(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(true) => out.push_str("true"),
            Json::Bool(false) => out.push_str("false"),
            Json::Num(n) => {
                let _ = write!(out, "{n}");
            }
            Json::Str(s) => write_escaped(out, s),
            Json::Arr(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write_into(out);
                }
                out.push(']');
            }
            Json::Obj(fields) => {
                out.push('{');
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_escaped(out, k);
                    out.push(':');
                    v.write_into(out);
                }
                out.push('}');
            }
        }
    }
}

/// Escape a string into a quoted JSON string literal.
fn write_escaped(out: &mut String, s: &str) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Parse a JSON document from the supported subset.
///
/// # Errors
/// Returns a message describing the first syntax error, or trailing junk after
/// the top-level value.
pub(crate) fn parse(input: &str) -> Result<Json, String> {
    let mut p = Parser {
        bytes: input.as_bytes(),
        pos: 0,
    };
    p.skip_ws();
    let value = p.value()?;
    p.skip_ws();
    if p.pos != p.bytes.len() {
        return Err(format!("trailing data at byte {}", p.pos));
    }
    Ok(value)
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, byte: u8) -> Result<(), String> {
        if self.peek() == Some(byte) {
            self.pos += 1;
            Ok(())
        } else {
            Err(format!("expected '{}' at byte {}", byte as char, self.pos))
        }
    }

    fn value(&mut self) -> Result<Json, String> {
        self.skip_ws();
        match self.peek() {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't' | b'f') => self.boolean(),
            Some(b'n') => self.null(),
            Some(c) if c.is_ascii_digit() => self.number(),
            _ => Err(format!("unexpected token at byte {}", self.pos)),
        }
    }

    fn object(&mut self) -> Result<Json, String> {
        self.expect(b'{')?;
        let mut fields = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Json::Obj(fields));
        }
        loop {
            self.skip_ws();
            let key = self.string()?;
            self.skip_ws();
            self.expect(b':')?;
            let val = self.value()?;
            fields.push((key, val));
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Json::Obj(fields));
                }
                _ => return Err(format!("expected ',' or '}}' at byte {}", self.pos)),
            }
        }
    }

    fn array(&mut self) -> Result<Json, String> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Json::Arr(items));
        }
        loop {
            let val = self.value()?;
            items.push(val);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Json::Arr(items));
                }
                _ => return Err(format!("expected ',' or ']' at byte {}", self.pos)),
            }
        }
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            let byte = self
                .peek()
                .ok_or_else(|| "unterminated string".to_string())?;
            self.pos += 1;
            match byte {
                b'"' => return Ok(out),
                b'\\' => {
                    let esc = self
                        .peek()
                        .ok_or_else(|| "unterminated escape".to_string())?;
                    self.pos += 1;
                    match esc {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'b' => out.push('\u{0008}'),
                        b'f' => out.push('\u{000c}'),
                        b'u' => out.push(self.unicode_escape()?),
                        other => {
                            return Err(format!("invalid escape '\\{}'", other as char));
                        }
                    }
                }
                // Continuation and lead bytes of a UTF-8 sequence: collect the
                // whole codepoint from the original slice.
                _ if byte < 0x80 => out.push(byte as char),
                _ => {
                    let start = self.pos - 1;
                    while self.peek().is_some_and(|b| (0x80..0xc0).contains(&b)) {
                        self.pos += 1;
                    }
                    let slice = &self.bytes[start..self.pos];
                    match std::str::from_utf8(slice) {
                        Ok(s) => out.push_str(s),
                        Err(_) => return Err("invalid UTF-8 in string".to_string()),
                    }
                }
            }
        }
    }

    fn unicode_escape(&mut self) -> Result<char, String> {
        let hex = self
            .bytes
            .get(self.pos..self.pos + 4)
            .ok_or_else(|| "truncated \\u escape".to_string())?;
        let text = std::str::from_utf8(hex).map_err(|_| "bad \\u escape".to_string())?;
        let code = u32::from_str_radix(text, 16).map_err(|_| "bad \\u escape".to_string())?;
        self.pos += 4;
        char::from_u32(code).ok_or_else(|| "\\u escape is not a scalar value".to_string())
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.pos;
        while self.peek().is_some_and(|b| b.is_ascii_digit()) {
            self.pos += 1;
        }
        let text = std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|_| "bad number".to_string())?;
        text.parse::<u64>()
            .map(Json::Num)
            .map_err(|e| format!("bad integer at byte {start}: {e}"))
    }

    fn boolean(&mut self) -> Result<Json, String> {
        if self.bytes[self.pos..].starts_with(b"true") {
            self.pos += 4;
            Ok(Json::Bool(true))
        } else if self.bytes[self.pos..].starts_with(b"false") {
            self.pos += 5;
            Ok(Json::Bool(false))
        } else {
            Err(format!("invalid literal at byte {}", self.pos))
        }
    }

    fn null(&mut self) -> Result<Json, String> {
        if self.bytes[self.pos..].starts_with(b"null") {
            self.pos += 4;
            Ok(Json::Null)
        } else {
            Err(format!("invalid literal at byte {}", self.pos))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_manifest_shaped_document() {
        let doc = Json::Obj(vec![
            ("version".to_string(), Json::Num(1)),
            (
                "entries".to_string(),
                Json::Arr(vec![Json::Obj(vec![
                    ("path".to_string(), Json::Str("src/a.lua".to_string())),
                    ("executable".to_string(), Json::Bool(false)),
                    ("size".to_string(), Json::Num(42)),
                ])]),
            ),
        ]);
        let text = doc.to_json_string();
        let parsed = parse(&text).unwrap();
        assert_eq!(doc, parsed);
    }

    #[test]
    fn escapes_and_unescapes_awkward_paths() {
        let s = "weird \"quoted\"/tab\there\nnewline/ünïcodé";
        let doc = Json::Str(s.to_string());
        let text = doc.to_json_string();
        assert_eq!(parse(&text).unwrap().as_str(), Some(s));
    }

    #[test]
    fn rejects_trailing_junk() {
        assert!(parse("{} garbage").is_err());
    }

    #[test]
    fn rejects_floats_outside_the_subset() {
        assert!(parse("1.5").is_err());
    }
}

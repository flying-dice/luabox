//! `file://` URI ↔ filesystem path conversion.
//!
//! `lsp_types::Uri` (0.97) is a thin wrapper over `fluent-uri` with no
//! filesystem helpers, so the drive-letter/percent-encoding dance lives
//! here. Editors send Windows paths as `file:///c%3A/dir/f.lua` (or
//! `file:///C:/dir/f.lua`); we normalise the drive letter to uppercase so
//! path keys compare equal regardless of which spelling the client uses.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use lsp_types::Uri;

/// Convert a `file://` URI to a filesystem path. Returns `None` for
/// non-`file` schemes or unparseable paths.
#[must_use]
pub fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    if let Some(scheme) = uri.scheme()
        && !scheme.as_str().eq_ignore_ascii_case("file")
    {
        return None;
    }
    let raw = uri.path().as_str();
    let decoded = percent_decode(raw);
    // `file:///C:/x` has path `/C:/x` — strip the leading slash before a
    // drive letter and uppercase the letter for stable map keys.
    let bytes = decoded.as_bytes();
    #[expect(
        clippy::string_slice,
        reason = "the guard proves byte 0 is `/` and byte 1 is ASCII, so byte offset 1 (and s[0..1]) are ASCII char boundaries"
    )]
    if bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':' {
        let mut s = decoded[1..].to_string();
        s.replace_range(0..1, &s[0..1].to_ascii_uppercase());
        return Some(PathBuf::from(s));
    }
    if decoded.is_empty() {
        return None;
    }
    Some(PathBuf::from(decoded))
}

/// Convert a filesystem path to a `file://` URI.
///
/// # Panics
///
/// Panics if the rendered URI does not parse.
///
/// That cannot happen for *any* path, not just the ones the server feeds it:
/// the loop below emits each byte either verbatim — and only from
/// [`is_uri_path_byte`], which is the RFC 3986 `pchar` set plus `/` — or as a
/// `%XX` escape, and the result is prefixed with `file://` plus a leading `/`
/// when the path has none. Every byte of the output is therefore
/// path-position-legal, so the grammar cannot reject it; the abort is on the
/// *encoder* being wrong, not on its input.
///
/// It aborts rather than returning a `Result` because the LSP has nowhere to
/// put the failure: a URI names the document in every request and response,
/// and a server that silently substituted a wrong or empty one would answer
/// requests about the wrong file. `a_path_of_every_byte_value_still_produces_
/// a_parseable_uri` sweeps the whole 0..=255 byte range through it.
#[must_use]
pub fn path_to_uri(path: &Path) -> Uri {
    use std::fmt::Write as _;

    let s = path.to_string_lossy().replace('\\', "/");
    let mut out = String::from("file://");
    if !s.starts_with('/') {
        out.push('/');
    }
    for &b in s.as_bytes() {
        if is_uri_path_byte(b) {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    #[expect(
        clippy::unreachable,
        reason = "every emitted byte is percent-encoded or in the pchar set, so the grammar \
                  cannot reject the result — see the # Panics section"
    )]
    Uri::from_str(&out).unwrap_or_else(|e| unreachable!("constructed URI is valid: {e}"))
}

/// Bytes legal verbatim in a URI path: unreserved, sub-delims, `:`, `@`, `/`.
fn is_uri_path_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'-' | b'.'
                | b'_'
                | b'~'
                | b'!'
                | b'$'
                | b'&'
                | b'\''
                | b'('
                | b')'
                | b'*'
                | b'+'
                | b','
                | b';'
                | b'='
                | b':'
                | b'@'
                | b'/'
        )
}

/// Decode `%XX` escapes (UTF-8, lossy on invalid sequences).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let (Some(hi), Some(lo)) = (
                bytes.get(i + 1).copied().and_then(hex_val),
                bytes.get(i + 2).copied().and_then(hex_val),
            )
        {
            out.push(hi * 16 + lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
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
    fn windows_path_roundtrips() {
        // A backslash spelling only denotes a Windows path on Windows; on
        // Unix it is a single filename, so drive the same roundtrip with
        // the forward-slash spelling there.
        let path = if cfg!(windows) {
            PathBuf::from(r"C:\Users\dev\proj\main.lua")
        } else {
            PathBuf::from("C:/Users/dev/proj/main.lua")
        };
        let uri = path_to_uri(&path);
        assert_eq!(uri.as_str(), "file:///C:/Users/dev/proj/main.lua");
        assert_eq!(uri_to_path(&uri).unwrap(), path);
    }

    #[test]
    fn percent_encoded_drive_colon_decodes() {
        // VS Code sends lowercase, percent-encoded drive letters.
        let uri = Uri::from_str("file:///c%3A/dir/a.lua").unwrap();
        assert_eq!(uri_to_path(&uri).unwrap(), PathBuf::from("C:/dir/a.lua"));
        // And the drive letter is case-normalised: same map key either way.
        let upper = Uri::from_str("file:///C:/dir/a.lua").unwrap();
        assert_eq!(uri_to_path(&uri), uri_to_path(&upper));
    }

    #[test]
    fn unix_path_roundtrips() {
        let uri = Uri::from_str("file:///home/dev/a.lua").unwrap();
        assert_eq!(uri_to_path(&uri).unwrap(), PathBuf::from("/home/dev/a.lua"));
    }

    #[test]
    fn spaces_and_unicode_are_encoded() {
        let path = PathBuf::from("C:/my dir/héllo.lua");
        let uri = path_to_uri(&path);
        assert_eq!(uri.as_str(), "file:///C:/my%20dir/h%C3%A9llo.lua");
        assert_eq!(uri_to_path(&uri).unwrap(), path);
    }

    /// The reachability proof behind `path_to_uri`'s `#[expect]`: the encoder
    /// emits nothing the URI path grammar can reject, whatever bytes the path
    /// holds. Sweeps the whole byte range (as one path segment per byte, plus
    /// all of them at once) rather than a hand-picked "weird path" list.
    #[test]
    fn a_path_of_every_byte_value_still_produces_a_parseable_uri() {
        for byte in 0u8..=255 {
            // `to_string_lossy` is what `path_to_uri` reads, so build the
            // path from a `String`: any byte that survives that round trip is
            // one the encoder must handle.
            let segment = String::from_utf8_lossy(&[byte]).into_owned();
            let uri = path_to_uri(Path::new(&format!("/tmp/{segment}/a.lua")));
            assert!(uri.as_str().starts_with("file:///tmp/"), "byte {byte}");
        }
        let all: String = (0u8..=255).map(|b| b as char).collect();
        let uri = path_to_uri(Path::new(&format!("/{all}")));
        assert!(uri.as_str().starts_with("file:///"));
    }

    #[test]
    fn a_relative_path_still_gets_the_leading_slash_the_scheme_requires() {
        assert_eq!(path_to_uri(Path::new("a.lua")).as_str(), "file:///a.lua");
    }

    #[test]
    fn non_file_scheme_is_rejected() {
        let uri = Uri::from_str("untitled:Untitled-1").unwrap();
        assert_eq!(uri_to_path(&uri), None);
    }

    #[test]
    fn a_uri_with_an_empty_path_is_rejected() {
        // `file://host` (authority only, no path) yields nothing to open.
        let uri = Uri::from_str("file://authority").unwrap();
        assert_eq!(uri_to_path(&uri), None);
    }

    #[test]
    fn lowercase_percent_escapes_decode() {
        let uri = Uri::from_str("file:///home/dev/a%2eb%2Fc.lua").unwrap();
        assert_eq!(
            uri_to_path(&uri).unwrap(),
            PathBuf::from("/home/dev/a.b/c.lua")
        );
    }

    #[test]
    fn a_malformed_percent_escape_is_left_verbatim() {
        // `%zz` is not a valid escape and a trailing `%` has no digits after
        // it: both pass through unchanged rather than corrupting the path.
        assert_eq!(percent_decode("a%zzb"), "a%zzb");
        assert_eq!(percent_decode("a%2"), "a%2");
        assert_eq!(percent_decode("a%"), "a%");
    }

    #[test]
    fn an_invalid_utf8_escape_sequence_decodes_lossily() {
        // A lone continuation byte is not valid UTF-8; the decoder must not
        // panic, it substitutes the replacement character.
        assert_eq!(percent_decode("%80"), "\u{FFFD}");
    }

    #[test]
    fn a_relative_path_gains_a_leading_slash() {
        // `path_to_uri` always produces an absolute URI path.
        let uri = path_to_uri(Path::new("rel/main.lua"));
        assert_eq!(uri.as_str(), "file:///rel/main.lua");
    }
}

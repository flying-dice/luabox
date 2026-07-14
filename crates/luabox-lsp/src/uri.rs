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
/// Panics if the rendered URI does not parse, which cannot happen for the
/// absolute paths the server feeds it (every produced character is either
/// percent-encoded or URI-legal).
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

    #[test]
    fn non_file_scheme_is_rejected() {
        let uri = Uri::from_str("untitled:Untitled-1").unwrap();
        assert_eq!(uri_to_path(&uri), None);
    }
}

//! SHA-256 helpers — the store's content-address function.
//!
//! Objects and trees are addressed by the lowercase hex encoding of their
//! SHA-256 digest. Hashing files is streamed so multi-megabyte archives never
//! land in memory whole.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use sha2::{Digest, Sha256};

/// Buffer size for streamed file hashing. Kept modest so it lives comfortably
/// on the stack; hashing is bandwidth-bound well below this granularity.
const CHUNK: usize = 8 * 1024;

/// Lowercase hex-encode a 32-byte digest without pulling in `hex`.
fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[usize::from(b >> 4)] as char);
        out.push(HEX[usize::from(b & 0x0f)] as char);
    }
    out
}

/// SHA-256 of an in-memory byte slice, hex-encoded.
#[must_use]
pub fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    to_hex(&hasher.finalize())
}

/// Streamed SHA-256 of a file's contents, hex-encoded.
pub fn hash_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    hash_reader(&mut file)
}

/// Streamed SHA-256 of any reader, hex-encoded.
pub fn hash_reader<R: Read>(reader: &mut R) -> io::Result<String> {
    let mut hasher = Sha256::new();
    let mut buf = [0u8; CHUNK];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(to_hex(&hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_is_the_known_sha256() {
        // The canonical SHA-256 of the empty string.
        assert_eq!(
            hash_bytes(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn abc_matches_the_reference_vector() {
        assert_eq!(
            hash_bytes(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn reader_and_bytes_agree() {
        let data = b"the quick brown fox";
        let mut cursor = std::io::Cursor::new(data);
        assert_eq!(hash_reader(&mut cursor).unwrap(), hash_bytes(data));
    }
}

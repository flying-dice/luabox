//! The OS keychain seam — where `luabox login` stores the GitHub token so it
//! survives across shells without living in plaintext.
//!
//! The token is held **encrypted at rest** by the platform's own secret store
//! (macOS Keychain, Windows Credential Manager, Linux Secret Service) via the
//! `keyring` crate. luabox never writes the token to a dotfile.
//!
//! ## Graceful degradation
//!
//! A platform store is not always reachable — headless Linux and CI boxes
//! frequently have no Secret Service running, and `keyring` surfaces that as a
//! `NoStorageAccess`/`PlatformFailure` error rather than a missing entry. Every
//! function here returns a [`keyring::Result`] so callers can degrade instead
//! of aborting: `login` prints a fallback pointing at `LUABOX_GITHUB_TOKEN`,
//! and the token-lookup path ([`crate::github`]) simply treats any read failure
//! as "no stored token" and falls through to anonymous. Nothing in this module
//! panics.

use keyring::{Entry, Error};

/// The Credential-Manager / Keychain / Secret-Service *service* name.
const SERVICE: &str = "luabox";

/// The account (username) the token is filed under within [`SERVICE`].
const ACCOUNT: &str = "github-token";

/// The keychain entry handle for luabox's GitHub token.
fn entry() -> keyring::Result<Entry> {
    Entry::new(SERVICE, ACCOUNT)
}

/// Store `token` in the OS keychain, replacing any existing value.
///
/// Returns the raw `keyring` error on failure (e.g. `NoStorageAccess` on a
/// headless box) so the caller can print the env-var fallback.
pub(crate) fn store(token: &str) -> keyring::Result<()> {
    entry()?.set_password(token)
}

/// Read the stored token, or `Ok(None)` when none has been saved.
///
/// A genuinely missing entry (`NoEntry`) is not an error — it is the common
/// "not signed in" case and maps to `Ok(None)`. A store that cannot be reached
/// at all still returns `Err`, which the token-lookup path swallows.
pub(crate) fn retrieve() -> keyring::Result<Option<String>> {
    match entry()?.get_password() {
        Ok(token) => Ok(Some(token)),
        Err(Error::NoEntry) => Ok(None),
        Err(other) => Err(other),
    }
}

/// Delete the stored token; idempotent — a missing entry is success.
pub(crate) fn delete() -> keyring::Result<()> {
    match entry()?.delete_credential() {
        Ok(()) | Err(Error::NoEntry) => Ok(()),
        Err(other) => Err(other),
    }
}

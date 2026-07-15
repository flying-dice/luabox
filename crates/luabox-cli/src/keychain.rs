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

/// The Credential-Manager / Keychain / Secret-Service *service* name, shared by
/// every secret luabox stores.
const SERVICE: &str = "luabox";

/// The account (username) the GitHub token is filed under within [`SERVICE`].
const GITHUB_ACCOUNT: &str = "github-token";

/// The account the luarocks.org upload API key is filed under within
/// [`SERVICE`] — populated by `luabox login --luarocks`, read by
/// `luabox publish`.
const LUAROCKS_ACCOUNT: &str = "luarocks-api-key";

/// A keychain entry handle in luabox's [`SERVICE`] for `account`.
fn entry(account: &str) -> keyring::Result<Entry> {
    Entry::new(SERVICE, account)
}

/// Store `token` in the OS keychain, replacing any existing value.
///
/// Returns the raw `keyring` error on failure (e.g. `NoStorageAccess` on a
/// headless box) so the caller can print the env-var fallback.
pub(crate) fn store(token: &str) -> keyring::Result<()> {
    entry(GITHUB_ACCOUNT)?.set_password(token)
}

/// Read the stored token, or `Ok(None)` when none has been saved.
///
/// A genuinely missing entry (`NoEntry`) is not an error — it is the common
/// "not signed in" case and maps to `Ok(None)`. A store that cannot be reached
/// at all still returns `Err`, which the token-lookup path swallows.
pub(crate) fn retrieve() -> keyring::Result<Option<String>> {
    retrieve_account(GITHUB_ACCOUNT)
}

/// Delete the stored token; idempotent — a missing entry is success.
pub(crate) fn delete() -> keyring::Result<()> {
    delete_account(GITHUB_ACCOUNT)
}

/// Store the luarocks.org API key in the OS keychain, replacing any existing
/// value (`luabox login --luarocks`).
pub(crate) fn store_luarocks_key(key: &str) -> keyring::Result<()> {
    entry(LUAROCKS_ACCOUNT)?.set_password(key)
}

/// Read the stored luarocks.org API key, or `Ok(None)` when none has been
/// saved. A missing entry maps to `Ok(None)`; an unreachable store returns
/// `Err` for the caller to degrade.
pub(crate) fn retrieve_luarocks_key() -> keyring::Result<Option<String>> {
    retrieve_account(LUAROCKS_ACCOUNT)
}

/// Delete the stored luarocks.org API key; idempotent (`luabox logout`).
pub(crate) fn delete_luarocks_key() -> keyring::Result<()> {
    delete_account(LUAROCKS_ACCOUNT)
}

/// Read `account`'s secret, mapping a genuinely missing entry to `Ok(None)`.
fn retrieve_account(account: &str) -> keyring::Result<Option<String>> {
    match entry(account)?.get_password() {
        Ok(secret) => Ok(Some(secret)),
        Err(Error::NoEntry) => Ok(None),
        Err(other) => Err(other),
    }
}

/// Delete `account`'s secret; a missing entry is success (idempotent).
fn delete_account(account: &str) -> keyring::Result<()> {
    match entry(account)?.delete_credential() {
        Ok(()) | Err(Error::NoEntry) => Ok(()),
        Err(other) => Err(other),
    }
}

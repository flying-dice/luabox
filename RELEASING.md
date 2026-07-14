# Releasing luabox

The process for cutting a tagged release, from a green `main` to binaries
attached to a GitHub Release. The public home is
<https://github.com/flying-dice/luabox> (`github` remote); the tailnet GitLab
remains the private origin for internal CI. See [CHANGELOG.md](CHANGELOG.md)
for the format releases are documented in, and
[`.github/workflows/release.yml`](.github/workflows/release.yml) for the
automation this process drives.

## Process

1. **Confirm `main` is green.** CI ([`.github/workflows/ci.yml`](.github/workflows/ci.yml),
   mirrored by the internal `.gitlab-ci.yml` check/test stages) must be
   passing on the commit you intend to release.
2. **Bump the version.** Edit `[workspace.package] version` in the
   workspace root `Cargo.toml` (every crate inherits it via
   `version.workspace = true`). Run `cargo check` (or `cargo build`) once
   so `Cargo.lock` picks up the bump.
3. **Finalize `CHANGELOG.md`.**
   - Move anything sitting under `## [Unreleased]` into a new
     `## [x.y.z] - YYYY-MM-DD` entry (or, for the first release, replace
     the `## [0.1.0] - drafted, unreleased` header's suffix with the real
     date).
   - Leave `## [Unreleased]` in place, empty, for the next round of
     changes.
   - This section is what the release workflow extracts verbatim as the
     GitHub Release notes body — keep it accurate and free of placeholder
     text before tagging.
4. **Commit.** `git commit -am "release: vX.Y.Z"` (or however your
   project's commit conventions phrase it) on `main`.
5. **Tag and push to `github`.**
   ```sh
   git tag vX.Y.Z
   git push github main
   git push github vX.Y.Z
   ```
   The `v` prefix is load-bearing: `release.yml` triggers only on tags
   matching `v*`.
6. **The release workflow does the rest.** On the `v*` tag,
   `.github/workflows/release.yml`:
   - Creates a GitHub Release with the matching `CHANGELOG.md` section as
     the notes body.
   - Builds the release binaries — Linux x86_64, macOS Apple Silicon, and
     Windows x86_64 — computes `SHA256SUMS`, and uploads them (with the
     `scripts/install.*` one-liners) as release assets.
   - **Smoke-installs** the freshly published binary on all three OSes via
     the one-line installers, and **only then marks the release as
     `latest`.** A release that fails any of the three smoke installs does
     not go latest — the installers keep resolving the previous good
     release until the failure is fixed and a new tag is cut.
7. **Verify.** Once the workflow finishes, check the
   [GitHub Releases page](https://github.com/flying-dice/luabox/releases)
   for the new release, its assets, and that it is marked latest;
   spot-check `scripts/install.sh`/`scripts/install.ps1` resolve and install
   it.
## Editor extensions

The editor integrations live in their own repos, version independently, and
cut their own releases — nothing in this repo's release process builds or
attaches them:

- **VS Code**: <https://github.com/flying-dice/luabox-vscode> — its release
  workflow packages the `.vsix` and attaches it to that repo's releases.
  The `.vsix` is what gets drag-and-dropped into the VS Code Marketplace
  publisher portal (<https://marketplace.visualstudio.com/manage>), or
  `npx ovsx publish`'d to Open VSX — both need Jonathan's publisher
  account/token.
- **JetBrains**: <https://github.com/flying-dice/luabox-jetbrains> — its
  release workflow builds the plugin `.zip` for install-from-disk;
  JetBrains Marketplace publishing likewise needs a vendor account/token.

Cut extension releases when *their* code changes; a toolchain release does
not require an extension release (the extensions track the `luabox` binary
on PATH, whatever its version).

## SemVer policy for 0.x

Standard SemVer (`https://semver.org`) applies, with the usual 0.x
looseness made explicit rather than left ambiguous:

- **While the major version is `0`,** minor version bumps (`0.1.0` →
  `0.2.0`) may contain breaking changes to:
  - CLI flags and subcommand behavior (`luabox.toml` shape, flag names,
    default values, output formats).
  - Type-checking semantics — as the LuaCATS-strictness launch gate lands
    (see [DIRECTION.md](DIRECTION.md)), diagnostics that didn't fire
    before may start firing, and vice versa. A 0.x bump is fair warning,
    not a stability promise on checker output.
  - Patch bumps (`0.1.0` → `0.1.1`) are reserved for backwards-compatible
    fixes only, same as post-1.0 SemVer.
- **The LuaCATS annotation surface itself is not luabox's to version.**
  `---@class`/`---@field`/etc. follow the upstream lua-language-server
  standard; luabox tracks it rather than forking it, so annotation syntax
  compatibility isn't part of luabox's own SemVer contract.
- **Post-1.0** (once the feature-parity + strictness launch gate in
  DIRECTION.md is reached and the initial public release milestone in
  BACKLOG.md closes), the usual SemVer guarantees apply in full: no
  breaking CLI/manifest/diagnostic changes without a major bump.

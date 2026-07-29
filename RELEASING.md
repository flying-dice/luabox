# Releasing luabox

The process for cutting a tagged release, from a green `main` to binaries
attached to a GitHub Release. The public home is
<https://github.com/flying-dice/luabox> (`github` remote); the tailnet GitLab
remains the private origin for internal CI. See [CHANGELOG.md](CHANGELOG.md)
for the format releases are documented in, and
[`.github/workflows/release.yml`](.github/workflows/release.yml) for the
automation this process drives.

Last released: **0.1.4** (2026-07-14). Next up: **0.2.0**, the v1 scope cut —
a breaking minor under the 0.x policy below, since it removes commands (see
[DIRECTION.md](DIRECTION.md#v1-scope-cut-accepted-2026-07-26)). Nothing in
this process publishes to a package registry: releases are GitHub Release
assets plus the install one-liners, and luabox holds no publishing
credential of its own.

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
     `## [x.y.z] - YYYY-MM-DD` entry. Where an entry is already drafted
     ahead of the tag — as `## [0.2.0] - 2026-07-26 (unreleased)` is —
     drop the `(unreleased)` suffix and correct the date to the release
     day.
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
   `.github/workflows/release.yml` runs six chained jobs. The shape matters:
   the release exists as a **draft** for the whole of it, and only becomes a
   real release once the artefacts it ships have been installed and fully
   exercised on every OS.

   1. **Create draft release** — a GitHub Release with the matching
      `CHANGELOG.md` section as the notes body, created with
      `--draft=true`. A draft has no public download URLs, is not returned
      by `/releases/latest`, and is invisible to anyone without repo read
      access — so a release that fails verification never existed as far as
      users are concerned.
   2. **Build** — release binaries for Linux x86_64, macOS Apple Silicon,
      and Windows x86_64, from the committed `Cargo.lock` (`--locked`).
   3. **Upload assets** — the three archives, `SHA256SUMS`, and both
      `scripts/install.*` one-liners, attached to the draft. The installers
      are attached (not just left in-tree) because the next job downloads
      them back out: the script that gets tested must be the artefact users
      will actually fetch.
   4. **Verify** — *the release gate*, on all three OSes. Each leg pulls
      `install.sh`/`install.ps1` out of the **draft**, runs it to install the
      **draft's** binary, asserts `luabox --version` reports the tag, and
      then runs the **entire black-box e2e suite** — the cucumber
      `acceptance` and `lsp_acceptance` targets, i.e. the whole executable
      spec — against that **installed** binary, via `LUABOX_E2E_BIN`. Two
      installer-only probes ride along, because the e2e suite tests the
      binary rather than the installer: installing into a directory whose
      path contains a space, and a pinned nonexistent version having to fail
      non-zero. On Windows the spaced-dir install also runs through
      `powershell.exe -NoProfile -` (REPL mode, where PSReadLine's
      `RuntimeInformation` stub shadows the real type).
   5. **Publish release (draft → current)** — reachable only if all three
      verify legs passed. Re-checks the release has exactly the six expected
      assets, re-downloads every archive and re-runs `sha256sum -c` against
      the attached `SHA256SUMS`, and then performs the **single**
      draft→published transition: `gh release edit "$TAG" --draft=false
      --latest`. This is the moment the release goes live.
   6. **Post-publish smoke** — the handful of things that can only work once
      the release is public: the no-token `curl … | bash` / `irm … | iex`
      one-liners against the real release-download URLs, and
      `luabox upgrade <tag>` (which resolves *published* releases, so it
      cannot see a draft) plus its negative. The release is already live
      when this runs; a failure here rolls nothing back, it just turns the
      pipeline red so a human can yank the release or cut a fix tag.

   Installing from a draft needs credentials users do not have, so
   `scripts/install.sh` and `scripts/install.ps1` grew exactly one extra
   path: under an explicit `LUABOX_DRAFT_INSTALL=1` opt-in — which requires
   `GITHUB_TOKEN` and a pinned `LUABOX_VERSION`, and fails loudly if either
   is missing — they resolve the release through the authenticated GitHub API
   (drafts appear only in the *list* endpoint — `releases/tags/<tag>` 404s
   for a draft) and fetch each asset by id, checksum verification included.
   `install.sh` needs `jq` on that path. Without the opt-in the scripts
   behave exactly as they always have — a developer whose shell exports
   `GITHUB_TOKEN` ambiently (Codespaces, `gh auth` setups) stays on the
   public path; that default behaviour is what job 6 proves against the real
   public URLs.
7. **Verify.** Once the workflow finishes, check the
   [GitHub Releases page](https://github.com/flying-dice/luabox/releases)
   for the new release: it should no longer be a draft, should carry all six
   assets, and should be marked latest. If the run went red *before* the
   publish job, the release is still sitting there as a draft — fix the
   cause and either delete the draft and re-tag, or re-run the workflow.

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

Cut extension releases when *their* code changes; a `luabox` release does
not require an extension release (the extensions track the `luabox` binary
on PATH, whatever its version). One exception worth knowing for 0.2.0: the
extensions' "Sign in with GitHub" affordance lost its backing command with
`luabox login`, so an extension release is needed to *remove* it — but that
is an extension-side change, cut on their schedule.

## SemVer policy for 0.x

Standard SemVer (`https://semver.org`) applies, with the usual 0.x
looseness made explicit rather than left ambiguous:

- **While the major version is `0`,** minor version bumps (`0.1.4` →
  `0.2.0`) may contain breaking changes to:
  - CLI flags and subcommand behavior (`luabox.toml` shape, flag names,
    default values, output formats) — **including removing subcommands
    outright**, which is exactly what 0.2.0 does to the dependency,
    credential and execution commands.
  - Type-checking semantics — as the LuaCATS-strictness launch gate lands
    (see [DIRECTION.md](DIRECTION.md)), diagnostics that didn't fire
    before may start firing, and vice versa. A 0.x bump is fair warning,
    not a stability promise on checker output.
  - Patch bumps (`0.2.0` → `0.2.1`) are reserved for backwards-compatible
    fixes only, same as post-1.0 SemVer.
- **The LuaCATS annotation surface itself is not luabox's to version.**
  `---@class`/`---@field`/etc. follow the upstream lua-language-server
  standard; luabox tracks it rather than forking it, so annotation syntax
  compatibility isn't part of luabox's own SemVer contract.
- **Post-1.0** (once the feature-parity + strictness launch gate in
  DIRECTION.md is reached and the initial public release milestone in
  BACKLOG.md closes), the usual SemVer guarantees apply in full: no
  breaking CLI/manifest/diagnostic changes without a major bump.

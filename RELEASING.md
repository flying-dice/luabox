# Releasing luabox

The process for cutting a tagged release, from a green `main` to binaries
attached to a GitLab Release. See [CHANGELOG.md](CHANGELOG.md) for the
format releases are documented in, and [`.gitlab-ci.yml`](.gitlab-ci.yml)
for the automation this process drives.

## Process

1. **Confirm `main` is green.** The `check` and `test` stages
   (`.gitlab-ci.yml`) must be passing on the commit you intend to release.
2. **Bump the version.** Edit `[workspace.package] version` in the
   workspace root `Cargo.toml` (every crate inherits it via
   `version.workspace = true`). Run `cargo check` (or `cargo build`) once
   so `Cargo.lock` picks up the bump.
3. **Update `CHANGELOG.md`.**
   - Move anything sitting under `## [Unreleased]` into a new
     `## [x.y.z] - YYYY-MM-DD` entry (or, for the first release, replace
     the `## [0.1.0] - drafted, unreleased` header's suffix with the real
     date).
   - Leave `## [Unreleased]` in place, empty, for the next round of
     changes.
   - This section is what the `release` stage's CI job extracts verbatim
     as the GitLab Release notes body — keep it accurate and free of
     placeholder text before tagging.
4. **Commit.** `git commit -am "release: vX.Y.Z"` (or however your
   project's commit conventions phrase it) on `main`.
5. **Tag and push.**
   ```sh
   git tag vX.Y.Z
   git push origin main
   git push origin vX.Y.Z
   ```
   The `v` prefix is load-bearing: `.gitlab-ci.yml`'s `release` stage (and
   its `workflow: rules`) only trigger on tags matching `^v`.
6. **CI builds and attaches binaries.** The tag pipeline's `release` stage:
   - Builds `--release` for Linux (`build-linux`, required) and, where a
     matching runner is registered, Windows and macOS
     (`build-windows`/`build-macos`, both `allow_failure: true` — see the
     comments in `.gitlab-ci.yml` for the runner tags an instance
     administrator needs to provision before those stop being best-effort).
   - Extracts the matching `CHANGELOG.md` section and publishes a GitLab
     Release via `release-cli`, with the Linux binary (and any others that
     built) attached as assets.
7. **Verify.** Once the pipeline finishes, check the GitLab Releases page
   for the new release and its assets, then sanity-check
   `scripts/install.sh`/`scripts/install.ps1` actually resolve and install
   it (this is the first point at which those scripts can be exercised
   end-to-end — see BACKLOG.md #95).

## Editor extensions

The four editor integrations under `editors/` (VS Code, Zed, JetBrains,
Neovim) all wrap the same released `luabox` binary (`luabox lsp`, stdio).
They are versioned independently of the CLI/LSP crate version today (each
pins its own `0.1.0` in its own manifest) — bump each editor's version
manifest by hand when its own code changes, not automatically off the
workspace `version`.

### Packaging each integration

| Editor | Command | Output |
| --- | --- | --- |
| VS Code | `cd editors/vscode && npm ci && npx @vscode/vsce package` | `editors/vscode/luabox-<version>.vsix` |
| JetBrains | `cd editors/jetbrains && ./gradlew buildPlugin` (`gradlew.bat` on Windows; needs **JDK 17+** on `PATH`/`JAVA_HOME`) | `editors/jetbrains/build/distributions/luabox-jetbrains-<version>.zip` |
| Zed | `cd editors/zed && rustup target add wasm32-wasip2 && cargo build --target wasm32-wasip2 --release` | sanity-checks the wasm compiles (Zed itself rebuilds it from source on install — there is no separate packaged artifact to ship) |
| Neovim | none — `editors/nvim` *is* the distributable | n/a |

None of this is wired into `.gitlab-ci.yml` yet: the `release` stage builds
and attaches only the Rust CLI/LSP binaries. Packaging the editor
integrations above is a manual step today.

### Residual manual steps (not automatable without credentials this repo doesn't hold)

These are the steps that remain **after** #93 (merging `shapes-v2` and
pushing/tagging a real release) — each requires an account/token that isn't
available in this environment:

1. **VS Code Marketplace**: create the `luabox` publisher at
   <https://marketplace.visualstudio.com/manage>, mint an Azure DevOps PAT
   with **Marketplace ▸ Manage** scope, then
   `npx @vscode/vsce login luabox && npx @vscode/vsce publish` from
   `editors/vscode`. See `editors/vscode/README.md#publishing-to-the-marketplace`.
2. **Open VSX** (VSCodium/Cursor/Gitpod): `npx ovsx publish` with an Open VSX
   access token, same `.vsix`.
3. **Attach the `.vsix` to the GitLab release**: until CI does this
   automatically, upload `editors/vscode/luabox-<version>.vsix` as a release
   asset by hand (GitLab Releases UI, or `release-cli` with an extra
   `--assets-link`) after the tag pipeline in [Process](#process) finishes.
4. **JetBrains Marketplace**: claim a vendor at
   <https://plugins.jetbrains.com/>, generate a permanent Marketplace token,
   configure `intellijPlatform { signing { … }; publishing { token = … } }`
   in `editors/jetbrains/build.gradle.kts` (certificate + key for signing,
   read from environment variables, not committed), then
   `./gradlew signPlugin publishPlugin`. First submissions are manually
   reviewed by JetBrains before appearing in the Marketplace. See
   `editors/jetbrains/README.md#publishing-to-the-jetbrains-marketplace`.
   Attaching the plugin zip to the GitLab release itself (for the
   "install from disk" path) is the same manual-upload story as step 3.
5. **Zed extension registry**: this repo is self-hosted on a tailnet, so
   before the PR in `zed-industries/extensions` (adding this extension as a
   submodule to their `extensions.toml`) is even possible, `editors/zed`
   needs mirroring to a **public** git host (with a top-level license file)
   and `extension.toml`'s `repository` field updated to match. See
   `editors/zed/README.md#publishing-to-the-zed-extension-registry` for the
   full submodule/PR sequence.
6. **Neovim**: no registry to publish to — if a standalone
   `luabox/luabox.nvim` repo is wanted (so plugin-manager users don't pull
   the whole monorepo), mirror `editors/nvim`'s contents there. No token
   required, just hosting.

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

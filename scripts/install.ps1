<#
.SYNOPSIS
    Install the latest luabox release binary from the canonical GitLab
    instance (gitlab.beluga-sirius.ts.net:flying-dice/luabox) — Windows.
    POSIX counterpart: scripts/install.sh.

.DESCRIPTION
    Looks up the latest release via the GitLab releases API, downloads the
    `luabox-x86_64-windows.exe` asset, and installs it as `luabox.exe` into
    `%USERPROFILE%\.cargo\bin` by default (already on PATH for anyone with
    rustup installed — a fair default for a Rust-toolchain-adjacent tool;
    override with -InstallDir or $env:LUABOX_INSTALL_DIR).

    Until the first `v*` tag exists (see RELEASING.md), the GitLab releases
    API has nothing to serve; this script detects that and errors out with
    a pointer to the `cargo install --git` fallback. That means the happy
    path here is unproven until the first tag lands — written defensively
    (every external call's result checked) but not yet end-to-end verified.

.PARAMETER InstallDir
    Where to place luabox.exe. Defaults to $env:LUABOX_INSTALL_DIR, else
    "$env:USERPROFILE\.cargo\bin".

.EXAMPLE
    irm https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/raw/main/scripts/install.ps1 | iex
    # or, checked out locally:
    powershell -File scripts/install.ps1
#>
[CmdletBinding()]
param(
    [string]$InstallDir = $(if ($env:LUABOX_INSTALL_DIR) { $env:LUABOX_INSTALL_DIR } else { Join-Path $env:USERPROFILE ".cargo\bin" })
)

$ErrorActionPreference = "Stop"

$GitLabHost = "gitlab.beluga-sirius.ts.net"
$ProjectPath = "flying-dice/luabox"
$ProjectPathEncoded = "flying-dice%2Fluabox"
$ApiBase = "https://$GitLabHost/api/v4/projects/$ProjectPathEncoded"
$AssetName = "luabox-x86_64-windows.exe"

function Write-Info($msg) { Write-Host "==> $msg" }
function Write-ErrorMsg($msg) { Write-Host "error: $msg" -ForegroundColor Red }
function Write-WarnMsg($msg) { Write-Host "warning: $msg" -ForegroundColor Yellow }

function Show-FallbackHint {
    Write-Host ""
    Write-Host "No published release was found. Until the first v* tag lands (see"
    Write-Host "RELEASING.md), install straight from source instead:"
    Write-Host ""
    Write-Host "    cargo install --git ssh://git@$GitLabHost/$ProjectPath.git luabox-cli"
    Write-Host ""
    Write-Host "(requires an SSH key registered with the GitLab instance, and a Rust"
    Write-Host "toolchain -- see https://rustup.rs)."
}

Write-Info "looking up the latest release for $ProjectPath on $GitLabHost..."

$release = $null
try {
    $release = Invoke-RestMethod -Uri "$ApiBase/releases/permalink/latest" -Method Get -ErrorAction Stop
} catch {
    Write-ErrorMsg "no release found at $ApiBase/releases/permalink/latest ($($_.Exception.Message))"
    Show-FallbackHint
    exit 1
}

if (-not $release -or -not $release.tag_name) {
    Write-ErrorMsg "no release found at $ApiBase/releases/permalink/latest"
    Show-FallbackHint
    exit 1
}

$asset = $null
if ($release.assets -and $release.assets.links) {
    $asset = $release.assets.links | Where-Object { $_.name -eq $AssetName } | Select-Object -First 1
}

if (-not $asset) {
    Write-ErrorMsg "release $($release.tag_name) has no '$AssetName' asset"
    Show-FallbackHint
    exit 1
}

Write-Info "found asset: $($asset.url)"

$tmpFile = [System.IO.Path]::GetTempFileName()
try {
    Write-Info "downloading..."
    Invoke-WebRequest -Uri $asset.url -OutFile $tmpFile -UseBasicParsing

    if (-not (Test-Path $InstallDir)) {
        New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    }
    $dest = Join-Path $InstallDir "luabox.exe"
    Move-Item -Path $tmpFile -Destination $dest -Force
} catch {
    Write-ErrorMsg "download/install failed: $($_.Exception.Message)"
    if (Test-Path $tmpFile) { Remove-Item $tmpFile -Force -ErrorAction SilentlyContinue }
    exit 1
}

Write-Info "installed to $dest"

$pathDirs = $env:PATH -split ";"
if ($pathDirs -notcontains $InstallDir.TrimEnd("\")) {
    Write-WarnMsg "$InstallDir is not on PATH; add it, e.g.:"
    Write-Host "    [Environment]::SetEnvironmentVariable('PATH', `"`$env:PATH;$InstallDir`", 'User')"
}

Write-Info "verifying..."
try {
    & $dest --version
    Write-Info "luabox installed successfully."
} catch {
    Write-ErrorMsg "installed binary at $dest failed to run '--version' -- the download may be corrupt or for the wrong platform"
    exit 1
}

#!/usr/bin/env pwsh
# Keep the examples green (Windows / PowerShell). Mirrors scripts/examples.sh:
# for every project under examples/ run the core gate (check, fmt --check,
# lint) plus per-example extras. Exits non-zero if any step fails.
#
# Usage: pwsh scripts/examples.ps1   (or:  powershell -File scripts\examples.ps1)
# Honours $env:LUABOX (path to the luabox binary); defaults to
# target/release/luabox.exe.

$ErrorActionPreference = 'Continue'
$repoRoot = Split-Path -Parent $PSScriptRoot
$examples = Join-Path $repoRoot 'examples'

$luabox = $env:LUABOX
if (-not $luabox) { $luabox = Join-Path $repoRoot 'target/release/luabox.exe' }
if (-not (Test-Path $luabox)) {
    $alt = Join-Path $repoRoot 'target/release/luabox'
    if (Test-Path $alt) { $luabox = $alt }
}
if (-not (Test-Path $luabox)) {
    Write-Error "luabox binary not found at '$luabox' — run 'cargo build --release'"
    exit 1
}
$luabox = (Resolve-Path $luabox).Path
# Put the binary on PATH so `[tasks]` that call `luabox` resolve.
$env:PATH = (Split-Path -Parent $luabox) + [IO.Path]::PathSeparator + $env:PATH

# Find a Lua interpreter for run steps. All locally-run example output
# (including timemachine's lowered bundle) is Lua 5.1-compatible, so one
# interpreter set via LUABOX_LUA drives every edition deterministically.
$lua = $null
foreach ($cand in @('lua', 'lua5.4', 'lua54', 'lua5.3', 'lua5.1', 'lua51', 'luajit')) {
    $found = Get-Command $cand -ErrorAction SilentlyContinue
    if ($found) { $lua = $found.Source; break }
}
if ($lua) {
    $env:LUABOX_LUA = $lua
    Write-Host "==> using Lua runtime: $lua"
} else {
    Write-Host "==> no Lua runtime on PATH — run steps will be skipped (not a failure)"
}

$script:fails = 0
function Pass($label) { Write-Host "    ok   $label" }
function Fail($label, $output) {
    Write-Host "    FAIL $label" -ForegroundColor Red
    if ($output) { $output -split "`n" | ForEach-Object { Write-Host "         | $_" } }
    $script:fails++
}

# Invoke luabox (or any exe) and report pass/fail on exit code.
function Step($label, $exe, [string[]]$argv) {
    $out = & $exe @argv 2>&1 | Out-String
    if ($LASTEXITCODE -eq 0) { Pass $label } else { Fail $label $out }
}

function Gate() {
    Step 'check'       $luabox @('check')
    Step 'fmt --check' $luabox @('fmt', '--check')
    Step 'lint'        $luabox @('lint')
}

function Section($name) { Write-Host ''; Write-Host "== $name ==" }

# 1. hello-luabox
Section 'hello-luabox'
Set-Location (Join-Path $examples 'hello-luabox')
Gate

# 2. geometry
Section 'geometry'
Set-Location (Join-Path $examples 'geometry')
Gate

# 3. renderer (path dep — install first)
Section 'renderer'
Set-Location (Join-Path $examples 'renderer')
Step 'install' $luabox @('install')
Gate
if ($lua) {
    $out = & $luabox run src/main.lua 2>&1 | Out-String
    if ($LASTEXITCODE -eq 0 -and $out -match 'area = 16') { Pass 'run (draws a square)' }
    else { Fail 'run (draws a square)' $out }
}

# 4. legacy-inifile
Section 'legacy-inifile'
Set-Location (Join-Path $examples 'legacy-inifile')
Gate

# 5. timemachine (build + bundle + run lowered output on Lua 5.1)
Section 'timemachine'
Set-Location (Join-Path $examples 'timemachine')
Gate
Step 'build'                       $luabox @('build')
Step 'bundle --minify --sourcemap' $luabox @('bundle', '--minify', '--sourcemap')
if ($lua) {
    $out = & $lua dist/timemachine.lua 2>&1 | Out-String
    if ($LASTEXITCODE -eq 0 -and $out -match 'sum\(1\.\.5\) = 15') { Pass 'run lowered bundle on Lua 5.1' }
    else { Fail 'run lowered bundle on Lua 5.1' $out }
} else {
    Write-Host '    skip lowered-run (no Lua runtime)'
}

# 6. love-asteroids-lite (bundle a .love and inspect its contents)
Section 'love-asteroids-lite'
Set-Location (Join-Path $examples 'love-asteroids-lite')
Gate
Step 'bundle --mode love' $luabox @('bundle', '--mode', 'love')
$lovePath = Join-Path (Get-Location) 'dist/asteroids-lite.love'
try {
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [System.IO.Compression.ZipFile]::OpenRead($lovePath)
    $names = $zip.Entries | ForEach-Object { $_.FullName }
    $zip.Dispose()
    if (($names -match 'main\.lua') -and ($names -match 'conf\.lua')) {
        Pass '.love contains main.lua + conf.lua'
    } else {
        Fail '.love contains main.lua + conf.lua' ($names -join "`n")
    }
} catch {
    Fail '.love contains main.lua + conf.lua' $_.Exception.Message
}

# 7. workspace (check fans out; gate a member standalone)
Section 'workspace'
Set-Location (Join-Path $examples 'workspace')
Gate
Set-Location (Join-Path $examples 'workspace/packages/core')
Step 'check (core member)' $luabox @('check')

Write-Host ''
if ($script:fails -eq 0) {
    Write-Host 'examples: ALL GREEN' -ForegroundColor Green
    exit 0
} else {
    Write-Host "examples: $($script:fails) step(s) FAILED" -ForegroundColor Red
    exit 1
}

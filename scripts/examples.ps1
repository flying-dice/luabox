#!/usr/bin/env pwsh
# Keep the examples green (Windows / PowerShell). Mirrors scripts/examples.sh:
# for every project under examples/ run the core gate (check, fmt --check,
# lint) plus per-example extras (build tree + bundle, .love packaging). Exits
# non-zero if any step fails.
#
# luabox itself never runs Lua — it is a static toolchain. The bash script
# additionally executes the built timemachine bundle under lua5.1 to prove the
# compiler's output runs; Windows CI has no lua5.1 package, so that step SKIPs
# here. Parity is deliberate and minimal: same step, same label, no runtime.
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

# The interpreter the built bundle would be executed under (timemachine's
# `[build] target` is 5.1). Windows CI installs no Lua, so this is normally
# empty and the run step SKIPs loudly-but-green — see scripts/examples.sh.
$lua51 = $null
foreach ($cand in @('lua5.1', 'lua51')) {
    if (Get-Command $cand -ErrorAction SilentlyContinue) { $lua51 = $cand; break }
}
if ($lua51) {
    Write-Host "==> executing built output under: $lua51"
} else {
    Write-Host '==> SKIP: no lua5.1 on PATH — the built bundle will not be executed'
}

$script:fails = 0
function Pass($label) { Write-Host "    ok   $label" }
function Skip($label) { Write-Host "    SKIP $label" }
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

# 3. renderer (path dep — cross-package types, read in place)
Section 'renderer'
Set-Location (Join-Path $examples 'renderer')
Gate

# 4. legacy-inifile
Section 'legacy-inifile'
Set-Location (Join-Path $examples 'legacy-inifile')
Gate

# 5. timemachine (build tree + bundle + run the lowered output)
Section 'timemachine'
Set-Location (Join-Path $examples 'timemachine')
Gate
# Config bundles to dist/timemachine.lua (minified, with a .map); --no-bundle
# forces the mirrored tree emit under dist/src/ instead.
Step 'build --no-bundle' $luabox @('build', '--no-bundle')
Step 'build'             $luabox @('build')
# Execute what we just compiled, when a 5.1 interpreter is available.
if ($lua51) {
    $out = & $lua51 'dist/timemachine.lua' 2>&1 | Out-String
    if (($LASTEXITCODE -eq 0) -and ($out -match 'sum\(1\.\.5\) = 15')) {
        Pass 'run lowered bundle on Lua 5.1'
    } else {
        Fail 'run lowered bundle on Lua 5.1' $out
    }
} else {
    Skip 'run lowered bundle on Lua 5.1 (no lua5.1 on PATH)'
}

# 6. love-asteroids-lite (bundle a .love and inspect its contents)
Section 'love-asteroids-lite'
Set-Location (Join-Path $examples 'love-asteroids-lite')
Gate
# `[build] mode = "love"` makes a bare `luabox build` package the .love.
Step 'build (.love via mode=love)' $luabox @('build')
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

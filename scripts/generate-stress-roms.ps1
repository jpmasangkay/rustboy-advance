# Regenerates the 3 git-ignored synthetic stress ROMs under `roms\homebrew\`
# and copies any fetched TONC ROMs from `test-roms\tonc\` into `roms\homebrew\`.
[CmdletBinding()]
param(
    [switch]$IncludeTonc
)

$ErrorActionPreference = "Stop"

$RepoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $RepoRoot

cargo run -p gba-testrunner --example gen_stress_roms
if ($LASTEXITCODE -ne 0) {
    throw "Failed generating synthetic stress ROMs (exit $LASTEXITCODE)"
}

if ($IncludeTonc) {
    $toncSrcDir = Join-Path $RepoRoot "test-roms\tonc"
    $destDir = Join-Path $RepoRoot "roms\homebrew"
    New-Item -ItemType Directory -Path $destDir -Force | Out-Null
    foreach ($name in @("m7_ex.gba", "win_demo.gba", "bld_demo.gba", "mos_demo.gba", "oacombo.gba", "bm_modes.gba")) {
        $src = Join-Path $toncSrcDir $name
        if (Test-Path $src) {
            Copy-Item -Path $src -Destination (Join-Path $destDir $name) -Force
        }
    }
}

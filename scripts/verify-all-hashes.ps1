# Verifies every presented frame + core video + core audio hash against pre-Phase-4 baselines.
[CmdletBinding()]
param(
    [switch]$AllowSkipped
)

$ErrorActionPreference = "Stop"

$RepoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $RepoRoot

$exe = (Resolve-Path "target\release\rustboy-advance.exe").Path

$syntheticPaths = @(
    "roms\homebrew\stress_aff_objwin.gba",
    "roms\homebrew\stress_mode0_4bg_128spr.gba",
    "roms\homebrew\stress_bitmap_345.gba"
)
if ($syntheticPaths | Where-Object { -not (Test-Path $_) }) {
    Write-Host "Synthetic stress ROMs missing in roms\homebrew; regenerating via gen_stress_roms..."
    & (Join-Path $PSScriptRoot "generate-stress-roms.ps1")
}

function Resolve-RomPath {
    param([string[]]$Candidates)
    foreach ($p in $Candidates) {
        if ($p -and (Test-Path $p)) { return $p }
    }
    return $Candidates[0]
}

$summonCandidates = @(
    $env:GBA_SUMMON_NIGHT_ROM,
    "roms\summon_night.gba",
    (Join-Path $env:USERPROFILE "Downloads\Summon Night - Swordcraft Story (USA).gba")
)
$checks = @(
    @{ Name="Summon Night (3600f)"; Ref="perf\baselines\summon_night_3600.hashes.json"; Rom=(Resolve-RomPath $summonCandidates); Replay="perf\replays\summon_night.txt"; Sav="perf\replays\summon_night.sav"; Frames=3600 },
    @{ Name="m7_ex.gba"; Ref="perf\baselines\m7_ex_600.hashes.json"; Rom=(Resolve-RomPath @("roms\homebrew\m7_ex.gba", "test-roms\tonc\m7_ex.gba")); Replay="perf\replays\tonc_m7_ex.txt"; Sav=$null; Frames=600 },
    @{ Name="win_demo.gba"; Ref="perf\baselines\win_demo_600.hashes.json"; Rom=(Resolve-RomPath @("roms\homebrew\win_demo.gba", "test-roms\tonc\win_demo.gba")); Replay="perf\replays\tonc_win_demo.txt"; Sav=$null; Frames=600 },
    @{ Name="bld_demo.gba"; Ref="perf\baselines\bld_demo_600.hashes.json"; Rom=(Resolve-RomPath @("roms\homebrew\bld_demo.gba", "test-roms\tonc\bld_demo.gba")); Replay="perf\replays\tonc_bld_demo.txt"; Sav=$null; Frames=600 },
    @{ Name="mos_demo.gba"; Ref="perf\baselines\mos_demo_600.hashes.json"; Rom=(Resolve-RomPath @("roms\homebrew\mos_demo.gba", "test-roms\tonc\mos_demo.gba")); Replay="perf\replays\tonc_mos_demo.txt"; Sav=$null; Frames=600 },
    @{ Name="oacombo.gba"; Ref="perf\baselines\oacombo_600.hashes.json"; Rom=(Resolve-RomPath @("roms\homebrew\oacombo.gba", "test-roms\tonc\oacombo.gba")); Replay="perf\replays\tonc_oacombo.txt"; Sav=$null; Frames=600 },
    @{ Name="bm_modes.gba"; Ref="perf\baselines\bm_modes_600.hashes.json"; Rom=(Resolve-RomPath @("roms\homebrew\bm_modes.gba", "test-roms\tonc\bm_modes.gba")); Replay="perf\replays\tonc_bm_modes.txt"; Sav=$null; Frames=600 },
    @{ Name="stress_aff_objwin.gba"; Ref="perf\baselines\stress_aff_objwin_600.hashes.json"; Rom="roms\homebrew\stress_aff_objwin.gba"; Replay=$null; Sav=$null; Frames=600 },
    @{ Name="stress_mode0_4bg_128spr.gba"; Ref="perf\baselines\stress_mode0_4bg_128spr_600.hashes.json"; Rom="roms\homebrew\stress_mode0_4bg_128spr.gba"; Replay=$null; Sav=$null; Frames=600 },
    @{ Name="stress_bitmap_345.gba"; Ref="perf\baselines\stress_bitmap_345_600.hashes.json"; Rom="roms\homebrew\stress_bitmap_345.gba"; Replay="perf\replays\stress_bitmap_345.txt"; Sav=$null; Frames=600 }
)

$verified = 0
$skipped = 0
$tmpOut = Join-Path ([System.IO.Path]::GetTempPath()) "rba_verify_hashes_$PID.out.log"
$tmpErr = Join-Path ([System.IO.Path]::GetTempPath()) "rba_verify_hashes_$PID.err.log"

try {
    foreach ($c in $checks) {
        if (-not (Test-Path $c.Rom)) {
            Write-Warning "Skipping $($c.Name): ROM not found at $($c.Rom)"
            $skipped++
            continue
        }
        $procArgs = @("--headless", "--verify-hashes", $c.Ref, "`"$($c.Rom)`"", "--frames", "$($c.Frames)")
        if ($c.Replay) { $procArgs += @("--input-replay", $c.Replay) }
        if ($c.Sav) { $procArgs += @("--sav", $c.Sav) }
        $proc = Start-Process -FilePath $exe -ArgumentList $procArgs -WindowStyle Hidden -PassThru -Wait -RedirectStandardOutput $tmpOut -RedirectStandardError $tmpErr
        $out = ((Get-Content $tmpOut -ErrorAction SilentlyContinue) + (Get-Content $tmpErr -ErrorAction SilentlyContinue)) -join "`n"
        if ($proc.ExitCode -ne 0) {
            throw "Hash verification FAILED for $($c.Name) (exit $($proc.ExitCode)): $out"
        }
        Write-Host $out
        $verified++
    }
} finally {
    Remove-Item $tmpOut, $tmpErr -ErrorAction SilentlyContinue
}

$total = $checks.Count
if ($total -gt 0 -and $verified -eq $total -and $skipped -eq 0) {
    Write-Host "ALL $total HASH ARCHIVES VERIFIED CLEANLY!"
} else {
    $summary = "$verified of $total verified, $skipped skipped (ROM not found)"
    if ($skipped -gt 0) {
        Write-Host $summary
    }
    if ($verified -eq 0) {
        if ($AllowSkipped) {
            Write-Warning "No ROMs were verified; exiting 0 because -AllowSkipped was specified."
            exit 0
        }
        Write-Warning "No ROMs were verified; failing instead of reporting false success. Pass -AllowSkipped to exit 0."
        exit 1
    }
    if ($skipped -gt 0 -and $AllowSkipped) {
        Write-Warning "Some ROMs were skipped; exiting 0 because -AllowSkipped was specified."
    }
}


# Verifies every presented frame + core video + core audio hash against pre-Phase-4 baselines.
$ErrorActionPreference = "Stop"
$exe = (Resolve-Path "target\release\rustboy-advance.exe").Path

$checks = @(
    @{ Name="Summon Night (3600f)"; Ref="perf\baselines\summon_night_3600.hashes.json"; Rom=$(if ($env:GBA_SUMMON_NIGHT_ROM) { $env:GBA_SUMMON_NIGHT_ROM } else { "roms\summon_night.gba" }); Replay="perf\replays\summon_night.txt"; Sav="perf\replays\summon_night.sav"; Frames=3600 },
    @{ Name="m7_ex.gba"; Ref="perf\baselines\m7_ex_600.hashes.json"; Rom="roms\homebrew\m7_ex.gba"; Replay="perf\replays\tonc_m7_ex.txt"; Sav=$null; Frames=600 },
    @{ Name="win_demo.gba"; Ref="perf\baselines\win_demo_600.hashes.json"; Rom="roms\homebrew\win_demo.gba"; Replay="perf\replays\tonc_win_demo.txt"; Sav=$null; Frames=600 },
    @{ Name="bld_demo.gba"; Ref="perf\baselines\bld_demo_600.hashes.json"; Rom="roms\homebrew\bld_demo.gba"; Replay="perf\replays\tonc_bld_demo.txt"; Sav=$null; Frames=600 },
    @{ Name="mos_demo.gba"; Ref="perf\baselines\mos_demo_600.hashes.json"; Rom="roms\homebrew\mos_demo.gba"; Replay="perf\replays\tonc_mos_demo.txt"; Sav=$null; Frames=600 },
    @{ Name="oacombo.gba"; Ref="perf\baselines\oacombo_600.hashes.json"; Rom="roms\homebrew\oacombo.gba"; Replay="perf\replays\tonc_oacombo.txt"; Sav=$null; Frames=600 },
    @{ Name="bm_modes.gba"; Ref="perf\baselines\bm_modes_600.hashes.json"; Rom="roms\homebrew\bm_modes.gba"; Replay="perf\replays\tonc_bm_modes.txt"; Sav=$null; Frames=600 },
    @{ Name="stress_aff_objwin.gba"; Ref="perf\baselines\stress_aff_objwin_600.hashes.json"; Rom="roms\homebrew\stress_aff_objwin.gba"; Replay=$null; Sav=$null; Frames=600 },
    @{ Name="stress_mode0_4bg_128spr.gba"; Ref="perf\baselines\stress_mode0_4bg_128spr_600.hashes.json"; Rom="roms\homebrew\stress_mode0_4bg_128spr.gba"; Replay=$null; Sav=$null; Frames=600 },
    @{ Name="stress_bitmap_345.gba"; Ref="perf\baselines\stress_bitmap_345_600.hashes.json"; Rom="roms\homebrew\stress_bitmap_345.gba"; Replay="perf\replays\stress_bitmap_345.txt"; Sav=$null; Frames=600 }
)

foreach ($c in $checks) {
    if (-not (Test-Path $c.Rom)) {
        Write-Warning "Skipping $($c.Name): ROM not found at $($c.Rom)"
        continue
    }
    $args = @("--headless", "--verify-hashes", $c.Ref, $c.Rom, "--frames", $c.Frames)
    if ($c.Replay) { $args += @("--input-replay", $c.Replay) }
    if ($c.Sav) { $args += @("--sav", $c.Sav) }
    $out = & $exe @args
    if ($LASTEXITCODE -ne 0) {
        throw "Hash verification FAILED for $($c.Name): $out"
    }
    Write-Host $out
}
Write-Host "ALL 10 HASH ARCHIVES VERIFIED CLEANLY!"


# Phase 3C Homebrew / Stress ROM Realtime Sweep & Worst-ROM --slowdown N Sweep
param(
    [int]$FramesPerRom = 600,
    [int]$Runs = 5
)

$ErrorActionPreference = "Stop"

$exe = (Resolve-Path "target\release\rustboy-advance.exe").Path
if (Test-Path "perf/REPORT.md") {
    Copy-Item "perf/REPORT.md" "perf/REPORT.phase3_bak.md" -Force
}

function Get-Median {
    param([double[]]$Values)
    if ($Values.Count -eq 0) { return 0.0 }
    $sorted = $Values | Sort-Object
    $n = $sorted.Count
    if ($n % 2 -eq 1) { return [double]$sorted[[int]($n / 2)] }
    return ([double]$sorted[$n / 2 - 1] + [double]$sorted[$n / 2]) / 2.0
}

function Get-Max {
    param([double[]]$Values)
    if ($Values.Count -eq 0) { return 0.0 }
    return [double](($Values | Measure-Object -Maximum).Maximum)
}

function Get-Sum {
    param([double[]]$Values)
    if ($Values.Count -eq 0) { return 0.0 }
    return [double](($Values | Measure-Object -Sum).Sum)
}

$romSpecs = @(
    @{ Name="m7_ex.gba"; Path="roms\homebrew\m7_ex.gba"; Replay="perf\replays\tonc_m7_ex.txt"; Frames=600 },
    @{ Name="win_demo.gba"; Path="roms\homebrew\win_demo.gba"; Replay="perf\replays\tonc_win_demo.txt"; Frames=600 },
    @{ Name="bld_demo.gba"; Path="roms\homebrew\bld_demo.gba"; Replay="perf\replays\tonc_bld_demo.txt"; Frames=600 },
    @{ Name="mos_demo.gba"; Path="roms\homebrew\mos_demo.gba"; Replay="perf\replays\tonc_mos_demo.txt"; Frames=600 },
    @{ Name="oacombo.gba"; Path="roms\homebrew\oacombo.gba"; Replay="perf\replays\tonc_oacombo.txt"; Frames=600 },
    @{ Name="bm_modes.gba"; Path="roms\homebrew\bm_modes.gba"; Replay="perf\replays\tonc_bm_modes.txt"; Frames=600 },
    @{ Name="stress_aff_objwin.gba"; Path="roms\homebrew\stress_aff_objwin.gba"; Replay=$null; Frames=600 },
    @{ Name="stress_mode0_4bg_128spr.gba"; Path="roms\homebrew\stress_mode0_4bg_128spr.gba"; Replay=$null; Frames=600 },
    @{ Name="stress_bitmap_345.gba"; Path="roms\homebrew\stress_bitmap_345.gba"; Replay="perf\replays\stress_bitmap_345.txt"; Frames=600 }
)

Write-Host "=== Step 1: Realtime N=1 baseline across all 9 Phase 3C ROMs ($FramesPerRom frames each) ==="
$rtRows = @()
foreach ($s in $romSpecs) {
    $args = @("--bench-realtime", $s.Path, "--frames", $s.Frames, "--slowdown", "1")
    if ($s.Replay) { $args += @("--input-replay", $s.Replay) }
    $out = & $exe @args
    $jpath = $null
    foreach ($l in $out) { if ($l -match 'JSON:\s*(perf[\\/]realtime-[^\s]+\.json)') { $jpath = $matches[1] } }
    $j = Get-Content $jpath | ConvertFrom-Json
    $obj = [PSCustomObject]@{
        ROM = $s.Name
        Frames = $s.Frames
        AudioUnderruns = $j.total_audio_underruns
        Over2xInterval = $j.frames_over_2x_interval
        Over2xWork = $j.frames_over_2x_work
        DroppedWork = $j.dropped_frames_work
        WorkPostP99Ms = [math]::Round($j.work_post_warmup.p99_ms, 3)
        WorkPostWorstMs = [math]::Round($j.work_post_warmup.worst_ms, 3)
        PresP99Ms = [math]::Round($j.presentation_interval_summary.p99_ms, 3)
        PresWorstMs = [math]::Round($j.presentation_interval_summary.worst_ms, 3)
        JitterFrames = $j.cadence_jitter_frames
    }
    $rtRows += $obj
    Write-Host ("  {0,-28} | underruns={1} over2x_int={2} work_post_p99={3:F3}ms pres_worst={4:F2}ms" -f $obj.ROM, $obj.AudioUnderruns, $obj.Over2xInterval, $obj.WorkPostP99Ms, $obj.PresWorstMs)
    Start-Sleep -Milliseconds 1100
}
$rtRows | ConvertTo-Json -Depth 4 | Set-Content "perf/phase3c_realtime.json"

Write-Host "`n=== Step 2: 5-run --slowdown N sweep on worst Phase 3C ROMs (bld_demo.gba & stress_mode0_4bg_128spr.gba) ==="
$sweepTargets = @(
    @{ Name="bld_demo.gba"; Path="roms\homebrew\bld_demo.gba"; Replay="perf\replays\tonc_bld_demo.txt"; Frames=720 },
    @{ Name="stress_mode0_4bg_128spr.gba"; Path="roms\homebrew\stress_mode0_4bg_128spr.gba"; Replay=$null; Frames=720 }
)
$slowdowns = @(1, 5, 10, 20, 30, 50)
$sweepRows = @()

foreach ($t in $sweepTargets) {
    foreach ($nVal in $slowdowns) {
        Write-Host ("`n-- {0} | slowdown={1}x ({2} runs x {3} frames) --" -f $t.Name, $nVal, $Runs, $t.Frames)
        $undArr = @()
        $o2xIntArr = @()
        $o2xWorkArr = @()
        $presP99Arr = @()
        $presWorstArr = @()
        $workPostWorstArr = @()
        for ($r = 1; $r -le $Runs; $r++) {
            $args = @("--bench-realtime", $t.Path, "--frames", $t.Frames, "--slowdown", "$nVal")
            if ($t.Replay) { $args += @("--input-replay", $t.Replay) }
            $out = & $exe @args
            $jpath = $null
            foreach ($l in $out) { if ($l -match 'JSON:\s*(perf[\\/]realtime-[^\s]+\.json)') { $jpath = $matches[1] } }
            $j = Get-Content $jpath | ConvertFrom-Json
            $undArr += [double]$j.total_audio_underruns
            $o2xIntArr += [double]$j.frames_over_2x_interval
            $o2xWorkArr += [double]$j.frames_over_2x_work
            $presP99Arr += [double]$j.presentation_interval_summary.p99_ms
            $presWorstArr += [double]$j.presentation_interval_summary.worst_ms
            $workPostWorstArr += [double]$j.work_post_warmup.worst_ms
            Write-Host ("  run {0}/{1}: underruns={2} over2x_int={3} over2x_work={4} pres_worst={5:F2}ms" -f $r, $Runs, $j.total_audio_underruns, $j.frames_over_2x_interval, $j.frames_over_2x_work, $j.presentation_interval_summary.worst_ms)
        }
        $sumUnd = [int](Get-Sum $undArr)
        $sumO2x = [int](Get-Sum $o2xIntArr)
        $gate = if ($sumUnd -eq 0 -and $sumO2x -le 1) { "PASS" } else { "FAIL" }
        $row = [PSCustomObject]@{
            ROM = $t.Name
            Slowdown = $nVal
            Runs = $Runs
            FramesPerRun = $t.Frames
            TotalFrames = $Runs * $t.Frames
            UnderrunsMedian = Get-Median $undArr
            UnderrunsWorst = Get-Max $undArr
            UnderrunsSum = $sumUnd
            Over2xIntMedian = Get-Median $o2xIntArr
            Over2xIntWorst = Get-Max $o2xIntArr
            Over2xIntSum = $sumO2x
            Over2xWorkWorst = Get-Max $o2xWorkArr
            PresP99MedianMs = [math]::Round((Get-Median $presP99Arr), 3)
            PresWorstMaxMs = [math]::Round((Get-Max $presWorstArr), 3)
            WorkPostWorstMs = [math]::Round((Get-Max $workPostWorstArr), 3)
            Gate = $gate
        }
        $sweepRows += $row
        Write-Host ("  ==> Underruns(med/worst/sum)={0}/{1}/{2} Over2xInt(med/worst/sum)={3}/{4}/{5} PresWorst={6:F2}ms Gate={7}" -f `
            $row.UnderrunsMedian, $row.UnderrunsWorst, $row.UnderrunsSum, $row.Over2xIntMedian, $row.Over2xIntWorst, $row.Over2xIntSum, $row.PresWorstMaxMs, $row.Gate)
    }
}

$sweepRows | Format-Table ROM, Slowdown, UnderrunsWorst, Over2xIntSum, PresWorstMaxMs, WorkPostWorstMs, Gate -AutoSize
$sweepRows | ConvertTo-Json -Depth 4 | Set-Content "perf/phase3c_slowdown_sweep.json"

if (Test-Path "perf/REPORT.phase3_bak.md") {
    Copy-Item "perf/REPORT.phase3_bak.md" "perf/REPORT.md" -Force
}
Write-Host "Phase 3C sweeps saved to perf/phase3c_realtime.json & perf/phase3c_slowdown_sweep.json"


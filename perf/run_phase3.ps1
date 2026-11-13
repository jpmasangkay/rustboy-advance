# Phase 3 Weak Hardware Simulation Benchmark Runner
$ErrorActionPreference = "Stop"

$rom = if ($env:GBA_BENCH_ROM) { $env:GBA_BENCH_ROM } else { "roms/summon_night.gba" }
$replay = "perf/replays/summon_night.txt"
$sav = "perf/replays/summon_night.sav"
$frames = 600
$gba_fps = 59.7275

function Restore-PowerPlan {
    Write-Host "Restoring power plan to 100%..."
    powercfg /setacvalueindex SCHEME_CURRENT SUB_PROCESSOR PROCTHROTTLEMAX 100
    powercfg /setactive SCHEME_CURRENT
}

function Kill-Workers($workers) {
    if ($workers) {
        foreach ($w in $workers) {
            try { Stop-Process -Id $w.Id -Force -ErrorAction SilentlyContinue } catch {}
        }
    }
}

$tests = @(
    @{ Name = "Core0 Unthrottled (100%)"; PowerPct = 100; Workers = 0 },
    @{ Name = "Core0 Powercfg 25%";       PowerPct = 25;  Workers = 0 },
    @{ Name = "Core0 Powercfg 15%";       PowerPct = 15;  Workers = 0 },
    @{ Name = "Core0 Powercfg 5%";        PowerPct = 5;   Workers = 0 },
    @{ Name = "Core0 + 1 Busy Worker";    PowerPct = 100; Workers = 1 },
    @{ Name = "Core0 + 2 Busy Workers";   PowerPct = 100; Workers = 2 },
    @{ Name = "Core0 + 3 Busy Workers";   PowerPct = 100; Workers = 3 },
    @{ Name = "Core0 + 4 Busy Workers";   PowerPct = 100; Workers = 4 }
)

$results = @()

try {
    foreach ($t in $tests) {
        Write-Host "`n======================================================="
        Write-Host "Running Setting: $($t.Name)"
        Write-Host "======================================================="

        # Apply power setting
        powercfg /setacvalueindex SCHEME_CURRENT SUB_PROCESSOR PROCTHROTTLEMAX $t.PowerPct
        powercfg /setactive SCHEME_CURRENT

        # Spawn workers if any
        $workerProcs = @()
        for ($i = 0; $i -lt $t.Workers; $i++) {
            $w = Start-Process -FilePath "powershell.exe" -ArgumentList "-NoProfile", "-NonInteractive", "-Command", "while(`$true){}" -PassThru -WindowStyle Hidden
            $w.ProcessorAffinity = [IntPtr]1
            $workerProcs += $w
        }
        Start-Sleep -Milliseconds 300

        try {
            # 1. Unthrottled Bench (600 frames)
            $cmd = "start /affinity 1 /b /wait target\release\rustboy-advance.exe --bench `"$rom`" --input-replay `"$replay`" --sav `"$sav`" --frames $frames"
            $outBench = cmd /c $cmd
            $benchPath = $null
            foreach ($line in $outBench) {
                if ($line -match 'JSON:\s*(perf[\\/]bench-[^\s]+\.json)') {
                    $benchPath = $matches[1]
                }
            }
            if (-not $benchPath -or -not (Test-Path $benchPath)) {
                $benchFiles = Get-ChildItem "perf" -Filter "bench-*.json" | Sort-Object LastWriteTime -Descending
                $benchPath = $benchFiles[0].FullName
            }
            $benchJson = Get-Content $benchPath | ConvertFrom-Json
            $unthrottled_fps = $benchJson.overall.avg_fps
            $headroom = [math]::Round($unthrottled_fps / $gba_fps, 2)
            Write-Host "  [Unthrottled] FPS: $([math]::Round($unthrottled_fps, 2)) | Headroom: ${headroom}x"

            # 2. Realtime Paced Bench (600 frames)
            $cmdRt = "start /affinity 1 /b /wait target\release\rustboy-advance.exe --bench-realtime `"$rom`" --input-replay `"$replay`" --sav `"$sav`" --frames $frames"
            $outRt = cmd /c $cmdRt
            $rtPath = $null
            foreach ($line in $outRt) {
                if ($line -match 'JSON:\s*(perf[\\/]realtime-[^\s]+\.json)') {
                    $rtPath = $matches[1]
                }
            }
            if (-not $rtPath -or -not (Test-Path $rtPath)) {
                $rtFiles = Get-ChildItem "perf" -Filter "realtime-*.json" | Sort-Object LastWriteTime -Descending
                $rtPath = $rtFiles[0].FullName
            }
            $rtJson = Get-Content $rtPath | ConvertFrom-Json

            $underruns = $rtJson.total_audio_underruns
            $over2x = $rtJson.frames_over_2x_interval
            $droppedWork = $rtJson.dropped_frames_work
            $droppedDead = $rtJson.dropped_frames_deadline
            $jitterFrames = $rtJson.cadence_jitter_frames
            $presP50 = $rtJson.presentation_interval_summary.p50_ms
            $presP99 = $rtJson.presentation_interval_summary.p99_ms
            $presWorst = $rtJson.presentation_interval_summary.worst_ms
            $workP99 = $rtJson.work_post_warmup.p99_ms
            $workWorst = $rtJson.work_post_warmup.worst_ms

            $status = if ($underruns -eq 0 -and $over2x -eq 0) { "PASS" } else { "FAIL" }
            Write-Host "  [Realtime] Underruns: $underruns | Over2x: $over2x | PresP99: $presP99 ms | PresWorst: $presWorst ms | JitterCount: $jitterFrames | Status: $status"

            $results += [PSCustomObject]@{
                Setting = $t.Name
                PowerPct = $t.PowerPct
                Workers = $t.Workers
                UnthrottledFps = [math]::Round($unthrottled_fps, 2)
                Headroom = $headroom
                AudioUnderruns = $underruns
                DroppedWork = $droppedWork
                DroppedDeadline = $droppedDead
                Over2xInterval = $over2x
                WorkPostWarmupP99 = [math]::Round($workP99, 3)
                WorkPostWarmupWorst = [math]::Round($workWorst, 3)
                PresIntervalP50 = [math]::Round($presP50, 3)
                PresIntervalP99 = [math]::Round($presP99, 3)
                PresIntervalWorst = [math]::Round($presWorst, 3)
                CadenceJitterFrames = $jitterFrames
                Status = $status
            }
        } finally {
            Kill-Workers $workerProcs
        }
    }
} finally {
    Restore-PowerPlan
}

$results | Format-Table -AutoSize
$results | ConvertTo-Json -Depth 4 | Set-Content "perf/weak_hardware.json"
Write-Host "`nWeak hardware simulation complete. Saved to perf/weak_hardware.json"

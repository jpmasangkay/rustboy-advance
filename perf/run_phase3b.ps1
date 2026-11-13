# Phase 3B Real Throttle Validation & --slowdown N Sweep (5 runs per condition)
param(
    [int]$FramesPerRun = 720,
    [int]$Runs = 5
)

$ErrorActionPreference = "Stop"

$exe = (Resolve-Path "target\release\rustboy-advance.exe").Path
$rom = "C:\Users\My Laptop\Downloads\Summon Night - Swordcraft Story (USA).gba"
$replay = "perf/replays/summon_night.txt"
$sav = "perf/replays/summon_night.sav"
$gbaFps = 59.72750056960583

if (-not (Test-Path $exe)) {
    throw "Missing release binary at $exe"
}
if (Test-Path "perf/REPORT.md") {
    Copy-Item "perf/REPORT.md" "perf/REPORT.phase3_bak.md" -Force
}

function Set-CpuPowerState {
    param(
        [int]$MinPct,
        [int]$MaxPct,
        [int]$BoostMode,
        [int]$MinEff1 = 5,
        [int]$MinEff2 = 5
    )
    powercfg /setacvalueindex SCHEME_CURRENT SUB_PROCESSOR PROCTHROTTLEMIN $MinPct | Out-Null
    powercfg /setacvalueindex SCHEME_CURRENT SUB_PROCESSOR PROCTHROTTLEMIN1 $MinEff1 | Out-Null
    powercfg /setacvalueindex SCHEME_CURRENT SUB_PROCESSOR PROCTHROTTLEMIN2 $MinEff2 | Out-Null
    powercfg /setacvalueindex SCHEME_CURRENT SUB_PROCESSOR PROCTHROTTLEMAX $MaxPct | Out-Null
    powercfg /setacvalueindex SCHEME_CURRENT SUB_PROCESSOR PROCTHROTTLEMAX1 $MaxPct | Out-Null
    powercfg /setacvalueindex SCHEME_CURRENT SUB_PROCESSOR PROCTHROTTLEMAX2 $MaxPct | Out-Null
    powercfg /setacvalueindex SCHEME_CURRENT SUB_PROCESSOR PERFBOOSTMODE $BoostMode | Out-Null
    powercfg /setactive SCHEME_CURRENT | Out-Null
}

function Restore-PowerPlan {
    Write-Host "Restoring original AC power plan (MIN=80, MIN1=5, MIN2=5, MAX=100, BOOST=2)..."
    Set-CpuPowerState -MinPct 80 -MaxPct 100 -BoostMode 2 -MinEff1 5 -MinEff2 5
}

function Kill-Workers($workers) {
    if ($workers) {
        foreach ($w in $workers) {
            try { Stop-Process -Id $w.Id -Force -ErrorAction SilentlyContinue } catch {}
        }
    }
}

function Run-CalibrateCore0 {
    param([bool]$PinCore0 = $true)
    $cmd = if ($PinCore0) {
        "start `"`" /affinity 1 /b /wait `"$exe`" --calibrate"
    } else {
        "`"$exe`" --calibrate"
    }
    $out = cmd /c $cmd
    $ms = $null
    $ck = $null
    foreach ($line in $out) {
        if ($line -match 'CALIBRATION_MS=([0-9.]+)\s+CHECKSUM=(0x[0-9a-fA-F]+)') {
            $ms = [double]$matches[1]
            $ck = $matches[2]
        }
    }
    if ($null -eq $ms) {
        throw "Calibration failed to parse output: $out"
    }
    return @{ Ms = $ms; Checksum = $ck }
}

function Measure-ActivePerfPct {
    param([bool]$PinCore0 = $true)
    # Run short CPU load during typeperf sampling so C-states do not pull down frequency reading
    $loader = Start-Process -FilePath "powershell.exe" -ArgumentList "-NoProfile", "-NonInteractive", "-Command", "1..20 | % { & '$exe' --calibrate | Out-Null }" -PassThru -WindowStyle Hidden
    if ($PinCore0) {
        try { $loader.ProcessorAffinity = [IntPtr]1 } catch {}
    }
    Start-Sleep -Milliseconds 150
    try {
        $tp = typeperf "\Processor Information(_Total)\% Processor Performance" -sc 1 2>$null
        $pct = $null
        foreach ($line in $tp) {
            if ($line -match '"([0-9]+\.[0-9]+)"\s*$') {
                $pct = [double]$matches[1]
            }
        }
        return $pct
    } finally {
        try { Stop-Process -Id $loader.Id -Force -ErrorAction SilentlyContinue } catch {}
    }
}

function Get-Median {
    param([double[]]$Values)
    if ($Values.Count -eq 0) { return 0.0 }
    $sorted = $Values | Sort-Object
    $n = $sorted.Count
    if ($n % 2 -eq 1) {
        return [double]$sorted[[int]($n / 2)]
    } else {
        return ([double]$sorted[$n / 2 - 1] + [double]$sorted[$n / 2]) / 2.0
    }
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

$conditions = @(
    @{ Group = "powercfg"; Name = "Core0 Baseline (Boost ON, 100%)"; PinCore0 = $true; MinPct = 80; MaxPct = 100; Boost = 2; Workers = 0; Slowdown = 1; ExpectedScale = 1.0 },
    @{ Group = "powercfg"; Name = "Core0 Boost OFF (MIN=5 MAX=100)"; PinCore0 = $true; MinPct = 5; MaxPct = 100; Boost = 0; Workers = 0; Slowdown = 1; ExpectedScale = 1.0 },
    @{ Group = "powercfg"; Name = "Core0 Powercfg 25% (MIN=5 MAX=25 BOOST=0)"; PinCore0 = $true; MinPct = 5; MaxPct = 25; Boost = 0; Workers = 0; Slowdown = 1; ExpectedScale = 4.0 },
    @{ Group = "powercfg"; Name = "Core0 Powercfg 15% (MIN=5 MAX=15 BOOST=0)"; PinCore0 = $true; MinPct = 5; MaxPct = 15; Boost = 0; Workers = 0; Slowdown = 1; ExpectedScale = 6.67 },
    @{ Group = "powercfg"; Name = "Core0 Powercfg 5% (MIN=5 MAX=5 BOOST=0)"; PinCore0 = $true; MinPct = 5; MaxPct = 5; Boost = 0; Workers = 0; Slowdown = 1; ExpectedScale = 20.0 },
    @{ Group = "contention"; Name = "Core0 + 2 Busy Workers (Boost ON)"; PinCore0 = $true; MinPct = 80; MaxPct = 100; Boost = 2; Workers = 2; Slowdown = 1; ExpectedScale = 3.0 },
    @{ Group = "contention"; Name = "Core0 + 4 Busy Workers (Boost ON)"; PinCore0 = $true; MinPct = 80; MaxPct = 100; Boost = 2; Workers = 4; Slowdown = 1; ExpectedScale = 5.0 },
    @{ Group = "slowdown"; Name = "Slowdown 1x (Baseline)"; PinCore0 = $false; MinPct = 80; MaxPct = 100; Boost = 2; Workers = 0; Slowdown = 1; ExpectedScale = 1.0 },
    @{ Group = "slowdown"; Name = "Slowdown 5x"; PinCore0 = $false; MinPct = 80; MaxPct = 100; Boost = 2; Workers = 0; Slowdown = 5; ExpectedScale = 5.0 },
    @{ Group = "slowdown"; Name = "Slowdown 10x"; PinCore0 = $false; MinPct = 80; MaxPct = 100; Boost = 2; Workers = 0; Slowdown = 10; ExpectedScale = 10.0 },
    @{ Group = "slowdown"; Name = "Slowdown 20x"; PinCore0 = $false; MinPct = 80; MaxPct = 100; Boost = 2; Workers = 0; Slowdown = 20; ExpectedScale = 20.0 },
    @{ Group = "slowdown"; Name = "Slowdown 30x"; PinCore0 = $false; MinPct = 80; MaxPct = 100; Boost = 2; Workers = 0; Slowdown = 30; ExpectedScale = 30.0 },
    @{ Group = "slowdown"; Name = "Slowdown 50x"; PinCore0 = $false; MinPct = 80; MaxPct = 100; Boost = 2; Workers = 0; Slowdown = 50; ExpectedScale = 50.0 }
)

$results = @()
$baselineCalibMs = $null
$boostOff100CalibMs = $null

try {
    foreach ($c in $conditions) {
        Write-Host "`n================================================================"
        Write-Host "Condition: $($c.Name) ($Runs runs x $FramesPerRun frames)"
        Write-Host "================================================================"

        Set-CpuPowerState -MinPct $c.MinPct -MaxPct $c.MaxPct -BoostMode $c.Boost
        $workerProcs = @()
        for ($i = 0; $i -lt $c.Workers; $i++) {
            $w = Start-Process -FilePath "powershell.exe" -ArgumentList "-NoProfile", "-NonInteractive", "-Command", "while(`$true){}" -PassThru -WindowStyle Hidden
            try { $w.ProcessorAffinity = [IntPtr]1 } catch {}
            $workerProcs += $w
        }
        Start-Sleep -Milliseconds 250

        try {
            # Unthrottled bench once for non-slowdown conditions (to measure true FPS/headroom)
            $unthrottledFps = $null
            if ($c.Slowdown -eq 1) {
                $bCmd = if ($c.PinCore0) {
                    "start `"`" /affinity 1 /b /wait `"$exe`" --bench `"$rom`" --input-replay `"$replay`" --sav `"$sav`" --frames 600"
                } else {
                    "`"$exe`" --bench `"$rom`" --input-replay `"$replay`" --sav `"$sav`" --frames 600"
                }
                $bOut = cmd /c $bCmd
                $bJsonPath = $null
                foreach ($line in $bOut) {
                    if ($line -match 'JSON:\s*(perf[\\/]bench-[^\s]+\.json)') {
                        $bJsonPath = $matches[1]
                    }
                }
                if ($bJsonPath -and (Test-Path $bJsonPath)) {
                    $bJson = Get-Content $bJsonPath | ConvertFrom-Json
                    $unthrottledFps = [double]$bJson.overall.avg_fps
                }
            }

            $calibSamples = @()
            $perfPctSamples = @()
            $underrunsArr = @()
            $over2xIntArr = @()
            $over2xWorkArr = @()
            $droppedWorkArr = @()
            $droppedDeadArr = @()
            $presP99Arr = @()
            $presWorstArr = @()
            $workPostWorstArr = @()
            $workPostAvgArr = @()
            $jitterArr = @()
            $runLogs = @()

            for ($r = 1; $r -le $Runs; $r++) {
                $cal = Run-CalibrateCore0 -PinCore0 $c.PinCore0
                $tpPct = Measure-ActivePerfPct -PinCore0 $c.PinCore0
                $calibSamples += $cal.Ms
                if ($null -ne $tpPct) { $perfPctSamples += $tpPct }

                $rtCmd = if ($c.PinCore0) {
                    "start `"`" /affinity 1 /b /wait `"$exe`" --bench-realtime `"$rom`" --input-replay `"$replay`" --sav `"$sav`" --frames $FramesPerRun --slowdown $($c.Slowdown)"
                } else {
                    "`"$exe`" --bench-realtime `"$rom`" --input-replay `"$replay`" --sav `"$sav`" --frames $FramesPerRun --slowdown $($c.Slowdown)"
                }
                $rtOut = cmd /c $rtCmd
                $rtJsonPath = $null
                foreach ($line in $rtOut) {
                    if ($line -match 'JSON:\s*(perf[\\/]realtime-[^\s]+\.json)') {
                        $rtJsonPath = $matches[1]
                    }
                }
                if (-not $rtJsonPath -or -not (Test-Path $rtJsonPath)) {
                    throw "Failed to locate realtime JSON for $($c.Name) run $r"
                }
                $rt = Get-Content $rtJsonPath | ConvertFrom-Json
                $underrunsArr += [double]$rt.total_audio_underruns
                $over2xIntArr += [double]$rt.frames_over_2x_interval
                $over2xWorkArr += [double]$rt.frames_over_2x_work
                $droppedWorkArr += [double]$rt.dropped_frames_work
                $droppedDeadArr += [double]$rt.dropped_frames_deadline
                $presP99Arr += [double]$rt.presentation_interval_summary.p99_ms
                $presWorstArr += [double]$rt.presentation_interval_summary.worst_ms
                $workPostWorstArr += [double]$rt.work_post_warmup.worst_ms
                $workPostAvgArr += [double]$rt.work_post_warmup.avg_ms
                $jitterArr += [double]$rt.cadence_jitter_frames
                $runLogs += $rtJsonPath

                Write-Host ("  run {0}/{1}: cal={2:F1}ms perf={3:F1}% | underruns={4} over2x_int={5} over2x_work={6} pres_worst={7:F2}ms" -f $r, $Runs, $cal.Ms, $tpPct, $rt.total_audio_underruns, $rt.frames_over_2x_interval, $rt.frames_over_2x_work, $rt.presentation_interval_summary.worst_ms)
            }

            $calibMed = Get-Median $calibSamples
            $calibMax = Get-Max $calibSamples
            $perfMed = if ($perfPctSamples.Count -gt 0) { Get-Median $perfPctSamples } else { 0.0 }
            if ($null -eq $baselineCalibMs) { $baselineCalibMs = $calibMed }
            if ($c.Name -like "*Boost OFF (MIN=5 MAX=100)*") { $boostOff100CalibMs = $calibMed }

            # Determine whether throttle actually applied proportionally
            $throttleStatus = "APPLIED"
            if ($c.Group -eq "powercfg" -and $c.MaxPct -lt 100) {
                $refMs = if ($null -ne $boostOff100CalibMs) { $boostOff100CalibMs } else { $baselineCalibMs }
                $actualScale = $calibMed / $refMs
                # Require at least 50% of expected slowdown vs 100% Boost OFF
                if ($actualScale -lt ($c.ExpectedScale * 0.5)) {
                    $throttleStatus = "NOT APPLIED"
                }
            } elseif ($c.Group -eq "powercfg" -and $c.Boost -eq 0) {
                $actualScale = $calibMed / $baselineCalibMs
                $throttleStatus = if ($actualScale -gt 1.25) { "BOOST_OFF_ONLY" } else { "NOT APPLIED" }
            } elseif ($c.Group -eq "slowdown" -and $c.Slowdown -gt 1) {
                $throttleStatus = "SYNTHETIC_$($c.Slowdown)X"
            }

            $effectiveWorkAvg = Get-Median $workPostAvgArr
            if ($c.Slowdown -gt 1) {
                # Synthetic N-times slower CPU effective headroom
                $unthrottledFps = if ($effectiveWorkAvg -gt 0) { 1000.0 / $effectiveWorkAvg } else { 0.0 }
            }
            $headroom = if ($null -ne $unthrottledFps) { [math]::Round($unthrottledFps / $gbaFps, 2) } else { 0.0 }

            $sumUnderruns = [int](Get-Sum $underrunsArr)
            $sumOver2xInt = [int](Get-Sum $over2xIntArr)
            $medUnderruns = Get-Median $underrunsArr
            $maxUnderruns = Get-Max $underrunsArr
            $medOver2xInt = Get-Median $over2xIntArr
            $maxOver2xInt = Get-Max $over2xIntArr
            $medOver2xWork = Get-Median $over2xWorkArr
            $maxOver2xWork = Get-Max $over2xWorkArr

            $totalAggregateFrames = $Runs * $FramesPerRun
            $passGate = ($sumUnderruns -eq 0) -and ($sumOver2xInt -le 1) -and ($throttleStatus -ne "NOT APPLIED")
            $gateReason = if ($throttleStatus -eq "NOT APPLIED") {
                "NOT APPLIED"
            } elseif ($sumUnderruns -eq 0 -and $sumOver2xInt -le 1) {
                "PASS"
            } else {
                "FAIL"
            }

            $obj = [PSCustomObject]@{
                Group = $c.Group
                Condition = $c.Name
                Slowdown = $c.Slowdown
                FramesPerRun = $FramesPerRun
                Runs = $Runs
                TotalFrames = $totalAggregateFrames
                ThrottleStatus = $throttleStatus
                CalibMedianMs = [math]::Round($calibMed, 2)
                CalibWorstMs = [math]::Round($calibMax, 2)
                TypeperfPerfPct = [math]::Round($perfMed, 1)
                UnthrottledFps = [math]::Round($unthrottledFps, 2)
                Headroom = $headroom
                UnderrunsMedian = $medUnderruns
                UnderrunsWorst = $maxUnderruns
                UnderrunsSum = $sumUnderruns
                Over2xIntMedian = $medOver2xInt
                Over2xIntWorst = $maxOver2xInt
                Over2xIntSum = $sumOver2xInt
                Over2xWorkMedian = $medOver2xWork
                Over2xWorkWorst = $maxOver2xWork
                DroppedWorkWorst = Get-Max $droppedWorkArr
                PresP99MedianMs = [math]::Round((Get-Median $presP99Arr), 3)
                PresP99WorstMs = [math]::Round((Get-Max $presP99Arr), 3)
                PresWorstMedianMs = [math]::Round((Get-Median $presWorstArr), 3)
                PresWorstMaxMs = [math]::Round((Get-Max $presWorstArr), 3)
                WorkPostWarmupWorstMs = [math]::Round((Get-Max $workPostWorstArr), 3)
                JitterWorst = Get-Max $jitterArr
                Gate = $gateReason
            }
            $results += $obj
            Write-Host ("  ==> CalibMed={0:F1}ms Perf={1:F1}% Status={2} | Underruns(med/worst/sum)={3}/{4}/{5} Over2xInt(med/worst/sum)={6}/{7}/{8} | Gate={9}" -f `
                $obj.CalibMedianMs, $obj.TypeperfPerfPct, $obj.ThrottleStatus, `
                $obj.UnderrunsMedian, $obj.UnderrunsWorst, $obj.UnderrunsSum, `
                $obj.Over2xIntMedian, $obj.Over2xIntWorst, $obj.Over2xIntSum, $obj.Gate)
        } finally {
            Kill-Workers $workerProcs
        }
    }
} finally {
    Restore-PowerPlan
    if (Test-Path "perf/REPORT.phase3_bak.md") {
        Copy-Item "perf/REPORT.phase3_bak.md" "perf/REPORT.md" -Force
    }
}

$results | Format-Table Condition, ThrottleStatus, CalibMedianMs, TypeperfPerfPct, UnthrottledFps, Headroom, UnderrunsWorst, Over2xIntSum, PresWorstMaxMs, Gate -AutoSize
$results | ConvertTo-Json -Depth 4 | Set-Content "perf/phase3b_results.json"
Write-Host "Saved perf/phase3b_results.json"

# Forwarding wrapper: canonical script lives in scripts/run-phase3b.ps1
[CmdletBinding()]
param(
    [int]$FramesPerRun = 720,
    [int]$Runs = 5
)
$ErrorActionPreference = "Stop"
& (Join-Path (Split-Path -Parent $PSScriptRoot) "scripts\run-phase3b.ps1") @PSBoundParameters
exit $LASTEXITCODE

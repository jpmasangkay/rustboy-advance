# Forwarding wrapper: canonical script lives in scripts/run-phase3c.ps1
[CmdletBinding()]
param(
    [int]$FramesPerRom = 600,
    [int]$Runs = 5
)
$ErrorActionPreference = "Stop"
& (Join-Path (Split-Path -Parent $PSScriptRoot) "scripts\run-phase3c.ps1") @PSBoundParameters
exit $LASTEXITCODE

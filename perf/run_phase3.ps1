# Forwarding wrapper: canonical script lives in scripts/run-phase3.ps1
$ErrorActionPreference = "Stop"
& (Join-Path (Split-Path -Parent $PSScriptRoot) "scripts\run-phase3.ps1") @args
exit $LASTEXITCODE

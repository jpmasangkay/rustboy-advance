# Forwarding wrapper: canonical script lives in scripts/verify-all-hashes.ps1
[CmdletBinding()]
param(
    [switch]$AllowSkipped
)
$ErrorActionPreference = "Stop"
& (Join-Path (Split-Path -Parent $PSScriptRoot) "scripts\verify-all-hashes.ps1") @PSBoundParameters
exit $LASTEXITCODE

[CmdletBinding()]
param(
    [switch]$Force
)

$ErrorActionPreference = "Stop"

$RepoRoot = Split-Path -Parent $PSScriptRoot
$OutRoot = Join-Path $RepoRoot "test-roms"

$JsmolkaSha = "a7113b67e63f83a9b321696ddd7042ccfad6c881"
$FuzzArmSha = "a675329cd57da48e3e406216ba2d79dd7e09ee20"
$ArmWrestlerSha = "802e55ad61b421f4bbbc6f74f96aa4e5df4d630f"
$NbaHwTestSha = "fbc99140e06f083c0a47612467cfbb02470e56dc"

$CurlExe = Get-Command curl.exe -ErrorAction SilentlyContinue

function Download-File {
    param(
        [string]$Url,
        [string]$DestPath
    )
    $parent = Split-Path -Parent $DestPath
    if (-not (Test-Path $parent)) {
        New-Item -ItemType Directory -Path $parent -Force | Out-Null
    }
    if ((Test-Path $DestPath) -and (-not $Force) -and ((Get-Item $DestPath).Length -gt 0)) {
        Write-Host "  [cached] $DestPath"
        return
    }
    Write-Host "  [fetch]  $Url -> $DestPath"
    $tmp = "$DestPath.tmp"
    if (Test-Path $tmp) { Remove-Item $tmp -Force }
    if ($CurlExe) {
        & $CurlExe.Source -fsSL --retry 3 --retry-delay 2 --max-time 60 -o $tmp $Url
        if ($LASTEXITCODE -ne 0) {
            throw "curl.exe failed ($LASTEXITCODE) downloading $Url"
        }
    } else {
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        Invoke-WebRequest -UseBasicParsing -Uri $Url -OutFile $tmp -TimeoutSec 60
    }
    if (-not (Test-Path $tmp) -or (Get-Item $tmp).Length -eq 0) {
        throw "Downloaded empty file from $Url"
    }
    Move-Item -Path $tmp -Destination $DestPath -Force
}

Write-Host "==> Fetching jsmolka/gba-tests ($JsmolkaSha)"
$JsmolkaBase = "https://raw.githubusercontent.com/jsmolka/gba-tests/$JsmolkaSha"
$JsmolkaFiles = @(
    @("arm/arm.gba", "jsmolka/arm.gba"),
    @("thumb/thumb.gba", "jsmolka/thumb.gba"),
    @("memory/memory.gba", "jsmolka/memory.gba"),
    @("nes/nes.gba", "jsmolka/nes.gba"),
    @("bios/bios.gba", "jsmolka/bios.gba"),
    @("unsafe/unsafe.gba", "jsmolka/unsafe.gba"),
    @("ppu/hello.gba", "jsmolka/ppu/hello.gba"),
    @("ppu/shades.gba", "jsmolka/ppu/shades.gba"),
    @("ppu/stripes.gba", "jsmolka/ppu/stripes.gba"),
    @("save/flash64.gba", "jsmolka/save/flash64.gba"),
    @("save/flash128.gba", "jsmolka/save/flash128.gba"),
    @("save/none.gba", "jsmolka/save/none.gba"),
    @("save/sram.gba", "jsmolka/save/sram.gba")
)
foreach ($entry in $JsmolkaFiles) {
    Download-File "$JsmolkaBase/$($entry[0])" (Join-Path $OutRoot $entry[1])
}

Write-Host "==> Fetching DenSinH/FuzzARM ($FuzzArmSha)"
$FuzzArmBase = "https://raw.githubusercontent.com/DenSinH/FuzzARM/$FuzzArmSha"
$FuzzArmFiles = @(
    "FuzzARM.gba",
    "ARM_Any.gba",
    "ARM_DataProcessing.gba",
    "THUMB_Any.gba",
    "THUMB_DataProcessing.gba"
)
foreach ($f in $FuzzArmFiles) {
    Download-File "$FuzzArmBase/$f" (Join-Path $OutRoot "fuzzarm/$f")
}

Write-Host "==> Fetching ARMWrestler ($ArmWrestlerSha)"
Download-File `
    "https://raw.githubusercontent.com/destoer/armwrestler-gba-fixed/$ArmWrestlerSha/armwrestler-gba-fixed.gba" `
    (Join-Path $OutRoot "armwrestler/armwrestler-gba-fixed.gba")

Write-Host "==> Fetching NanoBoyAdvance hw-test ($NbaHwTestSha)"
$NbaBase = "https://codeberg.org/nba-emu/hw-test/raw/commit/$NbaHwTestSha"
$NbaFiles = @(
    "bus/128kb-boundary/128kb-boundary.gba",
    "dma/burst-into-tears/burst-into-tears.gba",
    "dma/force-nseq-access/force-nseq-access.gba",
    "dma/latch/latch.gba",
    "dma/start-delay/start-delay.gba",
    "haltcnt/haltcnt.gba",
    "irq/irq-delay/irq-delay.gba",
    "ppu/bgpd/bgpd.gba",
    "ppu/bgpd/expected.png",
    "ppu/bgx/bgx.gba",
    "ppu/bgx/expected.png",
    "ppu/dispcnt-latch/dispcnt-latch.gba",
    "ppu/dispcnt-latch/expected.png",
    "ppu/greenswap/greenswap.gba",
    "ppu/greenswap/expected.png",
    "ppu/ram-access-timing/ram-access-timing.gba",
    "ppu/sprite-hmosaic/sprite-hmosaic.gba",
    "ppu/sprite-hmosaic/expected.png",
    "ppu/status-irq-dma/status-irq-dma.gba",
    "ppu/vram-mirror/vram-mirror.gba",
    "timer/reload/reload.gba",
    "timer/start-stop/start-stop.gba",
    "timer/tick-before-reload/tick-before-reload.gba"
)
foreach ($f in $NbaFiles) {
    Download-File "$NbaBase/$f" (Join-Path $OutRoot "nba-hw-test/$f")
}

Write-Host "==> Fetching Tonc prebuilt demos (tonc-bin.zip)"
$ToncDir = Join-Path $OutRoot "tonc"
$ToncMarker = Join-Path $ToncDir "win_demo.gba"
if ((-not (Test-Path $ToncMarker)) -or $Force) {
    if (-not (Test-Path $ToncDir)) {
        New-Item -ItemType Directory -Path $ToncDir -Force | Out-Null
    }
    $ToncZip = Join-Path $OutRoot "tonc-bin.zip"
    Download-File "https://www.coranac.com/files/tonc-bin.zip" $ToncZip
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [System.IO.Compression.ZipFile]::OpenRead($ToncZip)
    try {
        foreach ($entry in $zip.Entries) {
            if ($entry.FullName -match "\.gba$") {
                $name = Split-Path -Leaf $entry.FullName
                $dest = Join-Path $ToncDir $name
                [System.IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $dest, $true)
            }
        }
    } finally {
        $zip.Dispose()
    }
    Remove-Item $ToncZip -Force -ErrorAction SilentlyContinue
} else {
    Write-Host "  [cached] $ToncDir"
}

Write-Host ""
Write-Host "Note: mGBA suite (mgba-emu/suite) is source-only; see scripts/README.md for devkitARM build instructions (place at test-roms/mgba/suite.gba)."
Write-Host "Note: AGS Aging Cartridge is proprietary; place user-owned ROM at test-roms/ags/ags.gba if desired."
Write-Host "Done."

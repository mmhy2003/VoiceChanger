<#
.SYNOPSIS
Builds dist\Setup-VoiceChanger-<Version>.exe. CI's smoke build and release.yml both run it.
.EXAMPLE
pwsh -File installer\build.ps1 -Version 0.1.0-dev.42 -FileVersion 0.1.0.0
Needs target\release\voicechanger.exe (cargo build --release) and vendor\ (cargo xtask fetch-models).
#>
param(
    [Parameter(Mandatory = $true)] [ValidatePattern('^\d+\.\d+\.\d+(-dev\.\d+)?$')] [string] $Version,
    [Parameter(Mandatory = $true)] [ValidatePattern('^\d+\.\d+\.\d+\.0$')] [string] $FileVersion
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root 'target\release\voicechanger.exe'

$inputs = 'target\release\voicechanger.exe', 'vendor\models\hubert.onnx', 'vendor\models\rmvpe.onnx',
    'vendor\runtime\onnxruntime.dll', 'vendor\runtime\DirectML.dll', 'LICENSE', 'CHANGELOG.md'
$missing = @($inputs | Where-Object { -not (Test-Path (Join-Path $root $_) -PathType Leaf) })
if ($missing.Count -gt 0) {
    throw "Missing installer inputs: $($missing -join ', '). Run cargo build --release and cargo xtask fetch-models."
}
$exeVersion = (Get-Item $exe).VersionInfo.FileVersion
if ($exeVersion -ne $FileVersion) {
    throw "voicechanger.exe has file version '$exeVersion', expected '$FileVersion'."
}
$image = [Text.Encoding]::ASCII.GetString([IO.File]::ReadAllBytes($exe))
if ($image.IndexOf('VCRUNTIME140', [StringComparison]::OrdinalIgnoreCase) -ge 0) {
    throw 'voicechanger.exe imports VCRUNTIME140.dll; build with the static CRT (.cargo/config.toml).'
}

& (Join-Path $PSScriptRoot 'notices.ps1')

function Find-Iscc {
    $cmd = Get-Command ISCC.exe -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    @(
        (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'),
        (Join-Path $env:ProgramFiles 'Inno Setup 6\ISCC.exe'),
        (Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe')
    ) | Where-Object { Test-Path $_ -PathType Leaf } | Select-Object -First 1
}

$iscc = Find-Iscc
if (-not $iscc) {
    if (-not (Get-Command choco -ErrorAction SilentlyContinue)) {
        throw 'Inno Setup 6.3+ not found. Install it from https://jrsoftware.org/isdl.php or with winget install JRSoftware.InnoSetup.'
    }
    choco install innosetup -y --no-progress
    if ($LASTEXITCODE -ne 0) { throw 'choco install innosetup failed' }
    $iscc = Find-Iscc
    if (-not $iscc) { throw 'ISCC.exe not found after installing Inno Setup' }
}

& $iscc /Qp "/DAppVersion=$Version" "/DFileVersion=$FileVersion" (Join-Path $PSScriptRoot 'setup.iss')
if ($LASTEXITCODE -ne 0) { throw "ISCC failed with exit code $LASTEXITCODE" }
$setup = Join-Path $root "dist\Setup-VoiceChanger-$Version.exe"
if (-not (Test-Path $setup -PathType Leaf)) { throw "ISCC did not write $setup" }
Write-Host "built $setup"

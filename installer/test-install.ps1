<#
.SYNOPSIS
Installs, upgrades and uninstalls a built Setup-VoiceChanger-<Version>.exe and checks the result.
CI runs it after the installer smoke build, and release.yml runs it before publishing.
.DESCRIPTION
Checks: per-user install without admin, files and file version, `voicechanger.exe --self-test`,
the VB-CABLE check (against fake endpoints under HKCU), upgrade keeps user data, a silent
uninstall keeps it and /PURGEDATA removes it.
It deletes %APPDATA%\VoiceChanger and %LOCALAPPDATA%\VoiceChanger, so it refuses to run when
they or an install already exist: run it on a clean machine (a CI runner or Windows Sandbox).
.EXAMPLE
pwsh -File installer\test-install.ps1 -Setup dist\Setup-VoiceChanger-0.1.0-dev.1.exe -Version 0.1.0-dev.1 -FileVersion 0.1.0.0
pwsh -File installer\test-install.ps1 ... -Previous Setup-VoiceChanger-1.0.0.exe
#>
param(
    [Parameter(Mandatory = $true)] [string] $Setup,
    [Parameter(Mandatory = $true)] [string] $Version,
    [Parameter(Mandatory = $true)] [ValidatePattern('^\d+\.\d+\.\d+\.0$')] [string] $FileVersion,
    # Older installer to upgrade from. Without it the upgrade installs this build over itself.
    [string] $Previous
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$appId = '{7C3B9E52-4A1D-4F6E-9B8A-2D5E1C0F7A34}_is1'
$uninstallKey = "Software\Microsoft\Windows\CurrentVersion\Uninstall\$appId"
$appDir = Join-Path $env:LOCALAPPDATA 'Programs\VoiceChanger'
$roaming = Join-Path $env:APPDATA 'VoiceChanger'
$local = Join-Path $env:LOCALAPPDATA 'VoiceChanger'
$fakeCables = 'Software\VoiceChangerInstallerTest\Render'
$logs = Join-Path ([IO.Path]::GetTempPath()) "vc-installer-test-$PID"
$files = 'voicechanger.exe', 'unins000.exe', 'LICENSE.txt', 'THIRD_PARTY_NOTICES.md', 'CHANGELOG.md',
    'vendor\models\hubert.onnx', 'vendor\models\rmvpe.onnx',
    'vendor\runtime\onnxruntime.dll', 'vendor\runtime\DirectML.dll'

function Assert([bool] $Condition, [string] $Message) {
    if (-not $Condition) { throw "FAIL: $Message" }
    Write-Host "ok: $Message"
}

function Install([string] $Path, [string] $Name, [string[]] $Extra = @()) {
    $log = Join-Path $logs "$Name.log"
    $p = Start-Process -FilePath $Path -Wait -PassThru `
        -ArgumentList (@('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/LOG=`"$log`"") + $Extra)
    Assert ($p.ExitCode -eq 0) "$Name exits with 0 (got $($p.ExitCode), log $log)"
    Get-Content -Raw $log
}

# The uninstaller re-launches itself from %TEMP% and returns at once: wait for it to finish.
function Uninstall([string] $Name, [string[]] $Extra = @()) {
    $p = Start-Process -FilePath (Join-Path $appDir 'unins000.exe') -Wait -PassThru `
        -ArgumentList (@('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART') + $Extra)
    Assert ($p.ExitCode -eq 0) "$Name starts (exit $($p.ExitCode))"
    $deadline = (Get-Date).AddMinutes(3)
    while ((Test-Path "HKCU:\$uninstallKey") -or (Test-Path (Join-Path $appDir 'voicechanger.exe'))) {
        if ((Get-Date) -gt $deadline) { throw "FAIL: $Name did not finish within 3 minutes" }
        Start-Sleep -Seconds 1
    }
    # The uninstall entry goes before the post-uninstall data step: give that step time to run.
    Start-Sleep -Seconds 5
    Assert (-not (Test-Path $appDir)) "$Name removes $appDir"
}

function Seed-UserData {
    New-Item -ItemType Directory -Force (Join-Path $roaming 'models'), $local | Out-Null
    Set-Content -Encoding utf8 (Join-Path $roaming 'settings.toml') 'installer-test = true'
    Set-Content -Encoding utf8 (Join-Path $roaming 'models\test-model.onnx') 'not a model'
    Set-Content -Encoding utf8 (Join-Path $local 'cache.txt') 'cache'
}

function Test-UserData {
    (Test-Path (Join-Path $roaming 'settings.toml')) -and
    (Test-Path (Join-Path $roaming 'models\test-model.onnx')) -and
    (Test-Path (Join-Path $local 'cache.txt'))
}

$Setup = (Resolve-Path $Setup).Path
if ($Previous) { $Previous = (Resolve-Path $Previous).Path }
$existing = @($appDir, $roaming, $local | Where-Object { Test-Path $_ })
if ((Test-Path "HKCU:\$uninstallKey") -or $existing.Count -gt 0) {
    throw "Refusing to run: VoiceChanger is installed or has user data ($($existing -join ', ')). Run on a clean machine."
}
New-Item -ItemType Directory -Force $logs | Out-Null

try {
    # 1. Install the previous version (or this one), add user data, then upgrade to this build.
    if ($Previous) {
        Install $Previous 'install-previous' | Out-Null
    } else {
        Install $Setup 'install-first' | Out-Null
    }
    Seed-UserData
    $log = Install $Setup 'upgrade'

    Assert ($log -match 'Administrative install mode: No') 'setup runs without admin rights'
    Assert ($log -match 'Install mode root key: HKEY_CURRENT_USER') 'setup installs per user'
    foreach ($f in $files) {
        Assert (Test-Path (Join-Path $appDir $f) -PathType Leaf) "installs $f into $appDir"
    }
    Assert (Test-Path "HKCU:\$uninstallKey") 'uninstall entry is under HKCU'
    Assert (-not (Test-Path "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\$appId")) 'no uninstall entry under HKLM'
    $shown = (Get-ItemProperty "HKCU:\$uninstallKey").DisplayVersion
    Assert ($shown -eq $Version) "Apps & features shows version $Version (got '$shown')"
    $exeVersion = (Get-Item (Join-Path $appDir 'voicechanger.exe')).VersionInfo.FileVersion.Trim()
    Assert ($exeVersion -eq $FileVersion) "voicechanger.exe has file version $FileVersion (got '$exeVersion')"
    Assert (Test-UserData) 'the upgrade keeps settings and imported models'

    # 2. The installed app finds its bundled models and runtime and runs them.
    $out = Join-Path $logs 'self-test.txt'
    $p = Start-Process -FilePath (Join-Path $appDir 'voicechanger.exe') -ArgumentList '--self-test' `
        -Wait -PassThru -RedirectStandardOutput $out
    $report = (Get-Content -Raw $out).Trim()
    Assert ($p.ExitCode -eq 0 -and $report -match '^self-test passed') "voicechanger.exe --self-test: $report"

    # 3. The VB-CABLE prompt follows the cable check, with and without a (fake) cable endpoint.
    Assert ($log -match 'VoiceChanger: virtual cable not found') 'no cable: the finish page offers the VB-CABLE download'
    $endpoint = "HKCU:\$fakeCables\{00000000-0000-0000-0000-000000000001}"
    New-Item -Force "$endpoint\Properties" | Out-Null
    New-ItemProperty -Force $endpoint -Name DeviceState -PropertyType DWord -Value 1 | Out-Null
    New-ItemProperty -Force "$endpoint\Properties" -Name '{a45c254e-df1c-4efd-8020-67d146a850e0},2' `
        -PropertyType String -Value 'CABLE Input' | Out-Null
    $log = Install $Setup 'reinstall-with-cable' @("/CableRegistryKey=$fakeCables")
    Assert ($log -match 'VoiceChanger: virtual cable found') 'with a cable: the finish page does not offer the download'

    # 4. A silent uninstall keeps user data; /PURGEDATA (the "Yes" answer) removes it.
    Uninstall 'uninstall'
    Assert (Test-UserData) 'uninstall without /PURGEDATA keeps user data'
    Install $Setup 'install-again' | Out-Null
    Uninstall 'uninstall /PURGEDATA' @('/PURGEDATA')
    Assert (-not (Test-Path $roaming)) "uninstall /PURGEDATA removes $roaming"
    Assert (-not (Test-Path $local)) "uninstall /PURGEDATA removes $local"
    Write-Host "installer test passed: $Setup"
} finally {
    Remove-Item -Recurse -Force 'HKCU:\Software\VoiceChangerInstallerTest' -ErrorAction SilentlyContinue
    Write-Host "setup logs: $logs"
}

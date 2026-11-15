<#
.SYNOPSIS
Installs, upgrades and uninstalls a built Setup-VoiceChanger-<Version>.exe and checks the result.
CI runs it after the installer smoke build, and release.yml runs it before publishing.
.DESCRIPTION
Checks first that the VB-CABLE pack pinned in setup.iss still downloads with the pinned SHA-256
and contains the pinned setup exe. It serves fake VB-CABLE packs from a local HTTP server on
127.0.0.1 (setup's test-only /CableUrl= and /CableSha256= switches). Then: per-user install without admin, files and file version, `voicechanger.exe --self-test`,
the VB-CABLE check (against fake endpoints under HKCU), a silent install without /INSTALLCABLE
downloads nothing, /CableUrl= outside 127.0.0.1 is ignored, /INSTALLCABLE downloads the pack,
refuses a wrong SHA-256, runs the setup exe with -i -h, asks for a restart when no cable shows up
and downloads nothing when a cable is present, upgrade keeps user data, the in-app
updater's /SILENT /RELAUNCH upgrade starts voicechanger.exe again while a silent install without
/RELAUNCH does not, a silent uninstall keeps user data and /PURGEDATA removes it.
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

function Get-PinnedValue([string] $Name) {
    $m = Select-String -Path (Join-Path $PSScriptRoot 'setup.iss') -Pattern "^#define $Name `"(.+)`"$"
    if (-not $m) { throw "setup.iss has no #define $Name" }
    $m.Matches[0].Groups[1].Value
}

# Downloads the pinned VB-CABLE pack: fails when vb-audio.com changes it under the same URL.
function Test-CablePin {
    $url = Get-PinnedValue 'CableUrl'; $sha = Get-PinnedValue 'CableSha256'; $exe = Get-PinnedValue 'CableSetupExe'
    $dir = Join-Path ([IO.Path]::GetTempPath()) "vc-cable-pin-$PID"; New-Item -ItemType Directory -Force $dir | Out-Null; $zip = Join-Path $dir 'pack.zip'
    for ($i = 1; $true; $i++) { try { Invoke-WebRequest -UseBasicParsing $url -OutFile $zip; break } catch { if ($i -ge 4) { throw "FAIL: could not download $url ($_)" }; Start-Sleep -Seconds (5 * $i) } }
    $got = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLowerInvariant()
    Assert ($got -eq $sha) "the pinned VB-CABLE pack $url has SHA-256 $sha (got $got)"
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $a = [IO.Compression.ZipFile]::OpenRead($zip); try { $names = @($a.Entries.FullName) } finally { $a.Dispose() }
    Assert ($names -contains ($exe -replace '\\','/')) "the pinned pack contains $exe"
    Remove-Item -Recurse -Force $dir
}

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

$cableEndpoint = "$fakeCables\{00000000-0000-0000-0000-000000000002}"
$packDir = Join-Path $logs 'packs'
$requests = Join-Path $logs 'requests.txt'
$stubArgs = Join-Path $logs 'stub-args.txt'

# A UAC prompt or a hang fails the run instead of blocking the runner.
function Install([string] $Path, [string] $Name, [string[]] $Extra = @(), [string] $Quiet = '/VERYSILENT') {
    $log = Join-Path $logs "$Name.log"
    $p = Start-Process -FilePath $Path -PassThru `
        -ArgumentList (@($Quiet, '/SUPPRESSMSGBOXES', '/NORESTART', "/LOG=`"$log`"") + $Extra)
    $null = $p.Handle   # keeps ExitCode readable after the process ends
    if (-not $p.WaitForExit(600000)) { $p | Stop-Process -Force; throw "FAIL: $Name did not finish within 10 minutes (log $log)" }
    Assert ($p.ExitCode -eq 0) "$Name exits with 0 (got $($p.ExitCode), log $log)"
    Get-Content -Raw $log
}

# Serves files from $Root on 127.0.0.1 and appends each request path to $Log. No URL ACL or admin needed.
function Start-PackServer([string] $Root, [string] $Log) {
    $listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
    $listener.Start()
    $job = Start-ThreadJob -ArgumentList $listener, $Root, $Log -ScriptBlock {
        param($listener, $root, $log)
        while ($true) {
            $client = $listener.AcceptTcpClient()
            try {
                $stream = $client.GetStream()
                $reader = [IO.StreamReader]::new($stream, [Text.Encoding]::ASCII, $false, 1024, $true)
                $method, $path = ($reader.ReadLine() -split ' ')[0, 1]
                while ($reader.ReadLine()) { }
                Add-Content -Path $log -Value $path
                $file = Join-Path $root $path.TrimStart('/')
                if ($path -match '^/[\w.-]+$' -and (Test-Path $file -PathType Leaf)) {
                    $body = [IO.File]::ReadAllBytes($file); $status = '200 OK'
                } else {
                    $body = [byte[]]::new(0); $status = '404 Not Found'
                }
                $head = [Text.Encoding]::ASCII.GetBytes(
                    "HTTP/1.1 $status`r`nContent-Type: application/zip`r`nContent-Length: $($body.Length)`r`nConnection: close`r`n`r`n")
                $stream.Write($head, 0, $head.Length)
                if ($method -ne 'HEAD') { $stream.Write($body, 0, $body.Length) }
                $stream.Flush()
            } catch { } finally { $client.Close() }
        }
    }
    [pscustomobject]@{ Listener = $listener; Job = $job; Url = "http://127.0.0.1:$($listener.LocalEndpoint.Port)" }
}

# Builds $packDir\<Name>.zip holding a stub VBCABLE_Setup_x64.exe that records its arguments in
# $stubArgs and, with -CreatesCable, adds an active "CABLE Input" endpoint under $cableEndpoint.
function New-StubPack([string] $Name, [switch] $CreatesCable) {
    $csc = Join-Path $env:WINDIR 'Microsoft.NET\Framework64\v4.0.30319\csc.exe'
    $work = Join-Path $logs "stub-$Name"
    $content = Join-Path $work 'pack'
    $exe = Join-Path $content (Get-PinnedValue 'CableSetupExe')   # keeps the real pack's layout
    New-Item -ItemType Directory -Force (Split-Path -Parent $exe), $packDir | Out-Null
    $create = if ($CreatesCable) { 'true' } else { 'false' }
    $source = @"
using System.IO;
using Microsoft.Win32;
class Stub {
    static int Main(string[] args) {
        File.WriteAllText(@"$stubArgs", string.Join(" ", args));
        if ($create) {
            using (var k = Registry.CurrentUser.CreateSubKey(@"$cableEndpoint"))
                k.SetValue("DeviceState", 1, RegistryValueKind.DWord);
            using (var p = Registry.CurrentUser.CreateSubKey(@"$cableEndpoint\Properties"))
                p.SetValue("{a45c254e-df1c-4efd-8020-67d146a850e0},2", "CABLE Input");
        }
        return 0;
    }
}
"@
    Set-Content -Encoding utf8 (Join-Path $work 'stub.cs') $source
    & $csc /nologo /target:exe "/out:$exe" (Join-Path $work 'stub.cs')
    if ($LASTEXITCODE -ne 0) { throw "csc failed with exit code $LASTEXITCODE" }
    $zip = Join-Path $packDir "$Name.zip"
    Compress-Archive -Force -Path (Join-Path $content '*') -DestinationPath $zip
    (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLowerInvariant()
}

function Reset-CableTest {
    Remove-Item -Recurse -Force "HKCU:\$fakeCables" -ErrorAction SilentlyContinue
    Remove-Item -Force $requests, $stubArgs -ErrorAction SilentlyContinue
}

# The leading comma keeps an empty or one-line list an array: under StrictMode $null.Count throws.
function Get-Requests { if (Test-Path $requests) { , @(Get-Content $requests) } else { , @() } }

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

Test-CablePin

$Setup = (Resolve-Path $Setup).Path
if ($Previous) { $Previous = (Resolve-Path $Previous).Path }
$existing = @($appDir, $roaming, $local | Where-Object { Test-Path $_ })
if ((Test-Path "HKCU:\$uninstallKey") -or $existing.Count -gt 0) {
    throw "Refusing to run: VoiceChanger is installed or has user data ($($existing -join ', ')). Run on a clean machine."
}
New-Item -ItemType Directory -Force $logs, $packDir | Out-Null
$server = Start-PackServer $packDir $requests

try {
    # 1. Install the previous version (or this one), add user data, then upgrade to this build.
    if ($Previous) {
        Install $Previous 'install-previous' | Out-Null
    } else {
        Install $Setup 'install-first' | Out-Null
    }
    Seed-UserData
    $log = Install $Setup 'upgrade' @('/RELAUNCH') '/SILENT'
    Assert ((Get-Requests).Count -eq 0) 'a silent install without /INSTALLCABLE downloads nothing'
    Assert ($log -notmatch 'VoiceChanger: downloading VB-CABLE') 'a silent install without /INSTALLCABLE does not install VB-CABLE'

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

    # The in-app updater passes /RELAUNCH: setup starts the installed app again, also when silent.
    $exe = Join-Path $appDir 'voicechanger.exe'
    Assert ($log -match '-- Run entry --') '/RELAUNCH runs a [Run] entry'
    Assert ($log -match [regex]::Escape("Filename: $exe")) "/RELAUNCH starts $exe"
    # The app logs "VoiceChanger starting" before it opens its window, so no GPU is needed.
    $deadline = (Get-Date).AddSeconds(30)
    $started = $false
    while (-not $started -and (Get-Date) -lt $deadline) {
        $started = @(Get-ChildItem -File (Join-Path $local 'logs') -ErrorAction SilentlyContinue |
            Where-Object { (Get-Content -Raw $_.FullName) -match 'VoiceChanger starting' }).Count -gt 0
        if (-not $started) { Start-Sleep -Seconds 1 }
    }
    Assert $started "the relaunched app logs 'VoiceChanger starting' in $local\logs"
    Get-Process voicechanger -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $exe } | Stop-Process -Force
    $deadline = (Get-Date).AddSeconds(10)
    while (@(Get-Process voicechanger -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $exe }).Count -gt 0) {
        if ((Get-Date) -gt $deadline) { throw "FAIL: the relaunched voicechanger.exe did not stop within 10 seconds" }
        Start-Sleep -Milliseconds 500
    }

    # 2. The installed app finds its bundled models and runtime and runs them.
    $out = Join-Path $logs 'self-test.txt'
    $p = Start-Process -FilePath (Join-Path $appDir 'voicechanger.exe') -ArgumentList '--self-test' `
        -Wait -PassThru -RedirectStandardOutput $out
    $report = (Get-Content -Raw $out).Trim()
    Assert ($p.ExitCode -eq 0 -and $report -match '^self-test passed') "voicechanger.exe --self-test: $report"

    # 3. The VB-CABLE prompt follows the cable check, with and without a (fake) cable endpoint.
    Assert ($log -match 'VoiceChanger: virtual cable not found') 'no cable: the finish page offers the VB-CABLE download'
    # Without /INSTALLCABLE the pack is never downloaded, so the real driver never runs here.
    Reset-CableTest
    $log = Install $Setup 'cable-url-not-local' @('/CableUrl=https://example.com/VBCABLE.zip',
        '/CableSha256=0000000000000000000000000000000000000000000000000000000000000000', "/CableRegistryKey=$fakeCables")
    Assert ($log -match [regex]::Escape('VoiceChanger: ignoring /CableUrl=https://example.com/VBCABLE.zip')) '/CableUrl= outside 127.0.0.1 is ignored'
    Assert ($log -match [regex]::Escape('VoiceChanger: VB-CABLE pack ' + (Get-PinnedValue 'CableUrl'))) 'setup keeps the pinned pack URL'

    # /INSTALLCABLE with a local fake pack: setup downloads, checks, unzips and runs the stub installer.
    $okSha = New-StubPack 'ok' -CreatesCable
    $noneSha = New-StubPack 'none'
    $cable = @('/INSTALLCABLE', "/CableRegistryKey=$fakeCables")
    # Case 3: wrong hash
    Reset-CableTest
    $log = Install $Setup 'cable-bad-hash' ($cable + "/CableUrl=$($server.Url)/ok.zip", '/CableSha256=0000000000000000000000000000000000000000000000000000000000000000')
    Assert ($log -match 'VoiceChanger: VB-CABLE not installed: ') 'a changed pack is not installed'
    Assert (-not (Test-Path $stubArgs)) 'a pack with the wrong hash is never run'
    Assert (Test-Path (Join-Path $appDir 'voicechanger.exe')) 'VoiceChanger installs although VB-CABLE failed'
    # Case 4: VB-Audio's installer ran, but no endpoint showed up
    Reset-CableTest
    $log = Install $Setup 'cable-restart' ($cable + "/CableUrl=$($server.Url)/none.zip", "/CableSha256=$noneSha")
    Assert ($log -match 'VoiceChanger: VB-CABLE restart needed') 'no cable after VB-Audio''s installer: setup asks for a restart'
    # Case 2: installs
    Reset-CableTest
    $log = Install $Setup 'cable-install' ($cable + "/CableUrl=$($server.Url)/ok.zip", "/CableSha256=$okSha")
    Assert ((Get-Requests) -contains '/ok.zip') 'setup downloads the pack'
    Assert ((Test-Path $stubArgs) -and (Get-Content -Raw $stubArgs).Trim() -eq '-i -h') 'VB-Audio''s installer runs with -i -h'
    Assert ($log -match 'VoiceChanger: VB-CABLE installed') 'setup installs VB-CABLE'
    Assert ($log -match 'VoiceChanger: virtual cable found') 'after the install the finish page no longer offers the download'
    # Case 5: cable present (the endpoint the stub created in case 2)
    Remove-Item -Force $requests -ErrorAction SilentlyContinue
    $log = Install $Setup 'cable-present' ($cable + "/CableUrl=$($server.Url)/ok.zip", "/CableSha256=$okSha")
    Assert ((Get-Requests).Count -eq 0) 'with a cable present setup downloads nothing'

    Reset-CableTest
    $endpoint = "HKCU:\$fakeCables\{00000000-0000-0000-0000-000000000001}"
    New-Item -Force "$endpoint\Properties" | Out-Null
    New-ItemProperty -Force $endpoint -Name DeviceState -PropertyType DWord -Value 1 | Out-Null
    New-ItemProperty -Force "$endpoint\Properties" -Name '{a45c254e-df1c-4efd-8020-67d146a850e0},2' `
        -PropertyType String -Value 'CABLE Input' | Out-Null
    $log = Install $Setup 'reinstall-with-cable' @("/CableRegistryKey=$fakeCables")
    Assert ($log -match 'VoiceChanger: virtual cable found') 'with a cable: the finish page does not offer the download'
    Assert ($log -notmatch '-- Run entry --') 'without /RELAUNCH a silent install does not start the app'

    # 4. A silent uninstall keeps user data; /PURGEDATA (the "Yes" answer) removes it.
    Uninstall 'uninstall'
    Assert (Test-UserData) 'uninstall without /PURGEDATA keeps user data'
    Install $Setup 'install-again' | Out-Null
    Uninstall 'uninstall /PURGEDATA' @('/PURGEDATA')
    Assert (-not (Test-Path $roaming)) "uninstall /PURGEDATA removes $roaming"
    Assert (-not (Test-Path $local)) "uninstall /PURGEDATA removes $local"
    Write-Host "installer test passed: $Setup"
} catch {
    # CI keeps no files: show the end of the newest setup log.
    Get-ChildItem -File $logs -Filter *.log -ErrorAction SilentlyContinue | Sort-Object LastWriteTime |
        Select-Object -Last 1 | ForEach-Object { Write-Host "--- end of $($_.Name)"; Get-Content $_.FullName -Tail 60 | Write-Host }
    throw
} finally {
    if (Get-Variable server -ErrorAction SilentlyContinue) { $server.Listener.Stop(); $server.Job | Remove-Job -Force }
    Remove-Item -Recurse -Force 'HKCU:\Software\VoiceChangerInstallerTest' -ErrorAction SilentlyContinue
    Write-Host "setup logs: $logs"
}

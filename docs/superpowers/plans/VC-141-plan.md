# VC-141 plan: setup offers to download and install VB-CABLE

## Goal
When no virtual audio cable is found, setup shows a "Virtual audio cable" page. If the user agrees, setup downloads the pinned VB-CABLE pack from vb-audio.com when they press Install, checks its SHA-256, unzips it with `tar.exe` and runs `VBCABLE_Setup_x64.exe -i -h` elevated. It offers a restart only when the cable still does not show up afterwards.

## Architecture
All the logic is in `installer/setup.iss` `[Code]`, next to the existing cable check (`VirtualCableInstalled`, `NoVirtualCable`). No Rust code changes. `installer/test-install.ps1` covers it in CI and in the release workflow:
- a local TCP HTTP server on 127.0.0.1, run as a thread job;
- fake packs holding a C# stub `VBCABLE_Setup_x64.exe`, compiled with the .NET Framework `csc.exe`;
- the test-only switches `/INSTALLCABLE`, `/CableUrl=` and `/CableSha256=`;
- a pin check that downloads the real pack.

## Facts about the repo (checked at 3eeeed8)
- `installer/setup.iss` has `PrivilegesRequired=lowest`, `ArchitecturesInstallIn64BitMode=x64compatible` (so `{sys}` is the 64-bit System32) and `MinVersion=10.0.19045`. Its `[Tasks]` has `desktopicon`, so `wpSelectTasks` is shown.
- `[Run]` line 71 opens `https://vb-audio.com/Cable/` with `Check: NoVirtualCable`. Keep it.
- `[Code]` already has `ParamGiven(Name)`, `VirtualCableInstalled` (honours `/CableRegistryKey=` = an HKCU subkey), `NoVirtualCable` (cached in `CableChecked`/`CableFound`) and `CurStepChanged`, which logs `VoiceChanger: virtual cable found|not found` at `ssPostInstall`.
- `installer/test-install.ps1`:
  - runs under pwsh 7 with `Set-StrictMode -Version Latest`;
  - its helpers are `Assert`, `Install(Path, Name, Extra, Quiet)` (always passes `/SUPPRESSMSGBOXES /NORESTART /LOG=`) and `Uninstall`;
  - the fake cable key is `$fakeCables = 'Software\VoiceChangerInstallerTest\Render'`, and `finally` deletes `HKCU:\Software\VoiceChangerInstallerTest`.
- `installer/build.ps1 -Version <v> -FileVersion <X.Y.Z.0>` builds `dist\Setup-VoiceChanger-<v>.exe`. It needs `cargo build --release` and `cargo xtask fetch-models` first. The workspace version is `1.1.0`, so locally use `-Version 1.1.0-dev.1 -FileVersion 1.1.0.0`. CI installs Inno Setup with choco (latest 6.x); the README asks for 6.3+.
- vb-audio.com/Cable today links `https://download.vb-audio.com/Download_CABLE/VBCABLE_Driver_Pack46.zip` ("NOV 2026, XP to WIN11 32/64 bits/Arm64", 1.29 MB). The page doesn't name the setup exe or its switches, so Step 1 confirms them from the zip.
- Commit style: `feat(installer): … (VC-141)`, `test(installer): …`, `docs: …`. Each commit ends with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- `test-install.ps1` refuses to run where VoiceChanger is installed or has data. The full run happens in CI (`.github/workflows/ci.yml` step "Installer test"). A builder runs it locally only on a clean VM or in Windows Sandbox; otherwise they push and read CI.

## File map
| File | Change |
|---|---|
| `installer/setup.iss` | `#define CableUrl/CableSha256/CableSetupExe`; `[Messages] FinishedRestartLabel`; `[Code]`: `PackUrl`, `PackSha256`, `CableRequested`, `InstallCable`, the outcome variable, the offer page, `UpdateReadyMemo`, `NextButtonClick`, `NeedRestart` |
| `installer/test-install.ps1` | pin check, pack server, stub packs, timeout in `Install`, cable cases 1–6 |
| `README.md` | Install step 3 and the "no manual test" paragraph |
| `CHANGELOG.md` | `[Unreleased]` → `### Added` entry |

## Shared contracts (used across steps)

### ISPP defines (top of setup.iss, after `#define Root`)
```
; VB-CABLE pack from vb-audio.com/Cable. Bump both together in a PR; test-install.ps1 checks them.
#define CableUrl "https://download.vb-audio.com/Download_CABLE/VBCABLE_Driver_Pack46.zip"
#define CableSha256 "<lowercase hex computed in Step 1>"
#define CableSetupExe "VBCABLE_Setup_x64.exe"
```
If Step 1 finds the exe in a subfolder of the zip, `CableSetupExe` holds that relative path with backslashes, e.g. `Pack46\VBCABLE_Setup_x64.exe`. The stub packs in the test copy that layout.

### Log lines (exact)
- `VoiceChanger: VB-CABLE pack <url>`: always, from `InitializeWizard`.
- `VoiceChanger: ignoring /CableUrl=<value>`: when the override is rejected.
- `VoiceChanger: downloading VB-CABLE from <url>`
- `VoiceChanger: VB-CABLE installed`
- `VoiceChanger: VB-CABLE restart needed`
- `VoiceChanger: VB-CABLE not installed: <reason>`
- `VoiceChanger: VB-CABLE declined`
- `VoiceChanger: VB-CABLE skipped`
- The existing `VoiceChanger: virtual cable found|not found` lines stay.

### Pascal (setup.iss [Code])
```pascal
const
  LocalUrlPrefix = 'http://127.0.0.1:';
  ErrorCancelled = 1223;
var
  CableOutcome: String;   { '', 'installed', 'restart', 'failed', 'declined', 'skipped' }
  CablePage: TInputOptionWizardPage;

{ /CableUrl= and /CableSha256= are for installer\test-install.ps1 only: accepted only for a local URL. }
function PackOverridden: Boolean;
begin
  Result := Pos(LocalUrlPrefix, ExpandConstant('{param:CableUrl|}')) = 1;
end;

function PackUrl: String;
begin
  if PackOverridden then
    Result := ExpandConstant('{param:CableUrl|}')
  else
    Result := '{#CableUrl}';
end;

function PackSha256: String;
begin
  Result := '{#CableSha256}';
  if PackOverridden and (ExpandConstant('{param:CableSha256|}') <> '') then
    Result := ExpandConstant('{param:CableSha256|}');
end;
```
`InitializeWizard` logs the pack URL and, when `{param:CableUrl|}` is not empty and `PackOverridden` is False, logs the ignoring line. `/CableSha256=` without an accepted URL is ignored, so the real driver can never run with a test hash.

```pascal
{ True when setup should download and install VB-CABLE: never in a silent install unless /INSTALLCABLE. }
function CableRequested: Boolean;
begin
  Result := False;
  if not NoVirtualCable then
    Exit;
  if ParamGiven('/INSTALLCABLE') then
    Result := True
  else if not WizardSilent then
    Result := (CablePage <> nil) and CablePage.Values[0];
end;
```
(`CablePage` is created in Step 4; until then `CableRequested` only checks `/INSTALLCABLE`. Step 3 writes it with the `else if` branch left out, and Step 4 adds it.)

### InstallCable (Step 3)
```pascal
procedure CableFailed(Reason: String);
begin
  CableOutcome := 'failed';
  Log('VoiceChanger: VB-CABLE not installed: ' + Reason);
  SuppressibleMsgBox('VB-CABLE could not be installed: ' + Reason + #13#10#13#10 +
    'Setup will continue installing {#AppName}. You can install VB-CABLE later from https://vb-audio.com/Cable/.',
    mbError, MB_OK, IDOK);
end;

{ Downloads the pinned pack into {tmp}, checks its SHA-256 (Inno raises on a mismatch or HTTP error),
  unzips it with Windows' tar.exe and runs VB-Audio's installer silently and elevated. }
procedure InstallCable;
var
  DownloadPage: TDownloadWizardPage;
  Zip, Dir, Exe: String;
  Code, I: Integer;
begin
  DownloadPage := nil;
  Zip := ExpandConstant('{tmp}\vbcable.zip');
  Dir := ExpandConstant('{tmp}\vbcable');
  Log('VoiceChanger: downloading VB-CABLE from ' + PackUrl);
  try
    try
      if WizardSilent then
        DownloadTemporaryFile(PackUrl, 'vbcable.zip', PackSha256, nil)
      else
      begin
        DownloadPage := CreateDownloadPage('Virtual audio cable',
          'Downloading VB-CABLE from vb-audio.com...', nil);
        DownloadPage.Clear;
        DownloadPage.Add(PackUrl, 'vbcable.zip', PackSha256);
        DownloadPage.Show;
        DownloadPage.Download;
      end;
    except
      if (DownloadPage <> nil) and DownloadPage.AbortedByUser then
      begin
        CableOutcome := 'skipped';
        Log('VoiceChanger: VB-CABLE skipped');
      end else
        CableFailed('the download failed (' + GetExceptionMessage + ')');
      Exit;
    end;
    ForceDirectories(Dir);
    if not Exec(ExpandConstant('{sys}\tar.exe'), '-xf "' + Zip + '" -C "' + Dir + '"', '',
      SW_HIDE, ewWaitUntilTerminated, Code) or (Code <> 0) then
    begin
      CableFailed('the pack could not be unzipped (tar.exe exit code ' + IntToStr(Code) + ')');
      Exit;
    end;
    Exe := Dir + '\{#CableSetupExe}';
    if not FileExists(Exe) then
    begin
      CableFailed('{#CableSetupExe} is missing from the pack');
      Exit;
    end;
    if DownloadPage <> nil then
    begin
      DownloadPage.SetText('Installing the VB-CABLE audio driver...',
        'Windows will ask you to approve VB-Audio''s installer and its driver.');
      DownloadPage.ProgressBar.Style := npbstMarquee;
    end;
    if not ShellExec('runas', Exe, '-i -h', ExtractFileDir(Exe), SW_SHOWNORMAL, ewWaitUntilTerminated, Code) then
    begin
      if Code = ErrorCancelled then
      begin
        CableOutcome := 'declined';
        Log('VoiceChanger: VB-CABLE declined');
      end else
        CableFailed('VB-Audio''s installer did not start (' + SysErrorMessage(Code) + ')');
      Exit;
    end;
    for I := 1 to 15 do
    begin
      if VirtualCableInstalled then
        Break;
      Sleep(1000);
    end;
    if VirtualCableInstalled then
    begin
      CableOutcome := 'installed';
      CableFound := True;
      CableChecked := True;
      Log('VoiceChanger: VB-CABLE installed');
    end else
    begin
      CableOutcome := 'restart';
      Log('VoiceChanger: VB-CABLE restart needed');
    end;
  finally
    if DownloadPage <> nil then
      DownloadPage.Hide;
  end;
end;

function NextButtonClick(CurPageID: Integer): Boolean;
begin
  if (CurPageID = wpReady) and CableRequested then
    InstallCable;
  Result := True;
end;

function NeedRestart: Boolean;
begin
  Result := CableOutcome = 'restart';
end;
```
Notes:
- `NextButtonClick` is also called in silent mode, where Inno simulates the Next clicks, so `/VERYSILENT /INSTALLCABLE` reaches `InstallCable`.
- With `/SUPPRESSMSGBOXES`, `SuppressibleMsgBox` returns `IDOK` at once.
- `PackUrl` and `PackSha256` must be declared before `InstallCable`, and `ParamGiven` before `CableRequested`. Move `ParamGiven` up next to the cable code if needed.
- Pascal has no `exit` inside `except` problems: `Exit` inside `try..finally` still runs `finally`.
- If the compiler rejects `nil` for the progress callback, add `function OnDownloadProgress(const Url, FileName: String; const Progress, ProgressMax: Int64): Boolean; begin Result := True; end;` and pass `@OnDownloadProgress`.

### [Messages] (Step 3)
```
[Messages]
FinishedRestartLabel=VB-CABLE was installed, but Windows does not show it yet. Restart your computer to finish installing it.%n%nIf "CABLE Input" is still missing after the restart, install VB-CABLE from https://vb-audio.com/Cable/. Would you like to restart now?
```
Only our cable install asks for a restart, so this label never shows for anything else.

### Offer page (Step 4)
```pascal
procedure InitializeWizard;
begin
  { existing logging from Step 2 stays here }
  CablePage := CreateInputOptionPage(wpSelectTasks, 'Virtual audio cable',
    'No virtual audio cable was found on this PC.',
    '{#AppName} sends your voice to a virtual audio cable, which Discord, Zoom, games and other apps then use as a microphone.' + #13#10#13#10 +
    'Setup can download VB-CABLE from vb-audio.com and install it. VB-CABLE is made by VB-Audio (www.vb-cable.com) and is donationware: if you find it useful, you can support it with a donation or a licence. Windows will ask you to approve its driver.',
    True, False);
  CablePage.Add('Download and install VB-CABLE (recommended)');
  CablePage.Add('Skip, I will install a virtual cable myself');
  CablePage.Values[0] := True;
end;

function ShouldSkipPage(PageID: Integer): Boolean;
begin
  Result := (CablePage <> nil) and (PageID = CablePage.ID) and not NoVirtualCable;
end;

function UpdateReadyMemo(Space, NewLine, MemoUserInfoInfo, MemoDirInfo, MemoTypeInfo,
  MemoComponentsInfo, MemoGroupInfo, MemoTasksInfo: String): String;
var
  S: String;
begin
  S := '';
  if MemoUserInfoInfo <> '' then S := S + MemoUserInfoInfo + NewLine + NewLine;
  if MemoDirInfo <> '' then S := S + MemoDirInfo + NewLine + NewLine;
  if MemoTypeInfo <> '' then S := S + MemoTypeInfo + NewLine + NewLine;
  if MemoComponentsInfo <> '' then S := S + MemoComponentsInfo + NewLine + NewLine;
  if MemoGroupInfo <> '' then S := S + MemoGroupInfo + NewLine + NewLine;
  if MemoTasksInfo <> '' then S := S + MemoTasksInfo + NewLine + NewLine;
  if CableRequested then
    S := S + 'Virtual audio cable:' + NewLine + Space + 'Download and install VB-CABLE from vb-audio.com' + NewLine + NewLine;
  Result := S;
end;
```

### Test helpers (test-install.ps1, Step 2)
```powershell
$cableEndpoint = "$fakeCables\{00000000-0000-0000-0000-000000000002}"
$packDir = Join-Path $logs 'packs'
$requests = Join-Path $logs 'requests.txt'
$stubArgs = Join-Path $logs 'stub-args.txt'

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

function Get-Requests { if (Test-Path $requests) { @(Get-Content $requests) } else { @() } }
```
`Get-PinnedValue` (Step 1) reads a define from setup.iss:
```powershell
function Get-PinnedValue([string] $Name) {
    $m = Select-String -Path (Join-Path $PSScriptRoot 'setup.iss') -Pattern "^#define $Name `"(.+)`"$"
    if (-not $m) { throw "setup.iss has no #define $Name" }
    $m.Matches[0].Groups[1].Value
}
```
`Install` gets a timeout, so a UAC prompt or a hang fails the run instead of blocking the runner:
```powershell
function Install([string] $Path, [string] $Name, [string[]] $Extra = @(), [string] $Quiet = '/VERYSILENT') {
    $log = Join-Path $logs "$Name.log"
    $p = Start-Process -FilePath $Path -PassThru `
        -ArgumentList (@($Quiet, '/SUPPRESSMSGBOXES', '/NORESTART', "/LOG=`"$log`"") + $Extra)
    $null = $p.Handle   # keeps ExitCode readable after the process ends
    if (-not $p.WaitForExit(600000)) { $p | Stop-Process -Force; throw "FAIL: $Name did not finish within 10 minutes (log $log)" }
    Assert ($p.ExitCode -eq 0) "$Name exits with 0 (got $($p.ExitCode), log $log)"
    Get-Content -Raw $log
}
```

## Steps

### 1. Pin the VB-CABLE pack and add the pin check
See the step detail. It gives the exact commands to download the pack into a fresh temporary folder and to compute the hash and list the zip. Then it adds the defines and `Get-PinnedValue` + `Test-CablePin` to `test-install.ps1`.

### 2. Pack overrides, logging and the test harness (cases 1 and 6)
`PackOverridden`, `PackUrl`, `PackSha256` and `InitializeWizard` logging; the `Install` timeout, `Start-PackServer`, `New-StubPack` and `Reset-CableTest`; case 1 (the silent upgrade sends no request) and case 6 (`/CableUrl=https://example.com/...` is ignored).

### 3. Download, unzip and install VB-CABLE on /INSTALLCABLE (cases 2–5)
`CableOutcome`, `CableFailed`, `InstallCable`, `CableRequested` (without the page branch), `NextButtonClick`, `NeedRestart` and `[Messages] FinishedRestartLabel`; test cases 3, 4, 2 and 5, in that order.

### 4. The "Virtual audio cable" wizard page and Ready memo
`CablePage` in `InitializeWizard`, `ShouldSkipPage`, `UpdateReadyMemo`, and the page branch in `CableRequested`.

### 5. README and CHANGELOG
Install step 3, the "no manual test" paragraph, and the `[Unreleased]` → `### Added` entry.

## Final verification
- `cargo build --release; cargo xtask fetch-models; pwsh -File installer\build.ps1 -Version 1.1.0-dev.1 -FileVersion 1.1.0.0` compiles setup.iss without errors.
- On a clean Windows VM or Sandbox: `pwsh -File installer\test-install.ps1 -Setup dist\Setup-VoiceChanger-1.1.0-dev.1.exe -Version 1.1.0-dev.1 -FileVersion 1.1.0.0` prints all the `ok:` lines, including the pin check and cable cases 1–6, and ends with `installer test passed`.
- On the PR, the CI job `checks`, step "Installer test (install, upgrade, uninstall)", is green. The release workflow runs the same script through `ci.yml`.

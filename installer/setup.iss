; VoiceChanger installer. Build with installer\build.ps1, which runs
;   ISCC /DAppVersion=<display version> /DFileVersion=X.Y.Z.0 installer\setup.iss
; AppId must never change: it is what makes upgrades install in place.

#ifndef AppVersion
  #error Pass /DAppVersion=<version>
#endif
#ifndef FileVersion
  #error Pass /DFileVersion=X.Y.Z.0
#endif
#define AppName "VoiceChanger"
#define AppExe "voicechanger.exe"
#define Root AddBackslash(SourcePath) + ".."

[Setup]
AppId={{7C3B9E52-4A1D-4F6E-9B8A-2D5E1C0F7A34}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=VoiceChanger contributors
AppCopyright=Copyright (c) 2026 VoiceChanger contributors
AppPublisherURL=https://github.com/mmhy2003/VoiceChanger
AppSupportURL=https://github.com/mmhy2003/VoiceChanger/issues
AppUpdatesURL=https://github.com/mmhy2003/VoiceChanger/releases
VersionInfoVersion={#FileVersion}
VersionInfoProductVersion={#FileVersion}
VersionInfoProductTextVersion={#AppVersion}
VersionInfoProductName={#AppName}
VersionInfoDescription={#AppName} Setup
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog commandline
UsedUserAreasWarning=no
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.19045
CloseApplications=yes
RestartApplications=no
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
WizardStyle=modern
Compression=lzma2/ultra64
SolidCompression=yes
OutputDir={#Root}\dist
OutputBaseFilename=Setup-{#AppName}-{#AppVersion}

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#Root}\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\vendor\models\hubert.onnx"; DestDir: "{app}\vendor\models"; Flags: ignoreversion
Source: "{#Root}\vendor\models\rmvpe.onnx"; DestDir: "{app}\vendor\models"; Flags: ignoreversion
Source: "{#Root}\vendor\runtime\onnxruntime.dll"; DestDir: "{app}\vendor\runtime"; Flags: ignoreversion
Source: "{#Root}\vendor\runtime\DirectML.dll"; DestDir: "{app}\vendor\runtime"; Flags: ignoreversion
Source: "{#Root}\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion
Source: "{#Root}\THIRD_PARTY_NOTICES.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\CHANGELOG.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
Filename: "https://vb-audio.com/Cable/"; Description: "Open the VB-CABLE download page"; Flags: shellexec postinstall nowait skipifsilent; Check: NoVirtualCable
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: postinstall nowait skipifsilent

[Code]
const
  RenderKey = 'SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Render';
  { PKEY_Device_DeviceDesc ("CABLE Input") and PKEY_DeviceInterface_FriendlyName ("VB-Audio Virtual Cable"). }
  DeviceDescValue = '{a45c254e-df1c-4efd-8020-67d146a850e0},2';
  InterfaceNameValue = '{b3f8fa53-0004-438e-9003-51a46e139bfc},6';
  DeviceStateActive = 1;

var
  CableChecked: Boolean;
  CableFound: Boolean;

{ Same rule as is_virtual_cable in crates/audio/src/devices.rs. }
function IsVirtualCableName(Name: String): Boolean;
var
  Lower: String;
begin
  Lower := Lowercase(Name);
  Result := (Pos('cable', Lower) > 0) or (Pos('voicemeeter', Lower) > 0)
    or (Pos('virtual audio cable', Lower) > 0);
end;

{ True if an active render endpoint looks like a virtual cable. HKLM is readable without admin.
  installer\test-install.ps1 passes /CableRegistryKey=<HKCU subkey> to point the check at fake endpoints. }
function VirtualCableInstalled: Boolean;
var
  Root: Integer;
  Key: String;
  Endpoints: TArrayOfString;
  I: Integer;
  State: Cardinal;
  Props, Desc, Iface: String;
begin
  Result := False;
  Root := HKLM;
  Key := ExpandConstant('{param:CableRegistryKey|}');
  if Key = '' then
    Key := RenderKey
  else
    Root := HKCU;
  if not RegGetSubkeyNames(Root, Key, Endpoints) then
    Exit;
  for I := 0 to GetArrayLength(Endpoints) - 1 do
  begin
    if RegQueryDWordValue(Root, Key + '\' + Endpoints[I], 'DeviceState', State)
      and (State = DeviceStateActive) then
    begin
      Props := Key + '\' + Endpoints[I] + '\Properties';
      Desc := '';
      Iface := '';
      RegQueryStringValue(Root, Props, DeviceDescValue, Desc);
      RegQueryStringValue(Root, Props, InterfaceNameValue, Iface);
      if IsVirtualCableName(Desc + ' (' + Iface + ')') then
      begin
        Result := True;
        Exit;
      end;
    end;
  end;
end;

function NoVirtualCable: Boolean;
begin
  if not CableChecked then
  begin
    CableFound := VirtualCableInstalled;
    CableChecked := True;
  end;
  Result := not CableFound;
end;

{ The [Run] entry is skipped in silent mode, so log the result for installer\test-install.ps1. }
procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep <> ssPostInstall then
    Exit;
  if NoVirtualCable then
    Log('VoiceChanger: virtual cable not found')
  else
    Log('VoiceChanger: virtual cable found');
end;

{ /PURGEDATA on the uninstaller command line: delete user data without asking. }
function PurgeDataRequested: Boolean;
var
  I: Integer;
begin
  Result := False;
  for I := 1 to ParamCount do
    if CompareText(ParamStr(I), '/PURGEDATA') = 0 then
      Result := True;
end;

{ Setup never touches user data. The uninstaller asks (default No); a silent uninstall keeps it
  unless /PURGEDATA is passed. }
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Roaming, Local: String;
  Delete: Boolean;
begin
  if CurUninstallStep <> usPostUninstall then
    Exit;
  Roaming := ExpandConstant('{userappdata}\{#AppName}');
  Local := ExpandConstant('{localappdata}\{#AppName}');
  if not (DirExists(Roaming) or DirExists(Local)) then
    Exit;
  if PurgeDataRequested then
    Delete := True
  else if UninstallSilent then
    Delete := False
  else
    Delete := MsgBox('Also delete your voice models and settings?' + #13#10#13#10 + Roaming + #13#10 + Local,
      mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES;
  if Delete then
  begin
    DelTree(Roaming, True, True, True);
    DelTree(Local, True, True, True);
  end;
end;

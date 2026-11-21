; Mister Download Manager's Windows installer (Inno Setup 6). Built by tools/installer/build.ps1,
; which passes /DAppVersion=..., /DArch=... and /DBin=... and, when a certificate is set up,
; /DSign with /Smdmsign=...
;
; Install for the current user (no administrator rights, %LOCALAPPDATA%\Programs) or for
; everyone (Program Files); the wizard asks. Tasks: browser integration (native messaging host
; for Chrome, Edge, Brave and Firefox), .torrent files and magnet links, start with Windows, a
; desktop shortcut. A running MDM is asked to save and quit first (/exit). Uninstalling asks
; whether to keep the download list and settings; removing them also removes MDM's sign-ins from
; Windows Credential Manager (misterdownloadmanager --forget-sign-ins).

#ifndef AppVersion
  #define AppVersion "0.5.2"
#endif
#define AppName "Mister Download Manager"
#define AppExe "misterdownloadmanager.exe"
#define HostExe "misterdownloadmanager-nmhost.exe"
#define Root "..\.."
; The processor (build.ps1 -Arch): x64, arm64 or x86, an installer each; and the folder its
; programs were built in (target\<Rust's target>\release).
#ifndef Arch
  #define Arch "x64"
#endif
#ifndef Bin
  #define Bin Root + "\target\x86_64-pc-windows-msvc\release"
#endif

[Setup]
; Never change AppId: upgrades and the uninstaller find the installation by it.
AppId={{667DB4D8-32C2-4487-A0E5-5825BF29778C}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=Ahmed Nefzaoui
AppPublisherURL=https://github.com/anefzaoui/misterdownloadmanager
AppSupportURL=https://github.com/anefzaoui/misterdownloadmanager/issues
AppUpdatesURL=https://github.com/anefzaoui/misterdownloadmanager/releases
AppCopyright=Copyright (C) 2026 Ahmed Nefzaoui
DefaultDirName={autopf}\{#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
; The Run entry is the installing user's, also when installing for everyone.
UsedUserAreasWarning=no
; One installer per processor, all with the same AppId: the ARM64 one run on a Windows on ARM PC
; that has the x64 one replaces it in place. x64 runs on x64 Windows and, emulated, on Windows 11
; on ARM; ARM64 on Windows on ARM only; the 32-bit one on 32-bit Windows only (x86os): on a 64-bit
; one it would replace a per-user x64 copy (which would then update to the 32-bit files for good)
; or add a second copy in Program Files (x86). There Setup refuses with its own message, which
; names the processors it is for.
#if Arch == "arm64"
ArchitecturesAllowed=arm64
ArchitecturesInstallIn64BitMode=arm64
#elif Arch == "x86"
ArchitecturesAllowed=x86os
#else
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
#endif
MinVersion=10.0
WizardStyle=modern
SetupIconFile={#Root}\crates\app\assets\icons\app\mdm.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
OutputDir={#Root}\target\dist
; x64's keeps the name it had before there were others: MDM's Update now looks for it by name.
#if Arch == "x64"
OutputBaseFilename=MisterDownloadManager-{#AppVersion}-setup
#else
OutputBaseFilename=MisterDownloadManager-{#AppVersion}-setup-{#Arch}
#endif
; A 64 MB dictionary (#25): an 8 MB one (lzma2/max) only finds what repeats close by in the
; solid archive. A few MB smaller; building needs about 750 MB of memory, installing 64 MB.
Compression=lzma2/ultra64
SolidCompression=yes
ChangesAssociations=yes
; A running MDM is asked to quit by [Code] (it holds the mutex MisterDownloadManager.Running);
; AppMutex isn't used because it would refuse to start before that. Restart Manager is the
; fallback for anything else holding the files (a browser running the host).
CloseApplications=yes
VersionInfoVersion={#AppVersion}
VersionInfoCompany=Ahmed Nefzaoui
VersionInfoDescription={#AppName} setup
VersionInfoProductName={#AppName}
#ifdef Sign
SignTool=mdmsign
SignedUninstaller=yes
  #define SignFlag "signonce"
#else
  #define SignFlag ""
#endif

; Every interface language (i18n/languages.tsv): tools/installer/languages.py writes the list and
; the translations of the messages below. The wizard starts in Windows' display language.
#include "languages.iss"

[CustomMessages]
lang_en.TaskBrowser=Set up &browser integration (Chrome, Edge, Brave, Firefox)
lang_en.TaskTorrents=Open .&torrent files and magnet links with {#AppName}
lang_en.TaskStartup=Start {#AppName} when &Windows starts
lang_en.TaskGroupOther=Other:
lang_en.Launch=Start {#AppName}
lang_en.KeepData=Also delete your download list and settings?%n%nYour downloaded files are not touched either way.

; 32-bit Windows' setup run on 64-bit Windows (x86os above): which setup to take instead. Other
; languages show Setup's own message, that this Windows isn't supported.
#if Arch == "x86"
[Messages]
lang_en.WindowsVersionNotSupported=This is the setup for 32-bit Windows, and this computer runs 64-bit Windows. Download MisterDownloadManager-{#AppVersion}-setup.exe instead (or MisterDownloadManager-{#AppVersion}-setup-arm64.exe on a Windows on ARM PC).
#endif

[Tasks]
Name: "browser"; Description: "{cm:TaskBrowser}"
Name: "torrents"; Description: "{cm:TaskTorrents}"
Name: "startup"; Description: "{cm:TaskStartup}"
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:TaskGroupOther}"; Flags: unchecked

[Files]
Source: "{#Bin}\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion {#SignFlag}
Source: "{#Bin}\{#HostExe}"; DestDir: "{app}"; Flags: ignoreversion {#SignFlag}
Source: "{#Bin}\misterdownloadmanager-cli.exe"; DestDir: "{app}"; Flags: ignoreversion {#SignFlag}
Source: "{#Bin}\misterdownloadmanager-mcp.exe"; DestDir: "{app}"; Flags: ignoreversion {#SignFlag}
; The extension, for "Load unpacked" (Chromium) and about:debugging (Firefox) until it is in
; the stores.
Source: "{#Root}\target\extension\chromium\*"; DestDir: "{app}\extension\chromium"; Flags: ignoreversion recursesubdirs
Source: "{#Root}\target\extension\mdm-firefox.xpi"; DestDir: "{app}\extension"; Flags: ignoreversion
Source: "{#Root}\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion
Source: "{#Root}\ATTRIBUTION.md"; DestDir: "{app}"; DestName: "ATTRIBUTION.txt"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
; The language chosen in the wizard ({language}: "lang_" + a languages.tsv code with "_" for "-",
; see languages.py inno_name). The app
; takes it on its first start, when it has no language setting yet (settings.rs), then deletes it.
Root: HKCU; Subkey: "Software\MisterDownloadManager"; ValueType: string; ValueName: "InstallLanguage"; ValueData: "{language}"; Flags: uninsdeletevalue uninsdeletekeyifempty
; Start with Windows: the same value Options -> "Launch on startup" writes and reads, with the
; argument that starts MDM in the notification area (#226; win32.rs startup_command).
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "MisterDownloadManager"; ValueData: """{app}\{#AppExe}"" /startup"; Flags: uninsdeletevalue; Tasks: startup
; The "torrents" task, ticked or not: Torrent settings' switch on the app's first start that hasn't
; decided it (#1058: it then also decides whether the browser hands .torrent downloads over;
; settings.rs settle_torrent_handler). Only read, once.
Root: HKCU; Subkey: "Software\MisterDownloadManager"; ValueType: string; ValueName: "InstallTorrents"; ValueData: "1"; Flags: uninsdeletevalue uninsdeletekeyifempty; Tasks: torrents
Root: HKCU; Subkey: "Software\MisterDownloadManager"; ValueType: string; ValueName: "InstallTorrents"; ValueData: "0"; Flags: uninsdeletevalue uninsdeletekeyifempty; Tasks: not torrents
; .torrent files and magnet links (HKA: the user's or everyone's classes, as installed).
Root: HKA; Subkey: "Software\Classes\MisterDownloadManager.torrent"; ValueType: string; ValueName: ""; ValueData: "Torrent file"; Flags: uninsdeletekey; Tasks: torrents
Root: HKA; Subkey: "Software\Classes\MisterDownloadManager.torrent\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\{#AppExe},0"; Tasks: torrents
Root: HKA; Subkey: "Software\Classes\MisterDownloadManager.torrent\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" ""%1"""; Tasks: torrents
Root: HKA; Subkey: "Software\Classes\.torrent"; ValueType: string; ValueName: ""; ValueData: "MisterDownloadManager.torrent"; Flags: uninsdeletekeyifempty; Tasks: torrents
Root: HKA; Subkey: "Software\Classes\.torrent\OpenWithProgids"; ValueType: string; ValueName: "MisterDownloadManager.torrent"; ValueData: ""; Flags: uninsdeletevalue uninsdeletekeyifempty; Tasks: torrents
Root: HKA; Subkey: "Software\Classes\magnet"; ValueType: string; ValueName: ""; ValueData: "URL:Magnet link"; Tasks: torrents
Root: HKA; Subkey: "Software\Classes\magnet"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""; Tasks: torrents
Root: HKA; Subkey: "Software\Classes\magnet\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\{#AppExe},0"; Tasks: torrents
Root: HKA; Subkey: "Software\Classes\magnet\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" ""%1"""; Tasks: torrents
; Windows' Default apps (#1091): MDM listed for MAGNET and .torrent, where the user picks it (a
; choice recorded there for another program stays first, and no program may change it). The
; same keys and names as win32.rs set_torrent_handlers; ApplicationName matches the
; RegisteredApplications value's name, and each association has an MDM-only ProgID. The app
; writes its description in the interface language for the user's own registration.
Root: HKA; Subkey: "Software\Classes\MisterDownloadManager.Magnet"; ValueType: string; ValueName: ""; ValueData: "URL:Magnet link"; Flags: uninsdeletekey; Tasks: torrents
Root: HKA; Subkey: "Software\Classes\MisterDownloadManager.Magnet"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""; Tasks: torrents
Root: HKA; Subkey: "Software\Classes\MisterDownloadManager.Magnet\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"",0"; Tasks: torrents
Root: HKA; Subkey: "Software\Classes\MisterDownloadManager.Magnet\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" ""%1"""; Tasks: torrents
Root: HKA; Subkey: "Software\MisterDownloadManager\Capabilities"; ValueType: string; ValueName: "ApplicationName"; ValueData: "{#AppName}"; Flags: uninsdeletekey; Tasks: torrents
Root: HKA; Subkey: "Software\MisterDownloadManager\Capabilities"; ValueType: string; ValueName: "ApplicationDescription"; ValueData: "Downloads files, videos and torrents"; Tasks: torrents
Root: HKA; Subkey: "Software\MisterDownloadManager\Capabilities"; ValueType: string; ValueName: "ApplicationIcon"; ValueData: """{app}\{#AppExe}"",0"; Tasks: torrents
Root: HKA; Subkey: "Software\MisterDownloadManager\Capabilities\FileAssociations"; ValueType: string; ValueName: ".torrent"; ValueData: "MisterDownloadManager.Torrent"; Tasks: torrents
Root: HKA; Subkey: "Software\MisterDownloadManager\Capabilities\UrlAssociations"; ValueType: string; ValueName: "magnet"; ValueData: "MisterDownloadManager.Magnet"; Tasks: torrents
Root: HKA; Subkey: "Software\RegisteredApplications"; ValueType: string; ValueName: "{#AppName}"; ValueData: "Software\MisterDownloadManager\Capabilities"; Flags: uninsdeletevalue; Tasks: torrents

[Run]
Filename: "{app}\{#HostExe}"; Parameters: "--register"; Flags: runhidden; Tasks: browser; Check: not IsAdminInstallMode
Filename: "{app}\{#HostExe}"; Parameters: "--register --all-users"; Flags: runhidden; Tasks: browser; Check: IsAdminInstallMode
Filename: "{app}\{#AppExe}"; Description: "{cm:Launch}"; Flags: nowait postinstall skipifsilent
; Update now in MDM runs this setup silently with /relaunch=1: MDM starts again once installed,
; as the user (not as the administrator an all-users update asked for).
Filename: "{app}\{#AppExe}"; Parameters: "/updated"; Flags: nowait runasoriginaluser; Check: Relaunch

[UninstallRun]
Filename: "{app}\{#HostExe}"; Parameters: "--unregister"; Flags: runhidden; RunOnceId: "UnregisterUser"; Check: not IsAdminInstallMode
Filename: "{app}\{#HostExe}"; Parameters: "--unregister --all-users"; Flags: runhidden; RunOnceId: "UnregisterAll"; Check: IsAdminInstallMode

[UninstallDelete]
Type: files; Name: "{app}\com.misterdownloadmanager.host*.json"

[Code]
const
  RunningMutex = 'MisterDownloadManager.Running';

{ /relaunch=1 on the command line (MDM's Update now): start MDM again after installing. }
function Relaunch: Boolean;
begin
  Result := ExpandConstant('{param:relaunch|0}') = '1';
end;

{ Ask a running MDM to save and quit, and wait up to 15 s for it. }
procedure ExitRunningApp(Exe: String);
var
  Code, Waited: Integer;
begin
  if CheckForMutexes(RunningMutex) and FileExists(Exe) then
  begin
    Exec(Exe, '/exit', '', SW_HIDE, ewWaitUntilTerminated, Code);
    Waited := 0;
    while CheckForMutexes(RunningMutex) and (Waited < 15000) do
    begin
      Sleep(250);
      Waited := Waited + 250;
    end;
  end;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  ExitRunningApp(ExpandConstant('{app}\{#AppExe}'));
  Result := '';
end;

function InitializeUninstall(): Boolean;
begin
  ExitRunningApp(ExpandConstant('{app}\{#AppExe}'));
  Result := True;
end;

{ Remove a class key only while it still points at us (another program may have taken over
  magnet links or .torrent files since). }
procedure ForgetIfOurs(Key, Ours: String);
var
  Value: String;
begin
  if RegQueryStringValue(HKA, Key + '\shell\open\command', '', Value) and (Pos(Lowercase(Ours), Lowercase(Value)) > 0) then
    RegDeleteKeyIncludingSubkeys(HKA, Key);
end;

{ Asked before the files go (the sign-ins are removed by MDM itself), the list and settings
  removed after. A silent uninstall keeps both. }
var
  RemoveData: Boolean;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Value, Data: String;
  Code: Integer;
begin
  Data := ExpandConstant('{userappdata}\misterdownloadmanager');
  if CurUninstallStep = usUninstall then
  begin
    ForgetIfOurs('Software\Classes\magnet', ExpandConstant('{app}\{#AppExe}'));
    if RegQueryStringValue(HKA, 'Software\Classes\.torrent', '', Value) and (Value = 'MisterDownloadManager.torrent') then
      RegDeleteValue(HKA, 'Software\Classes\.torrent', '');
    { Default apps (#1091): what the app registered for this user (Torrent settings' switch),
      also when this install was everyone's. }
    if RegQueryStringValue(HKCU, 'Software\RegisteredApplications', '{#AppName}', Value) and (CompareText(Value, 'Software\MisterDownloadManager\Capabilities') = 0) then
      RegDeleteValue(HKCU, 'Software\RegisteredApplications', '{#AppName}');
    RegDeleteKeyIncludingSubkeys(HKCU, 'Software\MisterDownloadManager\Capabilities');
    if RegQueryStringValue(HKCU, 'Software\Classes\MisterDownloadManager.Magnet\shell\open\command', '', Value) and (Pos(Lowercase(ExpandConstant('{app}\{#AppExe}')), Lowercase(Value)) > 0) then
      RegDeleteKeyIncludingSubkeys(HKCU, 'Software\Classes\MisterDownloadManager.Magnet');
    RemoveData := False;
    if DirExists(Data) and not UninstallSilent() then
      RemoveData := MsgBox(CustomMessage('KeepData'), mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES;
    { The sign-ins MDM keeps in Windows Credential Manager (only MDM's own entries). }
    if RemoveData then
      Exec(ExpandConstant('{app}\{#AppExe}'), '--forget-sign-ins', '', SW_HIDE, ewWaitUntilTerminated, Code);
  end;
  if (CurUninstallStep = usPostUninstall) and RemoveData then
    DelTree(Data, True, True, True);
end;

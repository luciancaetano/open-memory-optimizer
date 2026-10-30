; OpenMemoryOptimizer - Inno Setup script
; Copyright (C) 2026 Lucian Caetano
; SPDX-License-Identifier: GPL-3.0-or-later
;
; Built by build.cmd, which passes the version from Cargo.toml:
;   ISCC.exe /DAppVersion=1.0.0 installer\OpenMemoryOptimizer.iss

#ifndef AppVersion
  #define AppVersion "1.0.0"
#endif
#define AppName "OpenMemoryOptimizer"
#define AppExe "OpenMemoryOptimizer.exe"
#define AppPublisher "Lucian Caetano"
; Must match schedule::TASK_NAME in src/schedule.rs
#define ScheduleTask "OpenMemoryOptimizer Task"
#define StartupTask "OpenMemoryOptimizer Startup"

[Setup]
; Never change AppId: Windows uses it to recognise upgrades of the same app.
AppId={{623E8AB3-2789-419E-B542-60ECBF6D3264}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
VersionInfoVersion={#AppVersion}
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
LicenseFile=..\LICENSE
SetupIconFile=..\icon.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
OutputDir=..\dist
OutputBaseFilename={#AppName}-Setup-{#AppVersion}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; The app itself requires administrator rights, so install machine-wide.
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
; The app hides to the tray instead of closing, so close it forcibly on upgrade.
CloseApplications=force
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "brazilianportuguese"; MessagesFile: "compiler:Languages\BrazilianPortuguese.isl"

[CustomMessages]
english.AutoStart=Start {#AppName} when I sign in (runs in the system tray)
brazilianportuguese.AutoStart=Iniciar o {#AppName} ao entrar no Windows (na bandeja do sistema)
english.StartupGroup=Startup:
brazilianportuguese.StartupGroup=Inicialização:

[Tasks]
Name: "autostart"; Description: "{cm:AutoStart}"; GroupDescription: "{cm:StartupGroup}"
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
; An app that requires administrator rights cannot auto-start from the Run registry key,
; so sign-in start uses a scheduled task with highest privileges.
Filename: "{sys}\schtasks.exe"; \
  Parameters: "/create /tn ""{#StartupTask}"" /tr ""\""{app}\{#AppExe}\"" --minimized"" /sc onlogon /rl highest /f"; \
  Flags: runhidden; Tasks: autostart
Filename: "{sys}\schtasks.exe"; Parameters: "/delete /tn ""{#StartupTask}"" /f"; \
  Flags: runhidden; Tasks: not autostart
; postinstall entries run as the original (non-elevated) user by default, and the app's
; requireAdministrator manifest then fails with "requires elevation". Inherit Setup's rights.
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; \
  Flags: nowait postinstall skipifsilent runascurrentuser

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/IM {#AppExe} /F"; Flags: runhidden; RunOnceId: "StopApp"
Filename: "{sys}\schtasks.exe"; Parameters: "/delete /tn ""{#StartupTask}"" /f"; Flags: runhidden; RunOnceId: "DeleteStartupTask"
Filename: "{sys}\schtasks.exe"; Parameters: "/delete /tn ""{#ScheduleTask}"" /f"; Flags: runhidden; RunOnceId: "DeleteScheduleTask"

[UninstallDelete]
; Settings written next to the executable at runtime.
Type: files; Name: "{app}\MemoryConfig.ini"

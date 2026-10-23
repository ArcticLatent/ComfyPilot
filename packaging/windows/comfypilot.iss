; ComfyPilot's Windows installer (Inno Setup 6), built by build.ps1:
;
;   iscc /DAppVersion=1.0.0 /DSourceDir=<staged app> /DOutputDir=<dist>
;        /DCleanupFile=<InstallDelete entries for the staged folders> comfypilot.iss
;
; A per-user install (no admin rights) into %LOCALAPPDATA%\Programs\ComfyPilot.
; The in-app updater runs it with /VERYSILENT /CLOSEAPPLICATIONS; [Run] starts
; the app again afterwards (no skipifsilent, so that also happens on updates).

#ifndef AppVersion
  #error Pass /DAppVersion=<version>
#endif
#ifndef SourceDir
  #error Pass /DSourceDir=<folder with ComfyPilot.exe and its Qt files>
#endif
#ifndef CleanupFile
  #error Pass /DCleanupFile=<file with the [InstallDelete] entries>
#endif
#ifndef OutputDir
  #define OutputDir "."
#endif

[Setup]
AppId={{566DF5AA-D783-4B06-BD0E-96CE09414A3B}
AppName=ComfyPilot
AppVersion={#AppVersion}
AppVerName=ComfyPilot {#AppVersion}
AppPublisher=Arctic Latent
AppPublisherURL=https://github.com/ArcticLatent/ComfyPilot
AppSupportURL=https://github.com/ArcticLatent/ComfyPilot/issues
AppUpdatesURL=https://github.com/ArcticLatent/ComfyPilot/releases
VersionInfoVersion={#AppVersion}
PrivilegesRequired=lowest
DefaultDirName={localappdata}\Programs\ComfyPilot
DisableProgramGroupPage=yes
DisableReadyPage=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.17763
CloseApplications=yes
RestartApplications=no
SetupIconFile=..\..\app\icons\icon.ico
UninstallDisplayIcon={app}\ComfyPilot.exe
UninstallDisplayName=ComfyPilot
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
OutputDir={#OutputDir}
OutputBaseFilename=ComfyPilot-Setup-{#AppVersion}

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[InstallDelete]
; The previous version's Qt files (DLLs and every folder windeployqt made),
; so none are left behind when a release drops or renames one.
Type: files; Name: "{app}\*.dll"
#include CleanupFile

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\ComfyPilot"; Filename: "{app}\ComfyPilot.exe"
Name: "{autodesktop}\ComfyPilot"; Filename: "{app}\ComfyPilot.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\ComfyPilot.exe"; Description: "{cm:LaunchProgram,ComfyPilot}"; Flags: nowait postinstall

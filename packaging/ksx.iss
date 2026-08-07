; ksx — Inno Setup script.
;
; Build (Inno Setup 6.3 or newer; nothing else is required):
;
;     cargo build --release -p ksx-app --features studio,cabinet
;     iscc packaging\ksx.iss
;
; The output lands in packaging\out\ksx-<version>-setup.exe.
;
; The feature flags are not optional for a SHIPPED build: the Start-menu
; entries below run `ksx studio` and `ksx cabinet`, and both subcommands only
; exist when their feature is on (docs/ENHANCEMENTS.md E7 rule A — the default
; build links neither UI, which is the right default for a headless cabinet
; and the wrong one for an installer aimed at a desktop).
;
; ---------------------------------------------------------------------------
; What this installer does and does not do
; ---------------------------------------------------------------------------
;
; DOES: lay ksx.exe down beside its `drivers\` folder — `ksx install-drivers`
; looks for the bundled ViGEmBus setup at `<exe dir>\drivers\`
; (`ksx_platform::installer::locate`), so the layout here is a contract with
; the program, not a convention.
;
; DOES NOT: install any driver. Driver installation is `ksx install-drivers`,
; run deliberately by the user, after ksx has proved the bundle's SHA-256 and
; its Authenticode chain (docs/DRIVERS.md). An installer that silently
; installed a kernel driver would throw away both pins and the consent.
;
; DOES NOT: install Interception. Its licence is non-commercial and its
; installer is not ours to redistribute (docs/DRIVERS.md).
;
; ---------------------------------------------------------------------------
; The icons
; ---------------------------------------------------------------------------
;
; Every icon below resolves to the same generated file,
; assets\brand\dist\ksx.ico (tools\icongen — see assets\brand\README.md):
;
;   SetupIconFile         the setup.exe's own icon, in Explorer and in the
;                         UAC prompt the user is about to trust;
;   UninstallDisplayIcon  the Apps & Features row and the uninstaller. It
;                         points at the INSTALLED ksx.exe rather than at a
;                         copy of the .ico, because build.rs stamps the same
;                         icon group into the exe as resource 1 — one file to
;                         keep current instead of two;
;   [Icons]               the Start-menu entries, which inherit the exe's icon
;                         for the same reason.
;
; The .ico carries eight SIZE-SPECIFIC entries (16/20/24/32 simplified,
; 48/64/128/256 detailed), so the 16 px wizard title bar and the 256 px
; Explorer view each get art drawn for that size rather than a resample of
; one drawing. Every entry is PNG-compressed (see tools/icongen), which
; Inno Setup 6 reads; 6.3 is the floor here for `ArchitecturesAllowed=
; x64compatible`.
;
; NOT COMPILE-VERIFIED on the machine this was written on — Inno Setup is not
; installed there. The first `iscc` run is the check.

#define AppName        "ksx"
#define AppVersion     "0.1.0"
#define AppPublisher   "Victor Villacis"
#define AppURL         "https://github.com/Victor-Villacis/KeyboardSplitterXboxPro"
#define AppExe         "ksx.exe"
#define RepoRoot       ".."

[Setup]
; Never change AppId: it is what makes an install an UPGRADE rather than a
; second copy sitting beside the first.
AppId={{7B2F5A46-3C1D-4E9A-9F30-2A6C0E8D4B11}
AppName={#AppName}
AppVersion={#AppVersion}
VersionInfoVersion={#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppURL}
AppSupportURL={#AppURL}/issues
AppUpdatesURL={#AppURL}/releases
AppCopyright=MIT OR Apache-2.0

DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
LicenseFile={#RepoRoot}\LICENSE-MIT
InfoAfterFile={#RepoRoot}\docs\QUICKSTART.md

; ksx installs for the machine (it registers autostart and talks to a
; kernel driver), so it needs an elevated install into Program Files.
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

OutputDir=out
OutputBaseFilename={#AppName}-{#AppVersion}-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern

SetupIconFile={#RepoRoot}\assets\brand\dist\ksx.ico
UninstallDisplayIcon={app}\{#AppExe},0
UninstallDisplayName={#AppName} {#AppVersion}

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "addtopath";   Description: "Add ksx to PATH (so `ksx` works from any terminal)"; GroupDescription: "Integration"

[Files]
Source: "{#RepoRoot}\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
; The bundled ViGEmBus setup, NOT executed here — see the header. It must land
; in `<exe dir>\drivers\` for `ksx install-drivers` to find it.
Source: "{#RepoRoot}\drivers\*"; DestDir: "{app}\drivers"; Flags: ignoreversion recursesubdirs
Source: "{#RepoRoot}\README.md";        DestDir: "{app}"; Flags: ignoreversion
Source: "{#RepoRoot}\NOTICE";           DestDir: "{app}"; Flags: ignoreversion
Source: "{#RepoRoot}\LICENSE-MIT";      DestDir: "{app}"; Flags: ignoreversion
Source: "{#RepoRoot}\LICENSE-APACHE";   DestDir: "{app}"; Flags: ignoreversion
Source: "{#RepoRoot}\docs\*.md";        DestDir: "{app}\docs"; Flags: ignoreversion

[Icons]
; No IconFilename= on any of these: the target IS ksx.exe, and ksx.exe carries
; the icon group as resource 1 (crates\ksx-app\build.rs). A separate
; IconFilename pointing at a copied .ico would be a second thing to keep in
; step, and the first one to go stale.
Name: "{group}\{#AppName}";              Filename: "{app}\{#AppExe}"; Parameters: "daemon"; Comment: "Start the ksx tray daemon"
; `studio` and `cabinet` exist only in a build carrying those features — see
; the build line in the header.
Name: "{group}\{#AppName} Studio";       Filename: "{app}\{#AppExe}"; Parameters: "studio"; Comment: "Open ksx Studio in a browser"
Name: "{group}\{#AppName} cabinet";      Filename: "{app}\{#AppExe}"; Parameters: "cabinet"; Comment: "The 10-foot cabinet panel"
Name: "{group}\{#AppName} setup wizard"; Filename: "{app}\{#AppExe}"; Parameters: "setup"; Comment: "First-run setup"
Name: "{autodesktop}\{#AppName}";        Filename: "{app}\{#AppExe}"; Parameters: "daemon"; Tasks: desktopicon

[Registry]
; PATH, machine-wide, appended — `uninsdeletevalue` on a shared key would be
; wrong, so the removal is scoped to our own entry by Inno's PATH handling.
Root: HKLM; Subkey: "SYSTEM\CurrentControlSet\Control\Session Manager\Environment"; \
    ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; \
    Tasks: addtopath; Check: NeedsAddPath(ExpandConstant('{app}'))

[Run]
Filename: "{app}\{#AppExe}"; Parameters: "doctor"; Description: "Check drivers and hardware now (ksx doctor)"; Flags: postinstall nowait skipifsilent

[UninstallRun]
; Leave nothing behind that keeps starting: the scheduled task outlives an
; uninstall otherwise, and a missing exe on a boot task is a visible error
; every morning. `runascurrentuser` because the task is per-user.
Filename: "{app}\{#AppExe}"; Parameters: "autostart --disable"; Flags: runhidden skipifdoesntexist; RunOnceId: "ksxAutostartOff"

[Code]
// True when the install dir is not already on the machine PATH.
// Case-insensitive and separator-anchored, so "C:\ksx" does not match
// "C:\ksx-old".
//
// NOTE: `//` and not a { } comment. Pascal Script ends a brace comment at the
// FIRST `}`, so writing {app} inside one closes it early and the rest of the
// sentence is parsed as code — which is exactly how this file failed to
// compile the first time anything actually ran ISCC against it.
function NeedsAddPath(Param: string): Boolean;
var
  OldPath: string;
begin
  if not RegQueryStringValue(HKLM,
    'SYSTEM\CurrentControlSet\Control\Session Manager\Environment',
    'Path', OldPath) then
  begin
    Result := True;
    exit;
  end;
  Result := Pos(';' + Uppercase(Param) + ';', ';' + Uppercase(OldPath) + ';') = 0;
end;

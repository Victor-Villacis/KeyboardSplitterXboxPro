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
; entries below run `ksx open`, `ksx studio` and `ksx cabinet`, and those
; subcommands only exist when their feature is on (docs/ENHANCEMENTS.md E7
; rule A — the default build links neither UI, which is the right default for
; a headless cabinet and the wrong one for an installer aimed at a desktop).
; `open` is gated with `studio` because it is Studio it opens.
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
; The Start-menu subfolder every surface that is NOT the product lives in.
; Spelled once because it appears on five entries and a typo makes a sixth
; folder rather than an error. See the [Icons] section for why it exists.
#define AdvancedGroup  "ksx (advanced)"

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
; CHECKED, deliberately — docs/FIRST-RUN.md §4 bullet 1. It used to carry
; `Flags: unchecked`, and the audit's finding was concrete: this installer's
; only other hand-off is the "run it now" checkbox at the end, so a user who
; declined that one was left with nothing on screen and a Start menu to hunt
; through. An icon on the desktop is what "installed" looks like to the person
; FIRST-RUN.md is written about.
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"
; UNCHECKED, equally deliberately — §4 bullet 4. PATH buys exactly one thing:
; typing `ksx` in a terminal. FIRST-RUN.md's premise is that the customer never
; opens one (and docs/SURFACES.md §3 keeps the CLI a development surface), so
; the default must not be "edit a machine-wide environment variable to buy the
; installing user nothing".
;
; ASCII ONLY in this Description, and in every Comment below. This file has no
; UTF-8 BOM, so ISCC reads it in the system code page: a byte above 127 in a
; string the USER sees becomes mojibake in a shortcut tooltip or a wizard
; checkbox. Comments are discarded by the compiler and may keep their dashes.
Name: "addtopath";   Description: "Add ksx to PATH (for the `ksx` command in a terminal; not needed to use ksx)"; GroupDescription: "Integration"; Flags: unchecked

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
; ---------------------------------------------------------------------------
; ONE entry at the top level, and it is the product (docs/FIRST-RUN.md §4
; bullet 3).
; ---------------------------------------------------------------------------
;
; This section used to put FIVE names in front of a new user: "ksx", "ksx
; daemon (tray only)", "ksx Studio (serve only)", "ksx cabinet" and "ksx setup
; wizard". Four of those are surfaces and development verbs. Nothing on screen
; told a first-time user which one was the application, and three of them —
; a tray icon with no window, a server with no client, a 10-foot panel meant to
; be driven by an arcade stick — LOOK BROKEN when opened with a mouse on a
; desktop. A menu that cannot be ranked teaches nothing and mis-teaches
; plenty.
;
; The four are NOT deleted; deleting them would take the tray-only daemon (the
; right thing on a cabinet or over RDP) and the serve-only Studio (the
; documented recovery path, docs/M9-DECISION.md §4 item 7) away from the only
; people who need them. They move one level down, into a folder named
; "ksx (advanced)". A folder is a question a user answers before reading its
; contents: someone who opens it has already decided they want something other
; than "ksx", which is precisely the population those entries serve. And since
; FIRST-RUN.md's premise is that the customer never types `ksx <verb>`, a
; Start-menu folder is also the only place these stay reachable at all without
; a shell.
;
; No IconFilename= on any entry whose target IS ksx.exe: the exe carries the
; icon group as resource 1 (crates\ksx-app\build.rs), so the shortcut inherits
; it. A separate IconFilename pointing at a copied .ico would be a second thing
; to keep in step, and the first one to go stale. The one exception is the
; doctor entry, whose target is the command processor — see it below.
;
; THE PLAIN "ksx" ENTRY RUNS `open`, NOT `daemon` (docs/M9-DECISION.md §4
; item 1). It used to run the daemon, which put a tray icon on screen and
; nothing else: the entry a person double-clicks appeared to do nothing, and
; the way to actually see ksx was to type a URL. `open` starts the daemon if
; one is not running, waits for it and for Studio, and then shows a window.
Name: "{group}\{#AppName}";       Filename: "{app}\{#AppExe}"; Parameters: "open"; Comment: "Open ksx"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Parameters: "open"; Comment: "Open ksx"; Tasks: desktopicon

; --- ksx (advanced) --------------------------------------------------------
; Everything below is a surface or a dev verb, and every one of them is the
; right answer to SOME question — just never to "I just installed this, what
; do I click?".
;
; `open`, `studio` and `cabinet` exist only in a build carrying those features
; — see the build line in the header.
;
; The tray-only daemon is the plain entry's OLD behaviour, kept because
; starting the tray without opening a window is what you want on a cabinet,
; over RDP, or when Studio is not the point.
Name: "{group}\{#AdvancedGroup}\daemon (tray only)"; Filename: "{app}\{#AppExe}"; Parameters: "daemon"; Comment: "Start the ksx tray daemon without opening a window"
; This entry SERVES Studio and opens nothing; the window is what "ksx" is for.
Name: "{group}\{#AdvancedGroup}\Studio (serve only)"; Filename: "{app}\{#AppExe}"; Parameters: "studio"; Comment: "Serve ksx Studio on 127.0.0.1:4460 for another device to open"
Name: "{group}\{#AdvancedGroup}\cabinet panel"; Filename: "{app}\{#AppExe}"; Parameters: "cabinet"; Comment: "The 10-foot cabinet panel, driven by the arcade panel rather than a mouse"
Name: "{group}\{#AdvancedGroup}\setup wizard"; Filename: "{app}\{#AppExe}"; Parameters: "setup"; Comment: "The console setup wizard"
; `ksx doctor` stays one click away — it is just not the hand-off any more
; (§4 bullet 2, and the [Run] section below).
;
; Through the command processor and not straight at ksx.exe, because ksx.exe is
; a CONSOLE subsystem binary (crates\ksx-app\src\console.rs, deliberately): a
; shortcut that ran `doctor` directly would print its driver tables into a
; console that closes the instant the process exits, which shows the one user
; who came here for those tables nothing at all. `/k` keeps the window. That
; target is cmd.exe, so this is the one entry that must name an icon.
Name: "{group}\{#AdvancedGroup}\driver check (ksx doctor)"; Filename: "{cmd}"; Parameters: "/k ""{app}\{#AppExe}"" doctor"; WorkingDir: "{app}"; IconFilename: "{app}\{#AppExe}"; IconIndex: 0; Comment: "Check drivers and hardware, in a window that stays open"

[Registry]
; PATH, machine-wide, appended — `uninsdeletevalue` on a shared key would be
; wrong, so the removal is scoped to our own entry by Inno's PATH handling.
Root: HKLM; Subkey: "SYSTEM\CurrentControlSet\Control\Session Manager\Environment"; \
    ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; \
    Tasks: addtopath; Check: NeedsAddPath(ExpandConstant('{app}'))

[Run]
; The hand-off is THE PRODUCT (docs/FIRST-RUN.md §4 bullet 2). This line used
; to run `ksx doctor`: a person who ticked "run this now" — the one moment the
; installer has their consent to show them what they just bought — got a
; console full of driver tables. That is a developer's answer to a question
; they did not ask, and it is the last screen of the install, so it is also
; their first impression of ksx.
;
; `open` is the same verb both icons run. It starts the daemon if one is not
; running, waits for it and for Studio, then puts a window on screen
; (crates\ksx-app\src\studio_launch.rs) — moment 3 of FIRST-RUN.md §1.
; `nowait` because that wait is seconds long and the wizard must not hold its
; Finish button hostage for it; `open` exits by design once the window is up.
;
; `runasoriginaluser` matters more than it looks. Setup is elevated
; (PrivilegesRequired=admin, for Program Files and the driver bundle), and
; without this flag the whole chain — `ksx open`, the daemon it starts, and the
; browser window that daemon's Studio ends up in — inherits that token. ksx is
; built to run WITHOUT one: `ksx autostart` registers its logon task as
; InteractiveToken/LeastPrivilege, "never elevated"
; (crates\ksx-app\src\autostart.rs), so an elevated first daemon would make
; moment 3 behave differently from every boot after it. It would also put the
; Chromium profile ksx owns under the ELEVATING account's %LOCALAPPDATA%, which
; on a machine where a standard user typed an admin's credentials is not the
; profile the user gets tomorrow.
Filename: "{app}\{#AppExe}"; Parameters: "open"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: postinstall nowait skipifsilent runasoriginaluser

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

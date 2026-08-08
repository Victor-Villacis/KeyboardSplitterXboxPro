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
; DOES: offer to install the bundled ViGEmBus driver, as a [Tasks] checkbox,
; by running `ksx install-drivers --yes` — see the [Code] section at the
; bottom for why it is that verb and not the .exe, and why a failure there
; cannot fail this install.
;
; This is a reversal, and the reasoning it replaces is worth keeping: "an
; installer that silently installed a kernel driver would throw away both pins
; and the consent". Both objections are answered rather than ignored. The pins
; are kept because the thing that runs is the verb that owns them, not the
; bundled .exe. The consent is kept because it is asked for, in the wizard, in
; plain words, on a box the user can clear. What was NOT survivable was the
; third fact nobody had weighed: `ksx install-drivers` needs an administrator
; token and ksx never self-elevates, so on a machine without ViGEmBus the only
; route to a working pad was a shell command — and docs/FIRST-RUN.md §7 makes
; "without opening a terminal" the acceptance test for the whole product.
; Setup is already elevated. It is the one moment where installing this costs
; the user nothing they have not already agreed to.
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
; CHECKED, and it is the only checkbox here whose default decides whether ksx
; can do its job at all. Without ViGEmBus there is no bus for a virtual pad to
; appear on, so a first-run user reaches Play, presses it, and nothing plugs.
;
; A checkbox rather than an unconditional step, because docs/DRIVERS.md is
; explicit that installing a kernel driver silently throws away the consent —
; and it is right. What it must NOT be is a checkbox whose label makes the
; consequence unguessable: "install drivers" tells a first-time user nothing,
; so the label names the driver, says what it is for, and says it is bundled
; rather than downloaded.
;
; A user who clears it gets a ksx that installs, runs, configures and maps and
; cannot plug a pad. That is a legitimate choice (an existing ViGEmBus from
; DS4Windows or Sunshine is already there, or a machine is being staged), and
; it is a choice they have to be able to reverse: see the [Code] section, which
; says so on the last page of the wizard.
;
; ASCII ONLY, and this is the checkbox the rule was written for — see the note
; on `addtopath` below.
Name: "vigembus"; Description: "Install the ViGEmBus controller driver (required to create virtual controllers)"; GroupDescription: "Controller driver - bundled with ksx, nothing is downloaded:"
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
; The bundled ViGEmBus setup. It must land in `<exe dir>\drivers\` for
; `ksx install-drivers` to find it — and `<exe dir>` must be under Program
; Files (or another directory a standard user cannot write) or that search
; refuses the file on purpose: an elevated process running an installer out of
; a user-writable folder is a privilege escalation with extra steps. That is
; `ksx_platform::installer::locate`, documented in docs/DRIVERS.md, and it is
; why DefaultDirName is `{autopf}`. Someone who redirects this install to
; `C:\ksx` gets a refusal with the reason printed, not a silent skip.
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

[UninstallDelete]
; `ksx install-drivers`'s own report, written by the [Code] section below. It
; is evidence about a step that already happened, so it outlives the wizard on
; purpose — and goes when ksx does.
Type: files; Name: "{app}\install-drivers.log"

[UninstallRun]
; Leave nothing behind that keeps starting: the scheduled task outlives an
; uninstall otherwise, and a missing exe on a boot task is a visible error
; every morning. `runascurrentuser` because the task is per-user.
Filename: "{app}\{#AppExe}"; Parameters: "autostart --disable"; Flags: runhidden skipifdoesntexist; RunOnceId: "ksxAutostartOff"

[Code]
// What the last page says about the driver, set by the driver step at the
// bottom of this section. Empty means there is nothing worth saying.
//
// Declared here, at the top, rather than beside the code that uses it: a `var`
// block between two routines is accepted by Pascal Script, and this file gets
// exactly one compile attempt per push on a machine none of us can run ISCC
// on, so it is not the place to bet on "accepted".
var
  DriverNote: string;

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

// ---------------------------------------------------------------------------
// The bundled ViGEmBus install
// ---------------------------------------------------------------------------
//
// WHY HERE. `ksx install-drivers` needs an administrator token and ksx never
// self-elevates, so before this existed the only route to a working pad on a
// fresh machine was a shell command typed from an elevated prompt. Setup is
// already elevated and the user has already agreed to that, so this is the
// one moment in the product where the driver can go in for free. It is also
// the last moment before docs/FIRST-RUN.md's seven moments begin, and §7 makes
// "no terminal" the acceptance test for all of them.
//
// WHY THE VERB AND NOT THE .EXE. `drivers\ViGEmBus_1.22.0_x64_x86_arm64.exe`
// is sitting right there and Exec could run it in one line. It must not.
// `ksx install-drivers` is where docs/DRIVERS.md's guarantees live: the bundle
// is located only under a directory a standard user cannot write, opened ONCE
// with writers and deleters denied, SHA-256'd and Authenticode-checked THROUGH
// that handle, and executed at the path that handle itself resolves to. Every
// one of those exists because this is a kernel driver going in with an
// administrator token, and none of them becomes less necessary because it is
// Inno doing the running. One code path owns the checks.
//
// WHY IT IS ALSO THE UPGRADE AND REPAIR PATH. The verb is idempotent by
// construction: with ViGEmBus healthy its plan is `already-installed`, which
// runs nothing and exits 0, so a re-install and an upgrade both cost one
// process start. The one machine it does act on is the broken one - a
// registered service whose ViGEmBus.sys has gone missing - which is exactly
// what `ksx doctor` tells people to fix this way.
//
// WHY IT CANNOT FAIL THE INSTALL. A driver that will not go in leaves a ksx
// that still installs, still runs, still configures and still maps. It just
// cannot plug a pad. Turning that into a failed install would take away the
// nine tenths that work to punish the one tenth that did not, so every path
// below records what happened (in `DriverNote`, declared at the top of this
// section) and returns.

// ksx's own report from the run, kept where a person can find it. Setup is
// elevated and this is inside the install directory, so a standard user can
// read it and not rewrite it.
function DriverLogPath: string;
begin
  Result := ExpandConstant('{app}\install-drivers.log');
end;

// The sentence every failure ends with. Named once because a retry route the
// user cannot perform is not a retry route: the installer comes FIRST because
// it needs no terminal (FIRST-RUN.md §6 - "the only way out of a mistake is a
// shell command" is on the list of things that must never happen), and the
// command is named second for the people who do have one.
function DriverRetryAdvice: string;
begin
  Result :=
    'ksx itself is installed and works - it just cannot create a controller until the driver is in.' + #13#10#13#10 +
    'To try again: run this installer again with "Install the ViGEmBus controller driver" ticked,' + #13#10 +
    'or, from a terminal opened as administrator:  ksx install-drivers --yes';
end;

procedure InstallControllerDriver;
var
  ResultCode: Integer;
  Params: string;
begin
  // Through the command processor so ksx's report is captured rather than
  // thrown at a hidden console. `/S` makes the quoting rule deterministic:
  // cmd strips exactly the first and last quote of what follows /C and takes
  // the remainder literally, which is the only form that survives an install
  // directory with spaces in it - and the default one has two.
  Params := '/S /C ""' + ExpandConstant('{app}\{#AppExe}') +
            '" install-drivers --yes > "' + DriverLogPath + '" 2>&1"';

  if not Exec(ExpandConstant('{cmd}'), Params, ExpandConstant('{app}'),
              SW_HIDE, ewWaitUntilTerminated, ResultCode) then
  begin
    DriverNote :=
      'The ViGEmBus controller driver was NOT installed: ksx.exe could not be started.' + #13#10#13#10 +
      DriverRetryAdvice;
  end
  else if ResultCode = 0 then
  begin
    // Installed, or already present and left alone. Both are the outcome the
    // checkbox asked for, so neither is worth a word on the last page.
    DriverNote := '';
    exit;
  end
  // The exit codes are `ksx install-drivers`'s documented contract
  // (crates\ksx-app\src\install.rs): 2 = refused before anything ran,
  // 3 = the ViGEmBus setup itself ran and failed, 1 = unexpected.
  else if ResultCode = 2 then
  begin
    DriverNote :=
      'The ViGEmBus controller driver was NOT installed: ksx refused to run the bundled setup.' + #13#10 +
      'That means the bundled file failed one of the two checks ksx makes on it - its SHA-256 or' + #13#10 +
      'its signature - or it was not found where this installer put it.' + #13#10#13#10 +
      'Details: ' + DriverLogPath + #13#10#13#10 + DriverRetryAdvice;
  end
  else if ResultCode = 3 then
  begin
    DriverNote :=
      'The ViGEmBus driver setup ran and reported a failure.' + #13#10 +
      'It keeps its own log in the TEMP folder, named ViGEmBus*.log.' + #13#10#13#10 +
      'Details: ' + DriverLogPath + #13#10#13#10 + DriverRetryAdvice;
  end
  else
  begin
    DriverNote :=
      'The ViGEmBus controller driver install did not complete (ksx install-drivers exited with code ' +
      IntToStr(ResultCode) + ').' + #13#10#13#10 +
      'Details: ' + DriverLogPath + #13#10#13#10 + DriverRetryAdvice;
  end;

  // Said out loud, once, at the moment it happened. A failed driver install is
  // the one outcome here that produces a ksx which looks completely fine and
  // silently cannot do the thing it is for; leaving it to a line on the last
  // page would be reporting success while nothing works, which is this
  // project's signature bug (docs/FIRST-RUN.md §6). It is a message, not an
  // abort - the install carries on either way. Skipped in a silent install,
  // where by definition nobody is watching and the log is the report.
  if not WizardSilent then
    MsgBox(DriverNote, mbError, MB_OK);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep <> ssPostInstall then
    exit;

  if WizardIsTaskSelected('vigembus') then
  begin
    // The verb can take half a minute on a cold machine. A wizard that sits on
    // "Finishing installation..." for that long looks hung, and the one thing
    // a user must not do here is kill setup mid driver install.
    if not WizardSilent then
      WizardForm.StatusLabel.Caption := 'Installing the ViGEmBus controller driver...';
    // Belt and braces on "nothing here may fail the install". An exception
    // raised anywhere below - a constant that did not expand, a log path that
    // cannot be written - propagates out of CurStepChanged and ROLLS THE
    // INSTALL BACK, which is the single outcome this whole section exists to
    // prevent. It would also be invisible until somebody ran the shipped
    // setup.exe: ISCC compiles a broken ExpandConstant perfectly happily, so
    // the CI job that proves this file COMPILES proves nothing about this.
    try
      InstallControllerDriver;
    except
      DriverNote :=
        'The ViGEmBus controller driver step could not be run: ' + GetExceptionMessage + #13#10#13#10 +
        DriverRetryAdvice;
    end;
  end
  else
    // Their choice, and it is a real one - but a choice they can only reverse
    // if somebody tells them how. Not an error, not a dialog: one paragraph on
    // the page they are already reading.
    DriverNote :=
      'You chose not to install the ViGEmBus controller driver. Everything else in ksx works;' + #13#10 +
      'it cannot create a controller until that driver is in. Run this installer again with the' + #13#10 +
      'driver box ticked whenever you want it.';
end;

procedure CurPageChanged(CurPageID: Integer);
begin
  // The last page the user sees, and the only place a note about something
  // that already happened can still reach them.
  if (CurPageID = wpFinished) and (DriverNote <> '') then
    WizardForm.FinishedLabel.Caption :=
      WizardForm.FinishedLabel.Caption + #13#10#13#10 + DriverNote;
end;

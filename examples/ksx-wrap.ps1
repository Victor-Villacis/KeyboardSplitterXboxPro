<#
.SYNOPSIS
    Run an emulator (or any program) with ksx emulation up, and ALWAYS stop ksx
    afterwards.

.DESCRIPTION
    This is the LaunchBox / RetroBat integration point. A frontend launches
    this script instead of the emulator; the script starts ksx, runs the
    emulator, and tears ksx down on every exit path.

    The one thing it must never do is leave ksx running with your keyboards
    captured. Every path out is traced below.

.PARAMETER Emulator
    Full path to the emulator executable.

.PARAMETER EmulatorArgs
    Everything after -- is passed to the emulator verbatim (rom paths, flags).

.PARAMETER Game
    games.toml profile whose slot layout ksx should apply. Optional: without it
    ksx uses the [[slot]] entries in config.toml.

.PARAMETER Ksx
    Path to ksx.exe. Defaults to ksx.exe on PATH.

.PARAMETER TimeoutSeconds
    How long to wait for ksx to arm the pads before giving up (default 20).

.EXAMPLE
    .\ksx-wrap.ps1 -Emulator "C:\RetroBat\emulators\mame\mame.exe" `
                   -Game "MAME 4P" -- sf2ce

.NOTES
    Exit code = the emulator's exit code, except:
      2  ksx refused to start (bad config, missing driver, unknown profile) —
         the emulator was NOT run, because a 4-player cabinet game with no pads
         is worse than a clear error.
      1  this script itself failed.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string] $Emulator,
    [string] $Game,
    [string] $Ksx = 'ksx.exe',
    [int]    $TimeoutSeconds = 20,
    [Parameter(ValueFromRemainingArguments = $true)][string[]] $EmulatorArgs
)

# Stop on unhandled errors, but keep native exit codes ours to interpret.
$ErrorActionPreference = 'Stop'

# ---------------------------------------------------------------------------
# Why this script is shaped the way it is
# ---------------------------------------------------------------------------
#
# `ksx run` blocks the keyboards it is assigned. If this script exits without
# stopping it, the cabinet is left with dead keyboards and a ghost process, and
# the only ways out are an emergency escape or Task Manager. So ksx is stopped
# from a `finally` block, and the `finally` block is reached from every path:
#
#   1. the emulator exits normally .......... try completes -> finally
#   2. the emulator crashes ................. try completes (a non-zero exit is
#                                             not a PowerShell error) -> finally
#   3. the emulator cannot be started ....... throw -> catch -> finally
#   4. ksx never comes up ................... throw -> catch -> finally
#   5. the user closes the console window ... trap/Ctrl+C -> finally
#      (see the note below: a hard console kill cannot run finally at all,
#       which is exactly why ksx also frees keyboards on process death)
#   6. this script has a bug ................ throw -> catch -> finally
#
# Path 5 has a genuine limit: closing a console window with the X button, or a
# `Stop-Process -Force`, terminates PowerShell without running `finally`. That
# is survivable ONLY because ksx is crash-only by design — when the ksx process
# dies, the Interception driver releases its filters with the context handle and
# every keyboard comes back within a second, with no cleanup required. The
# `finally` block is the tidy path, not the safety net; the safety net is that
# killing ksx is always safe.

$script:KsxProcess = $null

function Stop-Ksx {
    <#
      Stop ksx and wait for it to actually be gone. Idempotent: safe to call
      when ksx never started, already exited, or was stopped by an emergency
      escape.
    #>
    if ($null -eq $script:KsxProcess) { return }
    if ($script:KsxProcess.HasExited) {
        Write-Verbose "ksx had already exited (code $($script:KsxProcess.ExitCode))"
        $script:KsxProcess = $null
        return
    }

    Write-Verbose 'stopping ksx...'
    # CloseMainWindow is useless here (ksx is a console app with no window) and
    # Ctrl+C cannot be delivered to another process group. Kill is correct AND
    # safe: ksx needs no cleanup to release keyboards — the driver frees its
    # filters when the process handle closes. This is the documented recovery
    # path (`taskkill /f /im ksx.exe`), not a workaround.
    try { $script:KsxProcess.Kill() } catch { Write-Verbose "ksx already gone: $_" }
    try { $null = $script:KsxProcess.WaitForExit(5000) } catch { }
    if (-not $script:KsxProcess.HasExited) {
        Write-Warning 'ksx did not exit within 5 s. Your keyboards are still fine (blocking is released on process death), but check Task Manager for ksx.exe.'
    }
    $script:KsxProcess = $null
}

# Ctrl+C / console close: PowerShell raises a terminating error, so `finally`
# runs. This trap makes that explicit rather than implicit.
trap {
    Stop-Ksx
    break
}

try {
    if (-not (Test-Path -LiteralPath $Emulator -PathType Leaf)) {
        throw "emulator not found: $Emulator"
    }

    # -----------------------------------------------------------------------
    # 1. Validate the ksx side BEFORE starting anything.
    #    `--dry-run` touches no driver and resolves the whole plan, so a bad
    #    profile is caught here instead of half-way through a session.
    # -----------------------------------------------------------------------
    $dryRunArgs = @('run', '--dry-run')
    if ($Game) { $dryRunArgs += @('--game', $Game) }
    # --no-launch: the frontend is launching the emulator, not ksx. Without it
    # a profile with a `path` would start the game twice.
    if ($Game) { $dryRunArgs += '--no-launch' }

    & $Ksx @dryRunArgs | Out-Null
    if ($LASTEXITCODE -ne 0) {
        # Fail-out uses the console directly, never Write-Error: with
        # $ErrorActionPreference = 'Stop' a Write-Error is itself a terminating
        # error, which unwinds past the `exit` below and loses the exit code we
        # are deliberately choosing. A wrapper whose exit code is a coin flip is
        # worse than no wrapper.
        [Console]::Error.WriteLine("ksx refused the configuration (exit $LASTEXITCODE). Run: $Ksx $($dryRunArgs -join ' ')")
        exit 2
    }

    # -----------------------------------------------------------------------
    # 2. Start ksx and wait until the pads are actually up.
    #    Starting the emulator first would let it enumerate zero controllers
    #    and cache that answer for the whole session.
    # -----------------------------------------------------------------------
    $runArgs = @('run')
    if ($Game) { $runArgs += @('--game', $Game, '--no-launch') }

    # Count the XInput pads that exist BEFORE ksx starts, so the wait below can
    # look for an *increase* rather than for "any pad at all". A cabinet may
    # already have a real controller plugged in, and on the developer machine
    # this baseline is what stops the wait from being satisfied instantly.
    #
    # `Service='xusb22'` is the correct and only filter: ViGEmBus creates a PDO
    # whose hardware id binds Microsoft's inbox xusb22.sys, which is what makes
    # these real XInput slots (docs/research/virtual-gamepad-2026.md). Do NOT
    # widen this to Service='HidUsb' — that is every USB HID device on the
    # machine (keyboards, mice, the I-PAC itself), so the count is never zero
    # and the wait becomes a no-op, which is exactly the race this step exists
    # to prevent.
    $padFilter = "Service='xusb22'"
    $before = @(Get-CimInstance -ClassName Win32_PnPEntity -Filter $padFilter -ErrorAction SilentlyContinue).Count
    Write-Verbose "XInput pads before ksx: $before"

    $script:KsxProcess = Start-Process -FilePath $Ksx -ArgumentList $runArgs `
        -PassThru -WindowStyle Hidden

    # There is no IPC to wait on, so poll the device tree until at least one
    # more XInput pad exists than there was a moment ago.
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    $ready = $false
    while ((Get-Date) -lt $deadline) {
        if ($script:KsxProcess.HasExited) {
            $code = $script:KsxProcess.ExitCode
            $script:KsxProcess = $null
            [Console]::Error.WriteLine("ksx exited immediately (code $code). Run '$Ksx run $($runArgs -join ' ')' by hand to see why.")
            exit 2
        }
        $now = @(Get-CimInstance -ClassName Win32_PnPEntity -Filter $padFilter -ErrorAction SilentlyContinue).Count
        if ($now -gt $before) { $ready = $true; break }
        Start-Sleep -Milliseconds 250
    }
    if (-not $ready) {
        Write-Warning "no new XInput pad appeared within $TimeoutSeconds s; starting the emulator anyway. It may enumerate zero controllers."
    }

    # -----------------------------------------------------------------------
    # 3. Run the emulator, in the foreground, and wait for it.
    # -----------------------------------------------------------------------
    Write-Verbose "starting $Emulator $($EmulatorArgs -join ' ')"
    $emu = Start-Process -FilePath $Emulator -ArgumentList $EmulatorArgs `
        -WorkingDirectory (Split-Path -Parent $Emulator) -PassThru -Wait
    $exitCode = $emu.ExitCode
    Write-Verbose "emulator exited with $exitCode"
    exit $exitCode
}
catch {
    [Console]::Error.WriteLine("ksx-wrap: $_")
    exit 1
}
finally {
    # THE point of this script. Reached on every path above.
    Stop-Ksx
}

# Recovery Runbook

For when input goes wrong on the cabinet. Read this **before** you need it; keep a
copy printed or on your phone — if keyboards are dead you can't Google.

## 0. Always-available lifelines

- **Emergency unblock**: press **Left Ctrl five times** on any captured keyboard →
  ksx toggles keyboard capture off (audio cue). `Ctrl+Alt+Del` always reaches the
  secure desktop and (with ksx running) stops emulation.
- **Kill the app**: `taskkill /f /im ksx.exe` (or legacy `KeyboardSplitter.exe`).
  Both are crash-safe: capture handles close and keystrokes flow again within ~1 s.
- Keep **one spare USB keyboard** that is never assigned/claimed, on a different port.
- A hung (not dead) blocker is the bad case: ksx's watchdog force-releases capture;
  the legacy app does NOT — if the legacy UI ever freezes while blocking, reboot.

## 1. Interception driver dead after a Windows update (the 2026 cliff)

Symptom: legacy app reports driver "not installed", or **all keyboards dead at boot**
(enforcement blocked `keyboard.sys` mid-stack).

If keyboards are dead at boot:

1. Boot into **Safe Mode** (power-cycle ×3 → WinRE → Troubleshoot → Startup Settings).
2. Strip the filter from the keyboard class stack — run in elevated PowerShell:
   ```powershell
   $k = 'HKLM:\SYSTEM\CurrentControlSet\Control\Class\{4d36e96b-e325-11ce-bfc1-08002be10318}'
   (Get-ItemProperty $k).UpperFilters              # expect: keyboard, kbdclass
   Set-ItemProperty $k UpperFilters @('kbdclass')  # remove 'keyboard' (Interception)
   $m = 'HKLM:\SYSTEM\CurrentControlSet\Control\Class\{4d36e96f-e325-11ce-bfc1-08002be10318}'
   Set-ItemProperty $m UpperFilters @('mouclass')  # remove 'mouse' if present
   ```
3. Reboot. Keyboards work; Interception is gone; use ksx's WinUSB backend (M6+) or
   the legacy `legacy/InterceptionUninstall/` tool to clean up properly.

## 2. WinUSB claim went wrong (I-PAC stopped typing, ksx not running)

That's by design — the claimed interface is invisible to Windows. To restore the
normal keyboard driver:

```powershell
pnputil /enum-devices /connected | Select-String -Context 2 'D209'   # find instance ID
pnputil /remove-device "USB\VID_D209&PID_0430&MI_00\..."             # remove binding
pnputil /scan-devices                                                # re-enumerate → kbdhid again
```

Or Device Manager → the device under "Universal Serial Bus devices" → Uninstall
device (leave "delete driver" unchecked) → Action → Scan for hardware changes.

## 3. Virtual pads misbehaving

- Ghost/stuck pads: kill ksx (pads auto-unplug); check Device Manager under
  "Nefarius ViGEm Bus Device".
- Wrong player order: a real Xbox pad plugged in before ksx steals slot 1 — `ksx pads`
  shows each pad's actual XInput user index; replug order or unplug the real pad.
- ViGEmBus health: `pnputil /enum-drivers | Select-String -Context 3 'ViGEm'`;
  reinstall from the bundled signed installer.

## 4. Nuclear option

System Restore point / disk image taken at M0 (do make one). The legacy app +
drivers currently work; an image of that state is the ultimate fallback.

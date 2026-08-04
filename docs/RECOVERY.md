# Recovery Runbook

For when input goes wrong on the cabinet. Read this **before** you need it; keep a
copy printed or on your phone — if keyboards are dead you can't Google.

## 0. Always-available lifelines

- **Emergency unblock**: press **Left Ctrl five times** on any captured keyboard →
  ksx toggles keyboard capture off (audio cue). `Ctrl+Alt+Del` always reaches the
  secure desktop and (with ksx running) stops emulation.
- **Kill the app**: `taskkill /f /im ksx.exe` (or legacy `KeyboardSplitter.exe`).
  Both are crash-safe: capture handles close and keystrokes flow again within ~1 s.
  ⚠️ **This is not true of a WinUSB-claimed device** — killing ksx does not give
  a claimed panel back to Windows, it just stops anything reading it. See §2.
- Keep **one spare USB keyboard** that is never assigned/claimed, on a different port.
  With a WinUSB claim this stops being a nicety and becomes the recovery path;
  `ksx winusb claim` refuses to take the machine's last keyboard for that reason.
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

## 2. WinUSB claim: the panel stopped typing

**First, is this actually broken?** A WinUSB-claimed panel types **only while ksx
is running**. That is the design, not a fault:

| ksx state | what the claimed panel does |
|---|---|
| daemon running, emulation stopped | types normally — ksx re-injects every key with `SendInput` |
| daemon running, between two sessions | types normally: the daemon holds the claim for its whole life, so stopping a game hands the panel straight back |
| emulating | drives the virtual pads (and types nothing — correct) |
| `ksx run` (no daemon), after the run ends | **nothing.** That claim died with the process |
| **not running at all** | **nothing.** No keys, no menu, no way out from that panel |
| any state, at the lock screen / a UAC prompt / Ctrl+Alt+Del | **nothing.** Injected input cannot reach the secure desktop |

So the first question is not "how do I un-claim it" but **"is `ksx daemon`
running?"** Start it (`ksx daemon`) or fix autostart (`ksx autostart --status`)
and the panel comes back with no driver work at all. `ksx run` is not a
substitute here: it claims for one session and releases on the way out, so the
panel types during that run and is dark before and after it.

The rest of this section is for when you want the interface back on the normal
keyboard driver.

### 2a. The exact device tree on this cabinet

Verified read-only 2026-08-04 via `ksx winusb status --json`. **These paths are
port-topology-derived: if a board moves to a different USB port they change.**
Re-run `ksx winusb status --json` and use what it prints; do not trust this table
after a re-cable.

```
USB\VID_D209&PID_0430\4                              USB Composite Device (usbccgp) — never claim this
 ├ USB\VID_D209&PID_0430&MI_00\7&25eea38c&0&0000     ← THE KEYBOARD INTERFACE (claim/release target)
 │   └ HID\VID_D209&PID_0430&MI_00\8&2a0d0500&0&0000   HID Keyboard Device (kbdhid) — what `ksx devices` shows
 ├ USB\VID_D209&PID_0430&MI_01\7&25eea38c&0&0001     system/consumer/mouse collections — leave alone
 └ USB\VID_D209&PID_0430&MI_02\7&25eea38c&0&0002     two vendor collections — leave alone
USB\VID_D209&PID_15A2\6                              Ultimarc trackball — a mouse, never claimed
```

Other keyboards on this machine (the lifelines — `ksx winusb claim` counts these
and refuses if claiming would leave none). It counts only keyboards that can
*type right now*, one per physical board: a keyboard already bound to
`winusb.sys`, one that is disabled or driverless, and a Bluetooth keyboard that
is paired but switched off are all listed by `ksx winusb status` under "not
counted", and none of them will type the release command. Two collections of one
board count once:

```
HID\VID_03F0&PID_034A&MI_00\8&2d11884a&0&0000        HP USB keyboard
HID\VID_046D&PID_C085&MI_01&Col01\7&2518505f&0&0000  Logitech
```

### 2b. The one-command rollback (ksx works, you have any keyboard)

```powershell
ksx winusb status                       # confirm which interface is CLAIMED
ksx winusb release "USB\VID_D209&PID_0430&MI_00\7&25eea38c&0&0000"          # dry run: prints the commands
ksx winusb release "USB\VID_D209&PID_0430&MI_00\7&25eea38c&0&0000" --yes    # do it (elevated prompt)
```

A substring works too if it is unique: `ksx winusb release "PID_0430&MI_00" --yes`.
With two identical I-PACs it will **refuse as ambiguous** rather than guess — use
the full instance path from `status`.

### 2c. By hand, when ksx will not start

Elevated PowerShell. This is exactly what `ksx winusb release --yes` runs.

```powershell
# 1. Drop the devnode (this is the binding).
pnputil /remove-device "USB\VID_D209&PID_0430&MI_00\7&25eea38c&0&0000"

# 2. REQUIRED — find and delete the ksx INF from the driver store.
#    Skipping this is the classic mistake: the ksx INF matches on hardware id,
#    the in-box input.inf only on compatible id, so a rescan re-binds WinUSB
#    straight back and it looks like the removal "did nothing".
pnputil /enum-drivers | Select-String -Context 1,4 'ksx-winusb'
#    -> note the "Published Name: oemNN.inf" for ksx-winusb-vid-d209-pid-0430-mi-00.inf
pnputil /delete-driver oemNN.inf /uninstall /force

# 3. Re-enumerate. HidUsb -> hidclass -> kbdhid binds again.
pnputil /scan-devices

# 4. Confirm.
Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Enum\USB\VID_D209&PID_0430&MI_00\7&25eea38c&0&0000' |
  Select-Object Service      # expect HidUsb, not WinUSB
```

If the device still will not come back: unplug and replug the board, or reboot.
A reboot is always safe here — nothing about the claim survives a `/delete-driver`
except the devnode's cached binding.

### 2d. From Device Manager, with only a mouse

Every step below is clickable. You need no keyboard at all.

1. Right-click **Start** → **Device Manager**.
2. **View → Devices by connection**, then expand down to the I-PAC. (In the
   default *by type* view a WinUSB-claimed interface is under **Universal Serial
   Bus devices**, named from the INF's `DeviceName` — on a ksx claim that reads
   `HID Keyboard Device (ksx WinUSB claim MI_00)`.)
3. Right-click it → **Uninstall device**.
4. **Leave "Attempt to remove the driver for this device" UNCHECKED.** Checking
   it is fine too — it does the `/delete-driver` step for you — but if you leave
   it unchecked you must still remove the INF, or step 5 re-binds WinUSB.
   *Mouse-only shortcut: check the box.* That is the one-click equivalent of 2c
   steps 1 and 2 together.
5. **Action → Scan for hardware changes.**
6. The panel should type again immediately. Verify: the node reappears under
   **Human Interface Devices** as *HID Keyboard Device*.

If you unchecked the box in step 4 and the device came back as WinUSB, repeat
and check it this time.

### 2e. If the panel is your only keyboard

`ksx winusb claim` refuses this case (exit 2, code `last-keyboard`) precisely so
it cannot happen. If you got here anyway — a keyboard died, or the claim was done
by hand with Zadig:

1. **Plug in any USB keyboard.** This is the whole answer; a claimed interface
   does not stop other keyboards working.
2. No spare keyboard? Use **2d** — Device Manager needs only a mouse.
3. No mouse either? Boot **Safe Mode** (power-cycle ×3 → WinRE → Troubleshoot →
   Startup Settings → Safe Mode) and use the on-screen keyboard, or restore the
   system restore point taken before the claim.

### 2f. What a claim never breaks

- The **other interfaces of the same board.** MI_01 (system/consumer/mouse) and
  MI_02 (vendor) keep their normal drivers, so the panel's trackball and any
  volume/shutdown buttons work with or without ksx.
- **Every other keyboard on the machine.** The claim is scoped to one interface.
- **Booting.** `winusb.sys` is in-box, WHQL-signed, and loads regardless of the
  2026 cross-signed policy. There is no equivalent of the "all keyboards dead at
  boot" failure in §1 — that failure needed a *filter* in the class stack, and a
  WinUSB claim has none.

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

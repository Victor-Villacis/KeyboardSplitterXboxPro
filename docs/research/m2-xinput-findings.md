# M2 empirical findings — XInput slot identity on real hardware (2026-08-03)

Live results from this machine (Win11 25H2, ViGEmBus 1.21.442.0) that override the
research assumptions. Tools: `cargo run -p ksx-output --example slot_probe` / `slot_probe2`.

## 1. `get_user_index()` is NOT trustworthy

Research (virtual-gamepad-2026.md §6) claimed `get_user_index()` returns the XInput
`dwUserIndex` — "exactly what you need". Measured reality with 4 pads plugged:

```
user_index = [0, 0, UserIndexOutOfRange, UserIndexOutOfRange]   (stable for 8 s)
actual XInput slots (by active correlation) = [2, 3, none, none]
```

Two pads reporting index 0, values never settling, and the reported index disagreeing
with the slot that actually echoes the pad's input. **Do not use `get_user_index()`
as the slot source of truth.**

## 2. LED notifications are also unreliable

With notification registered between `plugin()` and `wait_ready()`: only pad 0
received any notifications (9× `led_number=2` ⇒ "player 1"), pads 1–3 received zero —
and pad 0's actual slot (by correlation) was 2, not 0. Wrong when present, usually absent.

## 3. Active correlation is ground truth

Press a button on virtual pad k → scan `XInputGetState` slots 0–3 for the change.
Unambiguous, matched physical reality in every probe. **M2 follow-up: implement
`resolve_slots_by_correlation()` in VigemBackend** — at plug/mount time, pulse an
innocuous input (e.g. LT=1) per pad and read back which slot changed; cache the
mapping; re-verify on hotplug events. `ksx pads` and the loopback test switch to it.

## 4. Foreign/leftover virtual pads are a real operational hazard

During verification, 2 `Xbox 360 Controller for Windows` devnodes (children of
**ViGEmBus**, arrival timestamps matching an interrupted test session) survived every
userland cleanup attempt — no owning process visible to Get-Process or Sysinternals
handle64 (consistent with a terminated process pinned by an uncancellable IOCTL).
Only a reboot clears that state. Consequences:

- **`ksx doctor` follow-up: "ghost-pads" check** — enumerate present
  `XnaComposite`/`VID_045E&PID_028E` devnodes, report count + parent bus + arrival
  time, warn when they exist while no known owner runs (XInput slots silently shrink).
- **`ksx pads`/loopback precondition**: count existing virtual-pad devnodes first;
  warn instead of asserting distinct slots 0–3 when the machine isn't clean.
- The plug-timeout path must not leave a helper thread blocked in `wait_ready`
  (current code documents the leak) — prefer plug serialization + generous timeout +
  loud error over detached threads.

## 5. Misc verified

- 4 ViGEm pads plug and appear in XInput within ~5 ms of `wait_ready` (no legacy-style
  5 s settle needed).
- Steam (when running) had plugged nothing here; the ScpVBus unplug IOCTL
  (`0x2AA008`, 16-byte struct) is correctly shaped for future phantom cleanup —
  returns ERROR_NO_SUCH_DEVICE when the serial isn't a ScpVBus child (useful negative test).
- Drop/process-exit cleanly unplugs pads in the normal case (verified repeatedly);
  the orphan case above required the kernel-stuck-IOCTL corner.

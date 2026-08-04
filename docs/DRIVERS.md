# Driver Story & Third-Party Terms

Deep dives: [`research/virtual-gamepad-2026.md`](research/virtual-gamepad-2026.md),
[`research/keyboard-capture-2026.md`](research/keyboard-capture-2026.md).

## Output: ViGEmBus 1.22.0 (committed)

- Attestation/HLK-signed by Nefarius Software Solutions e.U. → **unaffected** by
  Microsoft's 2026 cross-signed-trust removal.
- Project archived Nov 2023 (trademark dispute), driver frozen and stable; still the
  ecosystem default (Sunshine bundles it, DS4Windows/XOutput/JSM ship it).
- Client: vendored [`crates/vigem-client`](../crates/vigem-client) (CasualX, MIT,
  pure-Rust `DeviceIoControl`, includes `get_user_index()` + X360 notification API).
- Installer: bundled at `drivers/ViGEmBus_1.22.0_x64_x86_arm64.exe` (fetched from the
  official GitHub release v1.22.0; signer
  `CN=Nefarius Software Solutions e.U., L=Wels, C=AT`; SHA-256
  `89220A7865076B342892F98865F3499FB7C4CFD673159E89D352C360FD014C6A`). Never download
  at runtime (the old endpoints are rotting).

### ⚠ Expired signing certificate — `ksx install-drivers` currently refuses this bundle

Measured on the dev machine 2026-08-04 with `ksx install-drivers --dry-run --json`:

```
sha256        [OK]   89220A78…014C6A
authenticode  [FAIL] ValidExpiredCert, signer Nefarius Software Solutions e.U.
```

The chain verifies and the signer is right; the **signing certificate's validity
period has passed**. For a correctly timestamped Authenticode signature that is
normal and Windows still accepts it — it is *not* evidence of tampering, and it is
a different thing from the cross-signed kernel-driver problem below (that one is
about `keyboard.sys`'s 2012 cross-cert and the 2026 CI policy; this one is about a
user-mode WiX bootstrapper's leaf cert).

M5 nonetheless refuses it, by explicit design decision: `install-drivers` is the
only ksx command that runs elevated, and "currently-valid certificate" is the bar
for anything that crosses that boundary. The consequence is that **the bundle as
committed cannot be installed by ksx today**. Three ways out, to decide before the
M5 cabinet gate:

1. **Ship a ViGEmBus release whose signature is still in validity** and re-pin both
   the hash and the signer. Cleanest; depends on an archived project publishing one.
2. **Accept `ValidExpiredCert` when a valid RFC-3161 timestamp countersignature
   covers the signing time** — which is what Windows itself does. This is the
   technically correct relaxation; it needs the timestamp to be *verified*, not
   assumed, or the rule means nothing.
3. **Install ViGEmBus by hand** and use `ksx install-drivers` purely as a reporter
   (`ksx doctor` already tells you whether the bus is healthy).

Until one of those lands, the cabinet path is (3). The refusal message names the
expiry date and points here.
- Plan B: HIDMaestro (MIT, user-mode UMDF2; would mean a hand-written Rust client for
  its documented shared-memory protocol — verify its WGI double-input bug first).
  Plan C: libvirtualhid (LizardByte; revisit when Sunshine's PR merges and XInput slot
  behavior is verified). Not options: Nefarius VirtualPad (closed/commercial), vJoy
  (DirectInput-only).

### Where `install-drivers` will look for the bundle

When the process is **elevated**, the search is restricted to directories a
standard user cannot write: `%ProgramFiles%`, `%ProgramFiles(x86)%`,
`%ProgramW6432%`, `%SystemRoot%` and anything beneath them. A candidate outside
those roots is listed as `[SKIP]` with the reason, never silently ignored.

This is a **prefix policy, not a live ACL evaluation** — an administrator who
has loosened the ACL on `C:\Program Files\ksx` gets no warning from us. It fails
closed on the case that matters: a build tree under `C:\Users\…`, a downloads
folder, a USB stick, or a `C:\drivers\` next to a dev build, any of which a
standard user could populate and then have an admin execute. `%ProgramData%` is
deliberately **not** treated as protected: its default ACL lets `Users` create
files.

When the process is **not** elevated the restriction is off, so a development
build still finds the repo's `drivers/` directory — and cannot reach an
executable verdict anyway (`NeedsElevation` comes first), so nothing can be run
from an unprotected path in either case.

The verified file is opened once with `FILE_SHARE_READ` (writers and deleters
denied), hashed and signature-checked through that handle, and the handle is held
across `CreateProcess`, which targets the path `GetFinalPathNameByHandleW`
reports for it. The bytes that were checked are the bytes that run.

## Capture: Interception now → WinUSB later

### Interception (backend: `interception`, M3 — on borrowed time)

- `keyboard.sys`/`mouse.sys` are cross-signed with a cert that **expired 2012-10-21**;
  Windows 11's 2026 servicing rolls out audit-then-enforcement removal of cross-signed
  trust. `ksx doctor` checks the driver signature and the `{784c4414-…}` CI policy
  state and warns loudly.
- 10-keyboard device-ID space; IDs increment on replug/resume and devices die past 10
  until reboot — ksx detects exhaustion and says "reboot required" instead of going deaf.
- **License: dual. Non-commercial use of the distributed binaries via the published
  API only (we dynamically load `interception.dll`); commercial use requires a paid
  license from an author who is unreachable.** This backend is therefore permanently
  non-commercial. Fine for a personal cabinet.

### WinUSB direct claim (backend: `winusb`, M6 — the survival path)

- Rebind the I-PAC keyboard interface (`USB\VID_D209&PID_0430&MI_00`) to in-box,
  Microsoft-signed `winusb.sys`; read HID interrupt reports with `nusb`.
- Blocking and device identity become **structural**: the interface leaves the
  keyboard class stack entirely, Windows never sees a keystroke, and each board is its
  own `nusb::Device`. No third-party kernel driver, no signing cliff, no 10-device limit.
- Leave `MI_01`/`MI_02` (mouse/consumer/vendor collections) bound normally — the
  trackball/spinner keeps working natively in MAME.
- Recovery: `pnputil /remove-device <instance-id>` + rescan restores the standard
  keyboard driver; keep one non-claimed keyboard on a spare port during setup.

### Hardware escape hatch (documented, not built)

I-PAC Multi-Mode firmware 1.5x can present as **2 XInput pads per board** with zero
software. Costs per-key remapping and caps at 2 pads/board — escape hatch, not plan.

## License matrix

| Component | Where | License |
|---|---|---|
| ksx (all new Rust code) | `crates/ksx-*` | MIT OR Apache-2.0 |
| vigem-client (vendored) | `crates/vigem-client` | MIT (LICENSE preserved) |
| ViGEmBus driver + installer | bundled in releases | BSD-3-Clause (redistribution OK) |
| Interception `interception.dll` + drivers | user-installed / bundled installer | LGPL, **non-commercial**, API-boundary only |
| kanata-interception | crates.io dep | LGPL-3.0 (dynamic driver API binding) |
| Legacy C# app | `legacy/` | upstream repo shipped no license; kept as reference, not distributed |

Nothing from `legacy/` (embedded `devcon.exe`, ScpVBus, prebuilt DLLs) is ever carried
into release artifacts.

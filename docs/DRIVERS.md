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
- Plan B: HIDMaestro (MIT, user-mode UMDF2; would mean a hand-written Rust client for
  its documented shared-memory protocol — verify its WGI double-input bug first).
  Plan C: libvirtualhid (LizardByte; revisit when Sunshine's PR merges and XInput slot
  behavior is verified). Not options: Nefarius VirtualPad (closed/commercial), vJoy
  (DirectInput-only).

### Expired signing certificate — accepted, because a verified timestamp covers it

The bundle's signing certificate **expired 2025-02-16**. The chain verifies and
the signer is right; the certificate simply aged out, as code-signing certificates
do — they are issued for a year or two and the binaries they sign outlive them.

M5 originally refused on that alone ("currently-valid certificate" as the bar for
anything running elevated), which made **the committed bundle un-installable by
ksx at all**. That rule is now gone. Since 2026-08-04 the policy is Windows':

> An expired certificate is accepted **if, and only if, a timestamp
> countersignature proves the file was signed while the certificate was still
> valid.** No timestamp, no acceptance.

Measured on the dev machine with `ksx install-drivers --dry-run`:

```
sha256        [OK]   89220A7865076B342892F98865F3499FB7C4CFD673159E89D352C360FD014C6A
authenticode  [OK]   expired-timestamp-verified, signer Nefarius Software Solutions e.U.
certificate          valid 2023-03-13T00:00:00Z .. 2025-02-16T23:59:59Z  (EXPIRED)
timestamp     [OK]   signed 2023-11-02T16:32:03Z, countersigned by DigiCert Timestamp 2023
```

#### Why this, and not one of the other options

The three candidates were: re-pin a newer ViGEmBus release, relax the rule, or
tell people to install by hand.

- **A newer release** would be cleanest and does not exist. ViGEmBus was archived
  in Nov 2023; nobody is going to re-sign 1.22.0.
- **Refusing outright** sounds like the safe default and is not. `ksx doctor` says
  "run `ksx install-drivers`", `install-drivers` refuses its own bundle, and the
  only way forward left is double-clicking the same `.exe` from an admin prompt —
  with **no** hash check, **no** signer check, and no sealed handle. The refusal
  did not prevent that install; it just removed every check from it. A policy that
  routes users around itself is worse than the thing it was guarding against.
- **Verifying the timestamp** keeps both pins live on the path people actually
  take, and matches what the operating system concludes about the same file. The
  cost is one honest relaxation instead of a rule nobody can comply with.

Expiry is also *not* the same problem as the cross-signed kernel driver below:
that one is `keyboard.sys`'s 2012 cross-cert versus the 2026 CI policy, a
kernel-mode trust anchor being withdrawn. This is a user-mode WiX bootstrapper's
leaf cert reaching its ordinary end of life. `ksx doctor` still warns about the
former, loudly.

#### What "verified" means here — four checks, not a shrug

A countersignature is a *claim* ("this file was signed at T"). Trusting it
unexamined would be theatre, so `ksx install-drivers` accepts an expired
certificate only when all four of these hold
(`crates/ksx-platform/src/report.rs`, `TimestampInfo::problem`):

1. the countersigner is a **timestamp** countersigner (`SGNR_TYPE_TIMESTAMP`) and
   not some other unauthenticated attribute;
2. its **own certificate chain verified** — the timestamping authority is one
   Windows trusts (`dwError == 0`);
3. the authority's certificate was **itself valid at the instant it claims to have
   stamped** — a TSA vouching for a moment outside its own life vouches for
   nothing;
4. the stamped instant falls **inside the signing certificate's `NotBefore` ..
   `NotAfter` window** — the actual question being asked.

Any one of them failing is a refusal, and the message names which one. A
certificate whose validity window could not be read at all fails closed: unknown
is not the same as valid.

All of it is read from a **single `WinVerifyTrust` call against the sealed
handle** (`WINTRUST_FILE_INFO.hFile`), through
`CRYPT_PROVIDER_SGNR::pasCounterSigners`. `CryptQueryObject` is deliberately not
used: it accepts only a path or a blob, so reaching for it would mean a second
`open()` and would re-open the time-of-check/time-of-use gap the sealed handle
exists to close. One open, one check, no gap.

#### The states, and their codes

`--json` reports `installer.signature_code`; the same string appears on the
`authenticode` line. The first three are the certificate states, and they stay
three distinct states rather than collapsing into one boolean:

| code | meaning | installable |
|---|---|---|
| `valid` | chain verifies, certificate still inside its window | yes |
| `expired-timestamp-verified` | certificate expired; a timestamp passing all four checks dates the signature inside the window | yes — **this is the committed bundle** |
| `expired-no-valid-timestamp` | certificate expired and no countersignature survives checking (or there is none) | **no** |
| `wrong-signer` | chain verifies, but it is not Nefarius | no |
| `chain-not-trusted` | the chain itself does not verify | no |
| `unsigned` | no Authenticode signature at all | no |

`installer.signature` carries the raw evidence — chain status, both ends of the
certificate window, the timestamp instant, the authority, and each of the four
checks individually — so a script can re-derive the verdict instead of trusting
it. An accepted-but-expired bundle is announced in the human output as a `note:`,
never waved through silently.

Nothing else was relaxed. A bad hash, the wrong signer, an untrusted chain or an
unsigned file are refused exactly as before, and `WinVerifyTrust` returning
`CERT_E_EXPIRED` — Windows' own "no acceptable timestamp" answer — is refused too,
so ksx can never be *more* permissive than the platform whose behaviour it
matches.

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
- **`ksx.exe` does not require `interception.dll` to start.** It is loaded with
  `LoadLibrary`/`GetProcAddress` at the moment the Interception backend is
  constructed, and nowhere else — so `ksx --version`, `ksx devices`, `ksx doctor`,
  `ksx winusb …` and any session whose devices are all `backend = "winusb"` run on a
  machine that has never had the driver. If something does ask for the backend
  without it, the error names the two ways forward instead of the Windows loader
  putting up a "code execution cannot proceed" dialog before `main`. The installer
  therefore does **not** have to ship this DLL. CI pins it: the ksx binary's PE
  import table is asserted to contain no `interception.dll` entry
  (`crates/ksx-app/tests/no_interception_dll.rs`).

### WinUSB direct claim (backend: `winusb`, M6 — the survival path)

- Rebind the I-PAC keyboard interface (`USB\VID_D209&PID_0430&MI_00`) to in-box,
  Microsoft-signed `winusb.sys`; read HID interrupt reports with `nusb`.
- Blocking and device identity become **structural**: the interface leaves the
  keyboard class stack entirely, Windows never sees a keystroke, and each board is its
  own `nusb::Device`. No third-party kernel driver, no signing cliff, no 10-device limit.
- Leave `MI_01`/`MI_02` (mouse/consumer/vendor collections) bound normally — the
  trackball/spinner keeps working natively in MAME.
- Recovery: `ksx winusb release <instance-id> --yes`, or by hand
  `pnputil /remove-device` → `pnputil /delete-driver oemNN.inf /uninstall /force`
  → `pnputil /scan-devices`. **The middle step is not optional**: the ksx INF
  matches on hardware id and outranks the in-box `input.inf` (compatible id
  only), so a rescan with the INF still in the driver store re-binds WinUSB.
  Keep one non-claimed keyboard on a spare port — `ksx winusb claim` refuses to
  take the machine's last one, counting only keyboards that can type *right now*
  (not claimed, not disabled, not paired-but-disconnected) and one per physical
  board. Full runbook: `RECOVERY.md` §2.
- Ownership: the **daemon** claims once at startup and holds it for its whole
  lifetime, handing each session a borrowed view (`ARCHITECTURE.md` §M6). That is
  what keeps a claimed panel typing between games; a per-session claim would give
  the panel back to nobody.

#### The INF is the only signing cost

`winusb.sys` needs nothing: it is in-box and WHQL-signed, so it is unaffected by
the cross-signed-trust removal. But **the INF that points at it is third-party**,
and x64 Windows will not add an unsigned INF to the driver store. ksx generates
the INF deterministically (`ksx winusb claim --dry-run` prints it verbatim) and
prints the signing recipe; it does not and cannot sign it for you.

| Option | Cost | Verdict |
|---|---|---|
| Self-signed catalog (`inf2cat` + `signtool`, cert into Root + TrustedPublisher) | free, WDK needed | **the cabinet answer** — this is what Zadig/libwdi automate |
| [Zadig](https://zadig.akeo.ie/) by hand | free, one GUI click | equivalent; ksx does not care how the interface got bound |
| Attestation signing via Partner Center | EV cert (~$215–499/yr, or Trusted Signing ~$10/mo) + account | the answer if ksx is ever redistributed |
| `bcdedit /set testsigning on` | — | **rejected.** Disables a Secure Boot guarantee machine-wide to install one INF |

The generated INF carries `PnpLockdown = 1`, `Class = USBDevice`, and one
`DeviceInterfaceGUIDs` entry (`{B8B2D1F8-6E0E-4C7F-9E5A-3A9C1D6F2E10}`, ksx's own
device interface class) so `nusb` can find claimed interfaces. It matches **one**
interface's hardware id — never the composite parent, which would claim MI_01 and
MI_02 along with it and take the trackball down.

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
| `winusb.sys` (M6 capture) | in-box, `%SystemRoot%\System32\drivers` | Microsoft, ships with Windows — nothing to redistribute, nothing to license |
| ksx-generated WinUSB INF | written to `%APPDATA%\ksx\winusb` on claim | MIT OR Apache-2.0 (it is ksx output); the catalog signing it is the user's |
| Legacy C# app | `legacy/` | upstream repo shipped no license; kept as reference, not distributed |

Nothing from `legacy/` (embedded `devcon.exe`, ScpVBus, prebuilt DLLs) is ever carried
into release artifacts.

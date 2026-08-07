# Device identity

> Status: design, partially built. `ksx-core::DeviceSelector` implements the
> matching rule below and is fully tested. Nothing in a production path calls
> it yet — see [What is not built](#what-is-not-built).

`crates/ksx-core/src/selector.rs` references this file. This is that file.

## The defect this exists to remove

A `[[device]]` entry holds a raw Windows device instance path:

```toml
[[device]]
id = 'USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000'
alias = 'panel'
backend = 'winusb'
```

Everything after the last `\` is `usbccgp`'s `ParentIdPrefix` plus the interface
number, and `ParentIdPrefix` is derived from **which physical USB socket the
board is plugged into**. Move the board one port over and that string changes.

Nothing errors and nothing warns. The config now names a devnode that does not
exist, the plan finds no candidate, and the panel is dead — on a cabinet, that
reads as "ksx broke", not as "the encoder moved".

Identity that breaks when you replug into a different socket is not identity.
It is a cache key.

There is a second, larger problem hiding behind the first: that string was
typed by hand, and it is *one specific person's I-PAC 4X*. Every path that
teaches a user to paste one — `QUICKSTART.md`, `MIGRATION-WINUSB.md`, the setup
wizard's commit step — teaches them to hand-author a value they have no way to
know. Anyone without that exact board, in that exact socket, has no working
starting point.

## The rule: weakest identity that is still unique

Three rungs. Always prefer the weakest one that uniquely picks out one
connected interface, and escalate only when it does not.

| rung | spelling | survives a replug | tells twins apart |
|---|---|---|---|
| serial | `usb:d209:0430:00:sn=4` | yes | only if the firmware serials differ |
| model | `usb:d209:0430:00` | yes | **no** — refuses while twins are present |
| port | `usb:d209:0430:00:port=7&25EEA38C&0&0000` | **no** | yes, always |

**Model is the default, and it is the 99% case.** One board of a given
VID/PID connected means VID/PID alone names it unambiguously. The port is not
consulted, so the socket stops mattering — which is precisely the fragility
above, deleted.

**Ambiguity is refused, never guessed.** A selector that matches more than one
connected interface returns `Match::Ambiguous` carrying every candidate, and
the caller refuses with all of them listed. Two identical boards staying
tellable apart is the entire reason WinUSB capture beats Interception —
Interception *cannot* do it — so a "simpler" scheme that silently picks one
would be a regression wearing a cleanup costume.

**Port is the honest fallback, written only when it is earned.** When the
model rung is ambiguous and the serials collide too, the port rung is the only
thing left. It is written automatically in exactly that case, and the trade is
stated out loud at the moment it is made: *this board is now pinned to this
socket and will stop working if you move it.* A user who has two identical
encoders needs to know which of their boards that applies to.

## Why serials are a hint, not a promise

Measured on the cabinet, not assumed:

```
USB\VID_D209&PID_0430\4                          <- composite parent, instance "4"
USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000    <- keyboard interface, port-derived
```

The I-PAC 4X reports `iSerialNumber` = `"4"`. The Ultimarc SpinTrak next to it
reports `"6"`. Those are one-character constants that look like model or board
indices, not per-unit serials — so **two I-PAC 4X boards very probably both
answer `"4"`**.

A serial-bearing device would show `USB\VID_D209&PID_0430\<SERIAL>` as its
composite parent instance. This one shows an enumerator counter. So the serial
rung is *verified at match time*, never trusted at write time: `match_against`
compares it against every connected interface, and if two answer, it says so
rather than picking.

## ContainerId groups interfaces into boards

One physical I-PAC exposes three interfaces. They are one device to a human and
three devnodes to Windows:

```
{773D8CD7-…}  shared by MI_00, MI_01, MI_02   <- one physical I-PAC
{4F67EA1D-…}  SpinTrak                        <- a different physical device
```

`ContainerId` is how a picker says **"I-PAC 4X — 3 interfaces, keyboard on
MI_00"** instead of listing three cryptic paths and asking the user to guess.
It plays no part in matching — a selector never resolves via ContainerId — it
is purely how discovery is *presented*.

Note: nothing in the repository reads `ContainerId` today. It is the one new
fact enumeration must learn.

## What a vendor id may and may not decide

The rule the codebase already documents, restated because it is easy to erode:

- **May**: choose a friendly name for display (`[I-PAC]`, `Ultimarc SpinTrak`).
  A lookup table from VID/PID to a human string is fine and good.
- **May not**: gate capture, claiming, refusal, or backend selection. No code
  path may ask "is this an Ultimarc?" to decide *what to do*.

Today three separate copies of Ultimarc's VID exist (`ksx-app/src/devices.rs`,
`ksx-capture/src/winusb/enumerate.rs`, `ksx-platform/src/winusb.rs`), all
feeding display tags and JSON field names. None of them branches logic, so the
rule holds — but the duplication means it can be broken in three places
independently, and one refusal string currently gives I-PAC-specific advice to
every user regardless of hardware.

The `crate::vendors` module those files cite does not exist either. It should:
one table, one place, display-only by construction.

## Legacy spellings keep working

`DeviceSelector::parse` accepts all three forms, so no existing config breaks:

- `usb:…` — the form ksx writes from now on.
- A raw instance path — matched byte-exactly, case-insensitively. Every config
  written before this design existed holds one of these.
- An Interception hardware id (`HID\VID_D209&PID_0430&REV_0056&MI_00`) —
  never matches a USB interface, which is what makes a half-migrated config
  *diagnosable* rather than merely broken.

A legacy entry that still resolves is never silently rewritten. Rewriting a
user's config as a side effect of reading it is how you lose their trust once
and permanently. `ksx device scan` prints the stronger selector it *would*
write and leaves the decision with them.

**Round-trip constraint:** `parse` uppercases legacy paths and canonicalizes
`usb:` spellings, so a config layer that stores the parsed value and serializes
it back would rewrite files on load. The raw string must be preserved verbatim
alongside the parsed form. ksx-config already pins byte-identical round-trip
with tests; this must not be the change that breaks it.

## Selection stays opt-in

Making discovery dynamic is not permission to make claiming automatic. A
WinUSB claim removes a keyboard from the Windows input stack — on a machine
whose only keyboard is that board, an automatic claim is a lockout.

So the two verbs stay separate, and the safe one never implies the dangerous
one:

- `ksx device pick` writes config. It never claims. It prints the claim
  command as an explicit next step.
- `ksx winusb claim` stays dry-run by default, per-device, requires `--yes`
  and elevation, and keeps the existing last-keyboard refusal.

## What is not built

`DeviceSelector` has one production consumer — `UsbCandidate::facts()` — and
two round-trip tests. Everywhere else, identity is still a raw `String`
compared byte-exactly. The gap, smallest-first:

1. **Config stores a selector.** `DeviceEntry.id: String` becomes a
   raw-preserving pair, serialized through a `with`-module in the style of
   `persona_serde` / `socd_serde`, so old files round-trip byte-identically.
2. **One resolution pass.** Between plan-build and every backend consumer,
   resolve each selector against a fresh enumeration exactly once. `Match::One`
   proceeds; `Match::None` refuses naming the board; `Match::Ambiguous` refuses
   listing every hit plus the port-pinned selector that would disambiguate
   each. Interception hardware-id spellings pass through verbatim — the
   byte-exact path M3–M5 depend on.
3. **`ksx device scan`.** A read-only, daemon-free report in the shape
   `ksx devices` already uses: boards grouped by ContainerId, friendly names,
   which interface is claimable, the selector each would get, ambiguity marked,
   and a cross-reference showing which `[[device]]` entries match nothing.
4. **`ksx device pick`.** One writer, three faces (CLI, pipe verb, cabinet
   screen) — the shape `ksx slot assign` established in M9.
5. **Press-to-identify**, for twins. Reuse the existing learn verbs rather than
   inventing a mechanism. Two honest limits, surfaced as refusals rather than
   silence: identify cannot hear a board that is already WinUSB-claimed (it is
   off the input stack — identify *before* claiming, which is the twins
   workflow anyway), and cannot run while a session holds the keyboards.

## "Default device" needs no new concept

`[[device]]` plus `[[slot]].keyboard = 'panel'` already *is* the default — the
alias is the stable name, and the selector is what the alias resolves through.
Nothing new is required to express "always use this one on start".

What changes is only *when* resolution happens: today the config's string is
carried into the plan unresolved and byte-compared against hardware deep in the
pipeline. It should be resolved once, at start, against a fresh enumeration,
so that a board in a new socket still matches — and so that a board that is
genuinely missing is reported by name at the top, instead of surfacing as an
empty candidate list several layers down.

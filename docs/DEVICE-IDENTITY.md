# Device identity

> Status: design, partially built. `ksx-core::DeviceSelector` implements the
> matching rule below and is fully tested. §9 items 1 and 2 are now built —
> `[[device]] id` is a `DeviceRef` (raw + parsed) and `ksx-app/src/run/
> resolve.rs` resolves it once per session inside `plan::resolve_as`. Items 3–5
> (`scan`, `pick`, press-to-identify) are not.
>
> Section numbers are load-bearing: `ksx-capture/src/winusb/enumerate.rs` cites
> §6 and §3 by number. Renumber and those citations start lying.

`crates/ksx-core/src/selector.rs` references this file. This is that file.

## §1 The defect this exists to remove

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

## §2 The rule: weakest identity that is still unique

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

## §3 Why serials are a hint, not a promise

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

## §4 ContainerId groups interfaces into boards

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

## §5 Legacy spellings keep working — and parsing must be lenient

`DeviceSelector::parse` accepts three forms, so no existing config breaks:

- `usb:…` — the form ksx writes from now on.
- A raw instance path — matched byte-exactly, case-insensitively. Every config
  written before this design existed holds one of these.
- An Interception hardware id (`HID\VID_D209&PID_0430&REV_0056&MI_00`) —
  never matches a USB interface, which is what makes a half-migrated config
  *diagnosable* rather than merely broken.

**And it must accept a fourth: anything else containing `\`, as an opaque id.**
This is not a nicety. `parse` currently recognises only `usb:`, `USB\` and
`HID\` prefixes — but ksx's own setup wizard writes whatever Raw Input reports,
and for a laptop or PS/2 keyboard that is an ACPI path:

```
\\.\ACPI#PNP0303#4&1a2b3c4d&0   ->   ACPI\PNP0303\4&1A2B3C4D&0
```

`rawinput.rs` pins that normalisation with a test, and `upsert_device` commits
the result verbatim. So a config the wizard itself wrote, for a perfectly
ordinary keyboard, holds a spelling `parse` rejects. Wire the selector in
strictly and **that config stops loading** — a laptop user's setup breaks on
upgrade, with an error about an unrecognised prefix for an id ksx chose.

The codebase already has the right rule and states it plainly at
`ConfigFile::resolve_device`: *a literal instance path (contains `\`) passes
through unchanged*. `parse` must match that contract — unknown prefix plus a
backslash means an opaque raw id with hardware-id semantics (byte-exact,
case-insensitive, never matches a USB interface). Only a spelling with no
backslash and no known prefix is a parse error.

A legacy entry that still resolves is never silently rewritten. Rewriting a
user's config as a side effect of reading it is how you lose their trust once
and permanently. `ksx device scan` prints the stronger selector it *would*
write and leaves the decision with them.

**Round-trip constraint:** `parse` uppercases legacy paths and canonicalises
`usb:` spellings, so a config layer that stores the parsed value and serialises
it back would rewrite files on load. The raw string must be preserved verbatim
alongside the parsed form. ksx-config already pins byte-identical round-trip
with tests; this must not be the change that breaks it — and the test that
guards it should assert **text** equality, not value equality, or it proves
nothing about the bytes on disk.

## §6 What a vendor id may and may not decide

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

## §7 Selection stays opt-in

Making discovery dynamic is not permission to make claiming automatic. A
WinUSB claim removes a keyboard from the Windows input stack — on a machine
whose only keyboard is that board, an automatic claim is a lockout.

So the two verbs stay separate, and the safe one never implies the dangerous
one:

- `ksx device pick` writes config. It never claims. It prints the claim
  command as an explicit next step.
- `ksx winusb claim` stays dry-run by default, per-device, requires `--yes`
  and elevation, and keeps the existing last-keyboard refusal.

One corollary that is easy to get wrong: `pick` may set `backend = winusb` only
for an interface that is **already** WinUSB-bound. Setting it because the
interface merely *could* be claimed turns a working Interception keyboard into
a config that refuses to start, in one command that looked like a menu choice.
For anything not already bound, `pick` writes the Interception backend and
prints the claim command as the explicit next step.

## §8 Rules for whoever implements this

Four constraints that are not obvious from the type signatures, each of which
would otherwise be discovered as a bug on the cabinet.

**Two spellings resolving to one board is a refusal, not a dedupe.** If two
distinct `[[device]]` entries both resolve `Match::One` to the *same* concrete
interface, that is one physical board silently driving two slots — the exact
two-identical-boards case this design exists to protect. It is tempting to
dedupe the resolved list; don't. Today that situation fails loudly, because the
second WinUSB claim on one interface errors. Keep it loud: refuse, naming both
aliases and the board they collided on.

**A writer must verify uniqueness before it writes.** `strongest_for` can emit
a *port* selector that is still ambiguous: twins that share a devnode rather
than being composite get the identical instance tail (`MI_00`), so the port
rung does not always discriminate. Every path that prints or persists a
selector — `pick`, and `scan`'s upgrade suggestion — must confirm the selector
it chose matches exactly one connected interface, and say so plainly when no
rung can separate two boards.

**Resolution happens once, at the seam both start and reload share.** Hot-swap
eligibility compares `DeviceId`s to decide whether a config edit is a
structural change that must bounce the session. If resolution runs anywhere
downstream of that comparison, every preset edit will spuriously report "slot
N's input device changed" and bounce a live session mid-game. Resolve inside
the factory both paths go through, so what start sees and what reload compares
are the same values.

**The cabinet's alias table is a consumer.** The live Button-Check screen keys
device aliases by the raw config id. Once resolution rewrites ids to concrete
matched paths, a `usb:` spelling in config will no longer match the id arriving
on the live feed, and the screen the operator actually stands in front of shows
unnamed devices. It is display-only, and it must ride the same change-set.

## §9 What is not built

The gap, smallest-first. Items 1 and 2 are **built**; the rest is not:

1. ~~**Config stores a selector.**~~ Built. `DeviceEntry.id` is a
   `ksx_core::DeviceRef` — the raw string as written plus the parsed selector —
   serialized through `ksx_config::device_serde` in the style of
   `persona_serde` / `socd_serde`, so old files round-trip byte-identically.
   A value with no `\` and no known prefix is now a load error rather than a
   literal that silently matches nothing.
2. ~~**One resolution pass.**~~ Built, in `ksx-app/src/run/resolve.rs`, called
   from `plan::resolve_as` — the one call `ksx run`, `ksx daemon`, autostart
   and the tray's "Reload config" share, which is what keeps it upstream of the
   hot-swap comparison in §8. `Match::One` proceeds; `Match::None` refuses
   naming the board; `Match::Ambiguous` refuses listing every hit plus the
   port-pinned selector that would disambiguate each; two entries landing on
   one interface is a refusal naming both aliases. Interception hardware-id
   spellings pass through verbatim — the byte-exact path M3–M5 depend on — and
   a plan that needs none never enumerates at all.
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

## §10 "Default device" needs no new concept

`[[device]]` plus `[[slot]].keyboard = 'panel'` already *is* the default — the
alias is the stable name, and the selector is what the alias resolves through.
Nothing new is required to express "always use this one on start".

What changes is only *when* resolution happens: today the config's string is
carried into the plan unresolved and byte-compared against hardware deep in the
pipeline. It should be resolved once, at start, against a fresh enumeration,
so that a board in a new socket still matches — and so that a board that is
genuinely missing is reported by name at the top, instead of surfacing as an
empty candidate list several layers down.

# Device identity

> Status: design, mostly built. `ksx-core::DeviceSelector` implements the
> matching rule below and is fully tested. §9 items 1–4 are built: `[[device]]
> id` is a `DeviceRef` (raw + parsed); `ksx-app/src/run/resolve.rs` resolves it
> once per session inside `plan::resolve_as`; and `ksx device scan` / `ksx
> device pick` / `ksx device remove` ship as CLI verbs
> (`ksx-app/src/device_scan.rs`, `ksx-app/src/device_edit.rs`, wired in
> `ksx-app/src/main.rs`). `remove` is a fifth verb this document never
> specified — it is listed in §9 now. Only item 5, press-to-identify, is absent.
>
> Section numbers are load-bearing, and by far more than this note used to
> admit. It named one file. It is **fifteen source files and fifty references,
> around thirty of them by section number** (`rg 'DEVICE-IDENTITY\.md' crates/`).
> Renumber and every one of them starts lying — and a citation is often the only
> thing explaining why a refusal is worded the way it is. So
> `crates/ksx-app/tests/docs.rs` asserts that every section number cited from
> code still has a `## §N` heading here, and names the citing files when it
> fails.

`crates/ksx-core/src/selector.rs` references this file. This is that file.

## §1 The defect this exists to remove

A `[[device]]` entry holds a raw Windows device instance path:

```toml
[[device]]
id = 'USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000'
alias = 'panel'
backend = 'winusb'
```

Everything after the last `\` is the instance id Windows generated for that
board, plus the interface number. **When the board reports no USB serial** that
instance id is derived from which physical socket it is in, and moving the board
one port over changes the string.

**This paragraph is hedged on purpose, and the hedge was measured** (§3, on the
cabinet, 2026-08-07). A board that *does* report a serial is keyed off the
serial instead, and its whole devnode chain survives a port move byte-for-byte —
the I-PAC 4X is one of those. An earlier draft of this section said flatly that
the tail "is derived from which physical USB socket the board is plugged into",
three code sites cited §1 as the authority for it, and it is false for the only
board this project has ever been run on. It is corrected here rather than
deleted because the correction is the interesting part: **ksx cannot tell which
kind of path it is holding once the board is absent** — it cannot read the
serial of something that is not plugged in. So a refusal offers both causes and
asserts neither (`ResolveError::Missing`, and the `[WARN]` note beside it).

Either way the entry is a cache key rather than an identity: a value the user
cannot author, cannot verify and does not control, whose stability depends on a
descriptor field they have never seen.

Before `run/resolve.rs` existed, nothing errored and nothing warned when it
stopped matching: the config named a devnode that did not exist, the plan found
no candidate, and the panel was dead — on a cabinet that reads as "ksx broke",
not as "the encoder moved". That half is now fixed. A slot whose board is absent
is dropped with a `[WARN]` that names it, and a plan with no slot left refuses
outright (`ResolveError::Missing`) rather than starting dead. A better error
message is not identity, though, which is what the rest of this document is for.

There is a second, larger problem hiding behind the first: that string was
typed by hand, and it is *one specific person's I-PAC 4X*. Anyone without that
exact board, in that exact socket, had no working starting point.

All three paths that taught it now teach the picker instead. The setup wizard
writes what Raw Input reported, so nobody types anything
(`ksx-app/src/setup.rs`); `QUICKSTART.md` and `MIGRATION-WINUSB.md` show the
`usb:` selector and the verb that produces it. `crates/ksx-app/tests/docs.rs`
holds them to it: a `[[device]]` block containing a literal devnode path has to
name `ksx device pick` on the same page.

## §2 The rule: weakest identity that is still unique

Three rungs. Always prefer the weakest one that uniquely picks out one
connected interface, and escalate only when it does not.

| rung | spelling | survives a replug | tells twins apart |
|---|---|---|---|
| serial | `usb:d209:0430:00:sn=4` | yes | only if the firmware serials differ |
| model | `usb:d209:0430:00` | yes | **no** — refuses while twins are present |
| port | `usb:d209:0430:00:port=7&25EEA38C&0&0000` | **assume no** | yes, always |

The port row says *assume* no because that column is a promise ksx makes at
write time, not a fact about the socket. §3 measured a board whose instance tail
did not change across a port move, so a `port=` selector for a serial-reporting
board may well survive one. ksx must not rely on that: it chooses the rung
before it knows whether the user will ever move the board, and a selector that
survives by luck is not a selector that survives. `survives_replug()` therefore
answers by selector **shape**, and every surface that prints the trade says what
it costs without asserting what will happen.

**Model is the default, and it is the 99% case.** One board of a given
VID/PID connected means VID/PID alone names it unambiguously. The port is not
consulted, so the socket stops mattering — which is precisely the fragility
above, deleted.

**Ambiguity is refused, never guessed.** A selector that matches more than one
connected interface returns `Match::Ambiguous` carrying every candidate, and
**every** caller refuses with all of them listed. "Every" is load-bearing and
was once false: `resolve.rs::classify_backends` used to fall through an
ambiguous match with a bare `_ => continue`, on the reasoning that a refusal had
already been taken upstream. It had not — an entry a slot names by a *different*
spelling is skipped by the loop upstream, so the fall-through left `plan.winusb`
empty and the session fell back to Interception on a WinUSB-claimed board: a
dead panel reporting success. Pinned by
`an_ambiguous_winusb_entry_is_refused_even_when_a_slot_spells_the_board_longhand`.

Two identical boards staying tellable apart is the entire reason WinUSB capture
beats Interception — Interception *cannot* do it — so a "simpler" scheme that
silently picks one would be a regression wearing a cleanup costume.

**But identification is not claiming, and the claim path is worse off.** While
two boards of one model are plugged in, `ksx-platform/src/winusb.rs` refuses
`Refusal::SharedHardwareId` for *both* of them: an INF binds by hardware id, and
both boards carry the same one, so installing the driver for one would claim
every one of them. The port rung tells twins apart in config; the claim then
declines to act on it until the user unplugs the sibling. Telling identical
boards apart *during a claim* needs per-device installation, which ksx does not
do. Do not read the paragraph above as "twins work" — read it as "twins are
never silently confused".

**Port is the honest fallback, written only when it is earned.** When the
model rung is ambiguous and the serials collide too, the port rung is the only
thing left. It is written automatically in exactly that case, and the trade is
stated out loud at the moment it is made: *this board is now pinned to this
socket and will stop working if you move it.* A user who has two identical
encoders needs to know which of their boards that applies to.

## §3 Why serials are a hint, not a promise

Measured on the cabinet, not assumed:

```
USB\VID_D209&PID_0430\4                          <- composite parent, serial "4"
USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000    <- keyboard interface
```

The I-PAC 4X reports `iSerialNumber` = `"4"`. The Ultimarc SpinTrak next to it
reports `"6"`. Those are one-character constants that look like model or board
indices, not per-unit serials — so **two I-PAC 4X boards very probably both
answer `"4"`**.

### The instance path is NOT always port-derived — measured 2026-08-07

An earlier draft of this section called the parent instance "an enumerator
counter" and the interface path "port-derived". **Both were wrong**, and the
correction matters because a diagnostic was built on them.

Windows keys a USB devnode off the **serial number when the device reports one**,
and off the socket only when it does not. A bare instance with no `&`
(`…PID_0430\4`) is the serial form; the `6&1a2b3c&0&4` shape is the generated
one. The I-PAC 4X reports a serial, so its whole devnode chain is anchored to it.

Tested directly on the cabinet by moving the board between root ports:

| | interface instance | `DEVPKEY_Device_LocationPaths` |
|---|---|---|
| before | `7&25EEA38C&0&0000` | `USBROOT(0)#USB(5)#USB(4)` |
| after | `7&25EEA38C&0&0000` | `USBROOT(0)#USB(7)#USB(4)` |

The board demonstrably moved; the id did not change by one character.

Two consequences:

1. **A refusal must not claim the board moved.** When an entry matches nothing
   the device is *absent*, so ksx cannot read its serial and cannot know which
   kind of path it holds. `ResolveError::Missing` therefore offers both causes
   and asserts neither — asserting "you moved it" sends someone hunting a port
   problem they do not have. `survives_replug()` answers by selector *shape*,
   which is still the right input for choosing a `usb:` spelling, but it is
   **not** evidence that an instance path is fragile.

   This applies to **every** string the user reads, not only the one that is
   easiest to test. The first fix corrected `ResolveError::Missing`'s `Display`
   and left the `[WARN]` note thirty lines below it still saying "this id names
   one specific USB socket, so moving the board to another port breaks it" — and
   the note is what a user actually sees, because a dropped slot is the common
   case and a total refusal is the rare one. Both are now pinned, the note by
   `a_dropped_slots_warning_offers_both_causes_and_asserts_neither`.
2. **The twin-board risk gets sharper, not softer.** Two I-PAC 4X boards both
   reporting `"4"` collide in Windows' own instance namespace, before ksx sees
   anything — a stronger failure than the ambiguity §1 describes. Untested until
   a second board exists; test it before relying on either board's id.

So the serial rung is *verified at match time*, never trusted at write time:
`match_against` compares it against every connected interface, and if two
answer, it says so rather than picking.

## §4 ContainerId groups interfaces into boards

One physical I-PAC exposes three interfaces. They are one device to a human and
three devnodes to Windows:

```
{773D8CD7-…}  shared by MI_00, MI_01, MI_02   <- one physical I-PAC
{4F67EA1D-…}  SpinTrak                        <- a different physical device
```

Grouping interfaces into boards is how a picker says **"I-PAC 4X — 3
interfaces, keyboard on MI_00"** instead of listing three cryptic paths and
asking the user to guess. Whatever key does it plays no part in matching — a
selector never resolves via the group — it is purely how discovery is
*presented*.

**The picker shipped without `ContainerId`, and that is the interesting part.**
This section named `ContainerId` "the one new fact enumeration must learn", and
enumeration never learned it: nothing in the repository reads it today.
`ksx device scan` groups on the **composite parent instance path**
(`UsbCandidate::parent_id`, surfaced as `UsbRow::board`), which the enumerator
already had for free and which the three interfaces of one I-PAC share. Tested
by `three_devnodes_of_one_board_are_one_entry`.

So the stated prerequisite was not one. `ContainerId` is still the more correct
key in principle — it groups devices that do not share a composite parent, such
as a keyboard and its dongle-mate — and it stays here as the answer if the
parent-path grouping is ever found wanting. It is no longer described as
required work.

## §5 Legacy spellings keep working — and parsing must be lenient

`DeviceSelector::parse` accepts three forms, so no existing config breaks:

- `usb:…` — the form ksx writes from now on.
- A raw instance path — matched byte-exactly, case-insensitively. Every config
  written before this design existed holds one of these.
- An Interception hardware id (`HID\VID_D209&PID_0430&REV_0056&MI_00`) —
  never matches a USB interface, which is what makes a half-migrated config
  *diagnosable* rather than merely broken.

**And it accepts a fourth: anything else containing `\`, as an opaque id.**
Built — `DeviceSelector::parse` returns `HardwareId` (opaque, byte-exact,
never matches a USB interface) for any spelling with a backslash and no known
prefix. The argument for it, kept because it is the reason the leniency must not
be tidied away later: ksx's own setup wizard writes whatever Raw Input reports,
and for a laptop or PS/2 keyboard that is an ACPI path:

```
\\.\ACPI#PNP0303#4&1a2b3c4d&0   ->   ACPI\PNP0303\4&1A2B3C4D&0
```

`rawinput.rs` pins that normalisation with a test, and `upsert_device` commits
the result byte-for-byte. So a config the wizard itself wrote, for a perfectly
ordinary keyboard, would hold a spelling a strict `parse` rejects — and **that
config would stop loading**, a laptop user's setup breaking on upgrade with an
error about an unrecognised prefix for an id ksx chose.

(`upsert_device` is no longer the *unconditional* verbatim write this argument
was built on: it is fallible now, and refuses rather than committing an id that
would not load back. The bytes it does commit are still exactly what it was
handed. The leniency above is what makes those two statements compatible.)

The codebase already has the right rule and states it plainly at
`ConfigFile::resolve_device`: *a literal instance path (contains `\`) passes
through unchanged*. `parse` must match that contract — unknown prefix plus a
backslash means an opaque raw id with hardware-id semantics (byte-exact,
case-insensitive, never matches a USB interface). Only a spelling with no
backslash and no known prefix is a parse error.

A legacy entry that still resolves is never silently rewritten. Rewriting a
user's config as a side effect of reading it is how you lose their trust once
and permanently. `run/resolve.rs` therefore never touches the store; the only
writer is the explicit `ksx device pick`.

`ksx device scan` prints the stronger selector it *would* write and leaves the
decision with them. This half was a promise for a while and is now a fact: the
report prints an `id = 'usb:…'` line per board, `ksx devices` prints the same
spelling per interface, and both are pinned by tests. It matters more than it
looks, because two user-facing strings already told people this was happening —
`DeviceSelector::explain` for a legacy path, and `ResolveError::Missing`, which
says `ksx devices` "prints … the `usb:` selector that names the board either
way". Neither did.

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

**Built.** `ksx-core/src/vendors.rs` is the one table — one place, display-only
by construction: every public function returns a string type and none returns a
`bool`, so there is nothing for a branch to ask. `ksx-capture` and `ksx-platform`
re-export `ULTIMARC_VID` from it rather than each declaring their own, and
`ksx-app/src/devices.rs` holds no copy at all. The SpinTrak-is-not-an-I-PAC
regression this section was written after is a test in that module.

The one refusal string that gave I-PAC-specific advice to every user regardless
of hardware is fixed too: `Refusal::NotAKeyboard`'s advice is generic and
unconditional, with the Ultimarc paragraph *appended* only when the instance id
carries Ultimarc's VID.

That append is the closest thing left to a vendor branch, and §6's literal
wording ("may not gate … refusal") is strained by it, so the boundary is drawn
explicitly: **a vendor id may add a sentence to a refusal that has already been
issued; it may not decide whether to issue one, nor which code it carries.**
`the_ultimarc_hint_only_adds_a_paragraph_to_an_already_issued_refusal` pins
exactly that — same `code()`, same first paragraph, one extra paragraph — so the
branch cannot grow into a gate without a test going red.

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

The gap, smallest-first. Items 1–4 are **built**; only item 5 is not:

1. ~~**Config stores a selector.**~~ Built. `DeviceEntry.id` is a
   `ksx_core::DeviceRef` — the raw string as written plus the parsed selector —
   serialized through `ksx_config::device_serde` in the style of
   `persona_serde` / `socd_serde`, so old files round-trip byte-identically.
   A value with no `\` and no known prefix is now a load error rather than a
   literal that silently matches nothing.
2. ~~**One resolution pass.**~~ Built, in `ksx-app/src/run/resolve.rs`, called
   from `plan::resolve_as` — the one call `ksx run`, `ksx daemon`, autostart
   and the tray's "Reload config" share, which is what keeps it upstream of the
   hot-swap comparison in §8. `Match::One` proceeds; `Match::Ambiguous` refuses
   listing every hit plus the port-pinned selector that would disambiguate each;
   two entries landing on one interface is a refusal naming both aliases.
   Interception hardware-id spellings pass through verbatim — the byte-exact
   path M3–M5 depend on — and a plan that needs none never enumerates at all.

   **`Match::None` degrades rather than refusing, deliberately, and this item
   used to say otherwise.** A blanket refusal means one loose cable behind an
   eight-player cabinet blanks the machine for everybody, and before this pass
   existed an absent device took down only what needed it — so refusing would
   have been a regression wearing a safety check's clothes. `drop_missing`
   therefore drops the affected slot, nulls a missing mouse, names both in
   `[WARN]` notes, and refuses (`ResolveError::Missing`) only when no slot
   survives, because *then* a "successful" start is a dead panel with no error
   anywhere. `run/resolve.rs`'s own module header carries the same correction.
3. ~~**`ksx device scan`.**~~ Built (`ksx-app/src/device_scan.rs`): a read-only,
   daemon-free report — boards grouped, friendly names, which interface is
   claimable, and the selector each would get. Two of the six things this item
   asked for are **still missing**, and both are real: grouping is by composite
   parent instance path rather than `ContainerId` (§4 — the substitute works, so
   this is a note, not a defect), and there is **no cross-reference showing
   which `[[device]]` entries match nothing**. The nearest thing is
   `ksx devices`' `unmatched_winusb_config` warning, which covers only
   WinUSB-backed entries; an orphaned Interception entry is still reported
   nowhere. Doing it properly means `DevicesView` carrying the configured table,
   not just the alias that happened to land on a live row.
4. ~~**`ksx device pick`.**~~ Built as a CLI verb, correctly factored — a pure
   `plan_pick` plus an `apply_pick` — so the other faces are wiring, not
   rewriting. But it is **one writer, one face**: there is no pipe verb and no
   cabinet screen. `ksx device remove` is in the same position. That is task #22
   and `docs/SURFACES.md` §3 row 3, where the matrix has been corrected to say
   so rather than implying a screen exists.
5. **Press-to-identify**, for twins. Reuse the existing learn verbs rather than
   inventing a mechanism. Two honest limits, surfaced as refusals rather than
   silence: identify cannot hear a board that is already WinUSB-claimed (it is
   off the input stack — identify *before* claiming, which is the twins
   workflow anyway), and cannot run while a session holds the keyboards.

## §10 "Default device" needs no new concept

`[[device]]` plus `[[slot]].keyboard = 'panel'` already *is* the default — the
alias is the stable name, and the selector is what the alias resolves through.
Nothing new is required to express "always use this one on start".

What changed is only *when* resolution happens. The config's string used to be
carried into the plan unresolved and byte-compared against hardware deep in the
pipeline; it is now resolved once, at start, against a fresh enumeration
(`run/plan.rs` calls `resolve::apply` at the top, and everything downstream sees
concrete ids only). So a board in a new socket still matches, and a board that
is genuinely missing is reported by name at the top instead of surfacing as an
empty candidate list several layers down.

## §11 Transport decides which backends can reach a device

A device's **transport** — how it is attached — is a separate fact from its
identity, and it decides something identity does not: which capture backends
can ever reach it. Two devices with equally good ids can have completely
different answers.

The case that matters today is Bluetooth, and both halves of it surprise
people:

- **A Bluetooth keyboard CAN be captured by Interception.** Interception is a
  class filter on the Windows keyboard stack, and a paired Bluetooth keyboard
  is a keyboard-class devnode on that stack exactly like a USB one. Splitting
  it into virtual pads works today, with no new code.
- **A Bluetooth keyboard can NEVER be WinUSB-claimed.** A claim is an INF that
  binds a **USB interface** by hardware id (`USB\VID_xxxx&PID_yyyy&MI_zz`).
  There is no USB interface on a Bluetooth device for such an INF to match.

That second half is a property of the transport, not a feature ksx has not
written yet, and the wording matters: "not supported" invites someone to wait
for a release that cannot come. Every surface says *why* — `ksx_core::transport`
holds the sentence and `Reach::eligibility` composes the per-row line, so the
CLI, Studio's Rust seam and Studio's TypeScript island cannot word it three
ways. This is the same rule as §6 in a different costume: the fact may pick a
sentence and gate a *backend*, and it may not be re-derived at a surface.

Two consequences fall out, and both are refusals rather than silence:

1. `ksx winusb claim` on a Bluetooth keyboard refuses with
   `transport-cannot-claim`, **not** `not-a-keyboard`. The two send a user to
   opposite places — "not a keyboard" means *pick a different interface*, and
   this means *the interface is right and the backend is wrong*.
2. `backend = "winusb"` on a `[[device]]` entry that resolves to a Bluetooth
   device is a permanently broken entry, not one awaiting a claim. `ksx devices`
   and Studio call it out separately from "no such interface is present",
   because the fix is to edit the entry rather than to plug something in.

### Identity on a transport with no `usb:` selector

A `usb:` selector names a vendor, product and interface number (§2). A
Bluetooth device has none of the three, so `ksx device pick` writes the
keyboard devnode's **instance path**, byte-exact — `DeviceSelector::HardwareId`
semantics, the same lenient legacy spelling §5 keeps working. That id has no
rung to climb: it is already the weakest thing that names the device, and there
is no port to pin.

### The trap: present is not typing

A **paired but disconnected** Bluetooth keyboard is PRESENT in the device tree
all day — pairing is what puts it there, and the batteries have nothing to do
with it (`CM_PROB_DEVICE_NOT_CONNECTED`). It stays listed, because hiding it
would be its own lie, and every row says it cannot deliver a keystroke right
now. It is **excluded from the last-keyboard arithmetic**
(`Survey::keyboard_count`): otherwise someone reads "2 keyboards", claims their
panel, and is locked out by a keyboard in a drawer with dead batteries.

# Input transforms — chords, multi-bind, macros, and everything they imply

Victor's design thinking, 2026-08-05, extrapolated. He described three
shapes and asked what he's missing. One of them already works, one is a
genuine architectural gap with a nasty tradeoff, and the third needs a
subsystem we don't have. The rest of this doc is the map.

## 0. The frame that makes all of it make sense

**ksx does not send input events. It publishes STATE.** Every submit is a
complete snapshot of a pad: 16 button bits, 2 trigger bytes, 4 axis words.
The game never sees "a keystroke"; it polls (or is handed) the current
shape of a controller.

Two consequences run through everything below:

1. **Anything simultaneous is free.** A diagonal is not a mapping — it is
   two bits set at once. Up+Left = both dpad bits set = the game reads
   northwest. Same for the stick: `lx.min` + `ly.max` held together is a
   vector, not a rule we wrote. This is why Victor's "we don't set it but
   the game sees it" works: **combination is the natural state of a set.**
2. **Anything sequential must survive sampling.** A game polling at 60 Hz
   sees state every ~16.7 ms. A macro step held for 5 ms is invisible —
   not unreliable, *invisible*. Any timed feature (macros, turbo) must
   hold each step long enough to be sampled at least twice (~33 ms), or it
   is a lie. This single fact constrains every macro design below.

## 1. The three shapes, named

| Shape | Example | Status |
|---|---|---|
| **Multi-bind** (one physical → many virtual, simultaneous) | P → A + B + RT | **WORKS TODAY** |
| **Chord** (many physical → one virtual, simultaneous) | A + B → RT | **SHIPPED** (§1b) — `when`/`unless` guard, with consumption |
| **Macro** (one physical → a timed SEQUENCE) | P → ↓, ↘, →, A (hadouken) | **SHIPPED** (§1c) — engine-thread scheduler, one timer list |

### 1a. Multi-bind already works — try it now

`ksx-core/src/preset.rs` states it outright: *"No uniqueness constraint in
either direction: many keys → one function and one key → many functions are
both native (legacy needed the `<custom>` category for this; here it is
just more entries)."* The engine compiles each key to a `SmallVec` of
targets and applies them all.

In preset TOML, bind several functions to the same key:

```toml
A   = "P"
B   = "P"
rt  = "P"
```

Press P → A, B and RT all go down together; release → all up together.
That is Victor's "opposite" case, working today, no code needed. The gap
was only that the MAPPER had no way to express it (it assigned one key per
control and would overwrite). **Mapper work item — SHIPPED (2026-08-06)**:
a control accepts a key already used by another control of the same preset
without treating it as a conflict — the write goes through untouched, every
co-binder keeps the key, and both the response (`also_drives`) and the
legend ("also A · B") show the fan-out. `ksx map --move-from FUNCTION` is
the explicit, singular way to take a key away instead; `--force` no longer
moves anything (docs/CONTROL-SURFACE.md "Multi-bind").

### 1b. Chords — SHIPPED (2026-08-06)

He identified the real problem before writing a line: *if A→A and B→B, and
the game's move is A+B, a chord A+B→RT means the game never sees A+B.*
Correct, and it generalizes: **a chord must consume its constituents, or
it double-fires.** There were only three honest options:

- **Consume** — A+B produces RT and nothing else. The game loses A+B.
- **Pass through too** — the game sees A, B *and* RT. Almost always wrong.
- **Defer** — on A, wait N ms to see whether B arrives. If it does → RT;
  if not → send A *late*. Correct, and it **charges every A press N ms of
  latency**. This is the tap-hold tax (QMK/kanata live with it); on a
  fighting cabinet where a 16 ms frame decides a match, it is a real cost.

**ksx consumes, and never defers.** That is the whole design decision, and
everything below follows from it.

#### The model

A binding gains a GUARD. It is not a new binding *kind* — a chord is "this
binding, but only while these other keys are (not) also down" — so it
composes with buttons, triggers, axes and dpad identically
(`ksx-core/src/preset.rs::Chord`):

```rust
pub struct Chord {
    pub key: Key,          // the trigger
    pub binding: Binding,  // any binding kind
    pub when: Vec<Key>,    // ALL must be held
    pub unless: Vec<Key>,  // NONE may be held   (MAME's NOT — §2.7, free)
}
```

Guarded rows live in `Preset::chords`, unguarded ones stay in
`Preset::entries` **exactly as before**. That is not cosmetic: it is what
makes "no chords ⇒ nothing changed" checkable rather than claimed — the M3
replay corpus still hashes to the same `SESSION_DIGEST`, the legacy
importer is untouched, and every pre-chord preset file is byte-identical.

#### The file

```toml
[bindings]
A  = "G"                                              # unchanged
rt = { key = "D", when = ["F"] }                      # D+F -> RT
lb = { key = "D", when = ["F", "C"], unless = ["LeftShift"] }
lt = ["Q", { key = "A", when = ["B"] }]               # plain AND chord
```

A guard with nothing in it (`{ key = "G" }`) is normalized to a plain
binding — a zero-key "chord" would consume its own trigger and silently
disable that key's other bindings.

#### The semantics, exactly

- **Activation is state, not sequence.** A chord is a SET of held keys
  (§0.1): press order does not matter, and there is no window to miss.
- **Consumption.** While a chord is active, its constituents (trigger +
  every `when` key) are SUPPRESSED: their own unguarded entries drive
  nothing. `unless` keys are a negative condition and are never consumed.
- **One batch, always.** Activation releases whatever a consumed
  constituent was holding *in the same delta batch* that presses the
  chord's output — no stranded button, no intermediate state on the wire
  (the neutral-delta discipline `Engine::swap_tables` established).
  Release is the mirror: the chord's output goes and every constituent
  still held resumes its own binding in that one batch, so lifting B while
  A stays down gives you A back with no flicker.
- **A chord is a holder.** It participates in the all-keys-up rule and the
  opposite-axis snap like any key, so an endpoint driven by both a key and
  a chord stays down while either drives it.
- **Specificity.** A bigger guard beats a smaller one *where they share a
  constituent*: A+B+C suppresses A+B, and A+B comes back the instant C
  lifts. Disjoint chords never interfere. Chords with the SAME guard are a
  multi-bind (one chord, several outputs — native in ksx) and both fire.
  Two guards of the SAME size on the same trigger that could be satisfied
  together are a **config error**, reported by validation and refused at
  session start — never a coin flip on build order.
- **Everything releases on the way out**: unplug, session stop, hot-swap
  and `reset` all clear chord state and emit the releases.

#### The honest caveat

**There is no deferral and no timing window.** So if a chord key is *also*
bound on its own, the game sees that individual output for the moment
between the first and the second keypress. A+B→RT with A→X shows X, then
X-off + RT-on. That is a real, visible flash, and it is the price of never
charging a single press one millisecond of latency.

Therefore, in order:

1. **Prefer dedicated chord keys.** If the constituents are not bound
   individually there is no flash and no cost at all — the chord is a
   plain AND. On an arcade panel with spare buttons this is nearly always
   available, and it is what the docs, the CLI help and the validator all
   recommend.
2. If a constituent *is* individually bound, ksx allows it and **says so
   every time**: `ksx map` reports a `flash` advisory naming the key and
   what it flashes, validation emits `ChordConstituentAlsoBound`, and the
   plan prints it as a `[WARN]` (advisory, not a refusal — the config
   works exactly as written). Never silently.
3. Physical reality, unchanged: a human hitting two arcade buttons
   "together" lands them 10–30 ms apart. With no window that is not a
   correctness problem, only the flash above. Many fighting games also
   ship 2-button macro assignments in-game, which remains a better place
   to solve it when available.

#### The hot path

Guard evaluation is O(guard size) bit tests per event, allocation-free:

- guard keys are interned into the same dense-id space as everything else,
  so a guard is `bit(down, id)` — no key lookup, no preset scan;
- chords are precompiled per slot, sorted most-specific-first, so one
  forward pass resolves specificity;
- `held` / `consumed` / `blocked` / `scan` are sized in
  `EngineTables::build` (off the hot path, like the whole table set) and
  reused per event;
- **a slot with no chords never touches any of it** — the extra state is
  not even allocated, and the dispatch loop takes the pre-chord branch.
  `tests/engine_chords_alloc.rs` pins zero allocation on the chord path;
  `tests/engine_alloc.rs` and the replay corpus pin the chord-free one.

### 1c. Macros — SHIPPED (2026-08-05; `repeat` and the autorepeat fix 2026-08-06)

Hadouken is ↓, ↘, → + punch **over time**. That is not a set, it is a
timeline, and it needed three things the engine did not have: a clock, a
queue, and a policy for what happens when reality interrupts. All three
landed; what did not is listed at the end of this section.

#### The model

A macro is a named list of steps, and a step is **a set of bindings to hold
plus a duration** (`ksx-core/src/macros.rs`). Because combination is the
natural state of a set (§0.1), the diagonal ↘ is one step holding two
bindings — not two events, not a special case.

```rust
pub struct MacroStep { hold: Vec<Binding>, duration: StepDuration, allow_short: bool }
pub enum   StepDuration { Ms(u32), Frames(u32) }
pub struct Macro { name, steps, on_release, retrigger, interrupt, repeat, turbo }
pub enum   Repeat { Once, WhileHeld, Turbo }
pub enum   TurboRate { Hz(u32), GapMs(u32) }
pub struct MacroTrigger { key: Key, index: u16 }   // key -> macro
```

Macros live in `Preset::macros`, beside `entries` and `chords` and never
inside them — so `preset.macros.is_empty()` is a *checkable* "this preset
predates macros", the M3 replay corpus still hashes to the same
`SESSION_DIGEST`, and a macro-free preset file is byte-identical.

#### The file

```toml
[macros.hadouken]
steps = [
  { hold = ["dpad.down"],              ms = 50 },
  { hold = ["dpad.down","dpad.right"], ms = 50 },
  { hold = ["dpad.right"],             ms = 50 },
  { hold = ["A"],                      ms = 50 },
]

[bindings]
macro.hadouken = "P"       # any number of keys; the usual multi-bind rules
```

`macro.<name>` is its own grammar, deliberately outside `parse_function`: a
macro is named by the preset's own `[macros]` table, not by the fixed pad
vocabulary, so the name cannot resolve to a `Binding` without knowing which
preset it came from. An empty `hold = []` is legal and useful — it is a
deliberate neutral gap, which is how a macro says "let go, then press
again" so the game sees two presses instead of one long hold.

#### `frames` — an ergonomic unit, and only that

`{ hold = ["dpad.right"], frames = 3 }` is accepted and converted once at
60 Hz (rounded to nearest: 1→17 ms, 2→33, 3→50, 4→67 — rounded *once*, so
three frames is 50 ms and not 3×17). Exactly one of `ms` / `frames` per
step; both, or neither, is refused rather than resolved.

**It buys readability and nothing else.** ksx publishes STATE and the game
samples it on its own schedule (§0), at a rate ksx does not know and a
phase that drifts against ksx's clock every second the two run. `frames =
3` means "held for the wall-clock duration of three 60 Hz frames", NOT "the
game will read this on exactly three of its polls" — it may read it on two,
or four, and on a 120 Hz or vsync-coupled emulator on some other number
entirely. The stronger promise would need the game to tell us when it
polls, which no game does. What the unit *does* inherit is the floor below:
two frames is `MIN_STEP_MS`, and that is not a coincidence.

#### The sampling rule, enforced (§0.2)

A step shorter than ~33 ms is not unreliable at 60 Hz, it is **invisible**.
So `MacroStep::effective_ms` **raises** anything below `MIN_STEP_MS = 33` —
and validation says so every time (`MacroStepRaised`). The per-step opt-out
is `allow_short = true`, which runs the duration as written and warns
differently (`MacroStepMayBeMissed`). Both are advisories: one keeps the
macro correct at the cost of running longer, the other is the author having
been asked and having answered. **Neither is ever silent**, and there is no
configuration in which ksx emits a step a poller cannot see.

#### Scheduling: absolute, drift-free, one list

The scheduler runs on the **engine thread** — the capture thread still only
timestamps and forwards, so four players triggering macros at once cannot
lock or allocate anywhere near the hot path. There is exactly **one ordered
timer list for every macro in every slot** (`engine.rs::Timers`): entries
are `Copy`, the backing `Vec` is sized at `EngineTables::build` time to the
total macro count, arming is an insertion into an already-sorted list, and
ties are FIFO so two macros armed for the same millisecond always fire in
the order they were armed. No thread per macro, no allocation per step.

Deadlines are **absolute offsets from the macro's start**, never `now +
duration` accumulated per step. That is the real fix for jitter: a wake
that is 3 ms late is corrected at the very next step instead of pushing the
whole sequence back, so four 50 ms steps still end at 50/100/150/200 rather
than accumulating four scheduler jitters.

When a wake is *so* late that a step's whole window has already passed, the
step is **not skipped** — a skipped step is an input the game never
sampled, which §0.2 forbids. It is published for its sampling minimum
(`MacroStep::min_visible_ms`) and the rest of the timeline slides. A macro
may run long; no step is ever invisible.

The engine exposes `tick(now) -> Deltas` and `next_deadline() -> Option<u64>`;
the supervisor's engine loop already woke on input and on a poll timeout, so
a macro is simply one more reason to wake (`select!`'s `default(idle)` is
clamped to the next deadline). Time is **supplied, never read** — the engine
holds no clock — which is what makes a macro a pure function of
`(events, clock)`, reproducible to the millisecond in CI with a fake one.

Windows' default timer resolution is ~15.6 ms and a step is 33 ms, so the
engine thread raises it to 1 ms **only while a deadline is armed**
(`supervisor::TimerResolution`) and restores it the moment the last one
clears — with `Drop` covering every exit path, panic included. `ksx daemon`
lives for hours; holding 1 ms for all of it to serve a macro pressed twice
an hour would tax every other process's power management for nothing.

#### Interruption policy — three axes, all explicit

| Setting | Values | Default | Means |
|---|---|---|---|
| `on_release` | `finish` \| `abort` | `finish` | letting go of the trigger mid-run |
| `retrigger` | `ignore` \| `restart` | `ignore` | pressing the trigger again mid-run |
| `interrupt` | `none` \| `any-input` \| `opposing` | `none` | doing something *else* mid-run |

`finish` is the fighting-game expectation: you tap the button and the
quarter-circle comes out whole. `abort` is the hold-to-autofire shape.
`ignore` is the default because `restart` stutters a sequence back to step
0 on any switch bounce a real panel has.

`interrupt` composes with `on_release`, and is deliberately narrow:

- **`any-input`** — any other key *this slot* binds, going down, aborts.
- **`opposing`** — abort only on input that contradicts the macro, which is
  exactly two rules: (1) a key driving a direction **opposing** one the
  current step is holding (`ksx_core::socd::opposes` — the same relation
  SOCD cleans, so the two features cannot disagree), and (2) a key that
  triggers a **different** macro on this slot. A punch during a motion is
  neither, and passes straight through.

A macro is never interrupted by its own trigger; that is a retrigger, and
`retrigger` decides it. One press can abort one macro and start another,
and both land in the single delta batch that event produces.

#### A press is an EDGE, never an autorepeat (fixed 2026-08-06)

Windows repeats a held key ~30 times a second, and every repeat arrives as
another key-**down** for a key that is already down. For the key SET —
buttons, axes, chords — that is harmless and idempotent, which is why
nothing needed to know. For an edge-triggered feature it is not: every
repeat used to re-arm a finished macro, so *holding* the trigger played the
sequence over and over. On a cabinet that reads as "if I hold it, it never
stops… it's acting like a turbo."

`Engine::handle_at` now computes whether the event actually moved the key
and only runs the macro trigger and interrupt paths on a genuine edge. The
default is therefore exactly **one run per press**, and repetition is
something you ask for by name (below). Everything else — the pad state, the
all-keys-up rule, the delta diffing — is unchanged, because a repeated
key-down never changed any of it anyway.

#### Repeating — `once` / `while-held` / `turbo` (SHIPPED 2026-08-06)

| Setting | Values | Default | Means |
|---|---|---|---|
| `repeat` | `once` \| `while-held` \| `turbo` | `once` | what the END of a run does while the trigger is still down |
| `turbo_hz` | integer | — | full cycles (run + gap) per second |
| `gap_ms` | integer | — | the released window between runs, directly |

- **`once`** — one run per press. Holding the trigger changes nothing. This
  is the default and the fighting-game expectation: a special move must not
  become a machine gun because a switch stuck.
- **`while-held`** — re-run from step 0 the instant the last step ends, for
  as long as the trigger is down, **finishing the current run** either way.
  No gap: the two runs are one continuous motion, so an endpoint the last
  step and the first step share never blinks.
- **`turbo`** — the same with a deliberate neutral **gap** between runs, so
  the game samples a released frame and reads two presses instead of one
  long hold. The gap is published state for a real duration, like any step.

`turbo_hz` and `gap_ms` are two spellings of one number and giving both is
refused, exactly like `ms`/`frames` on a step; a `repeat = "turbo"` with
**neither** is refused too, because an auto-fire whose rate ksx picked is an
auto-fire nobody asked for.

**The rate has a ceiling, and it is arithmetic, not policy.** One turbo
cycle is a press *and* a release, so at a 60 Hz poll the best case is one
sample pressed and one released — `TURBO_MAX_HZ = 30`. Above it the rate is
**clamped, not refused** (refusing a number that is merely optimistic would
be unkind), and validation states both figures. The honest ceiling is
usually lower: ksx's own step floor makes the shortest visible press 33 ms
and the shortest visible gap another 33, so a one-step macro tops out near
15 Hz. `turbo_hz = 30` on a 50 ms macro really runs at about 12 Hz, and
`Macro::effective_turbo_hz()` is the number the preset actually gets —
computed once in ksx-core so the engine, validation and any plan printer
cannot drift apart.

**Precedence, when every policy has an opinion.** They answer four different
questions and are evaluated in the order the events happen:

1. `interrupt` — other input arrived. An abort is an **exit**: the run
   stops, everything releases, and *no repeat follows it*.
2. `on_release` — the trigger came up. `abort` stops now (and therefore
   never repeats); `finish` lets the run in flight complete.
3. `repeat` — the last step just ended. `once` stops; `while-held` and
   `turbo` re-run **only if the trigger is still down at that instant**.
   That is why `finish` + `while-held` means "let go and it stops after this
   run" rather than a contradiction.
4. `retrigger` — a NEW press arrived mid-run. Unchanged by any of the above,
   and unrelated to repeat: a repeat is not a press and never counts as one.

Everything releases on stop, from a turbo **gap** too — a macro resting
between runs holds nothing but is still armed, so `cancel_macros`, a device
yank and a hot swap all disarm it.

```toml
[macros.autofire]
repeat = "turbo"
turbo_hz = 10          # or: gap_ms = 50
steps = [{ hold = ["A"], frames = 2 }]
```

#### Everything releases on the way out

A macro STEP is an ordinary **holder**: `holder_bindings[first + i]` is step
`i`'s hold set, and the step is "held" exactly while the macro is on it. So
the all-keys-up rule, the opposite-axis snap, the releases-before-presses
order and the one-batch discipline are the chord machinery unchanged — a
macro cannot strand a button that a chord could not. Two consequences worth
naming: an endpoint carried from one step to the next **never flickers**
(step 0's ↓ is not released, because step 1 already holds it in the same
pass), and an endpoint driven by both a key and a macro stays down while
either drives it.

Every exit uses the same cancel-and-release path, and each has a test:

| Path | Mechanism |
|---|---|
| `on_release = "abort"`, `interrupt` | `macro_cancel` → one neutral batch |
| device yank | `release_device` cancels the slot's macros, then full resync |
| hot swap | `swap_tables` drops the run with its tables; neutral deltas follow |
| session stop | engine thread runs `cancel_macros` before it finishes |
| escape gesture (`LeftCtrl ×5` → blocking off) | supervisor sends `EngineCtl::CancelMacros` |
| `reset` | clears macro state and every armed deadline |
| process death | unchanged: the pads vanish and the driver releases everything |

#### Validation

`EmptyMacro`, `UnknownMacroHold`, `UnknownMacroRef` (a trigger naming a
macro the preset does not define), `GuardedMacroTrigger`,
`DuplicateMacroName` (names match ignoring case, so two tables differing
only in case would silently shadow), `MacroStepBadDuration` (both units or
neither), `MacroTurboBadRate` (both rate units, or a turbo with none) — all
faults. `MacroStepRaised`, `MacroStepMayBeMissed`, `TurboRateClamped`,
`TurboGapRaised`, `TurboRateWithoutTurbo` and `MacroHoldsOtherMechanism`
are the advisories, printed by the plan as `[WARN]`.

##### `MacroHoldsOtherMechanism` — "the diagonal never comes out"

A pad has three ways to say *right*: the dpad, the left stick, the right
stick. A game reads whichever one it was written for, and **ksx publishes
exactly what the step says** — so a step holding `dpad.right` on a preset
whose stick is `lx`/`ly` is faithfully published and read by nobody.

The symptom is nastier than "the macro does nothing", because it is usually
*one* step that was written that way — the diagonal, copied out of an
example that used the dpad — into a motion whose other steps use the
preset's own functions. The game then shows ↓ and →, and never ↘: a motion
with a hole in it, which looks exactly like an engine bug and is not one.
The engine was tested against this directly (`engine_macros.rs`: the axis
diagonal is one stable published state for the step's whole duration, a
60 Hz sampler sees it, SOCD never touches a perpendicular pair, and a step
may even hold a dpad bit *and* an axis at once).

So validation compares each step's directional holds against the mechanisms
the preset's own **bound** direction keys drive, and names the mismatch,
the step and both mechanisms. It stays quiet when it has nothing to say: a
preset that drives both, a hold that is a button or trigger (no mechanism),
an unbound placeholder row, or a preset with no direction keys at all.

#### CLI surface — triggers only, on purpose

`ksx map --preset "SF P1" --function macro.hadouken --key P` binds the key
that STARTS a macro, and `--clear` unbinds it; cross-slot conflicts,
`--force` and the `also_drives` multi-bind report all work exactly as they
do for a pad function. `--when`/`--unless` and `--move-from` are refused
with a reason.

**Authoring the sequence itself stays TOML-only**, and that is a decision,
not a gap: a step list is a timeline with durations, a hold set and three
interruption policies, and a flag-per-field CLI for it would be worse than
the `[macros]` table it would write. `ksx run --dry-run` prints every
configured macro — step count, total ms, all three policies, and the keys
that start it — in both the human and `--json` output, so the AI/CLI
surface can still *read* everything. A mapper UI for macros is a later pass.

#### What did not ship

- **No macro in the Studio mapper UI** — CLI + TOML only for now.
- **No chord that starts a macro.** `macro.x = { key = "P", when = ["Q"] }`
  is refused rather than half-implemented; the guard would have to compose
  with consumption, and nothing asked for it yet.
- ~~**No looping / hold-to-repeat.**~~ Shipped 2026-08-06 as `repeat`
  (above). The aliasing problem it was deferred for is not solved by
  avoiding it — it is `TURBO_MAX_HZ`, the effective-rate report, and the
  sampling floor on the gap, all stated out loud.
- **No `interrupt = "opposing"` beyond the two stated rules.** Anything
  fuzzier would be unpredictable on a cabinet, so it is direction
  opposition plus other-macro-triggers, and this document says so.
- **Fairness caveat, stated once**: macros are a first-class arcade
  tradition (real cabinets wire one button to several micro-switches), but
  online play and some anti-cheat treat sequence automation differently.
  ksx ships them without apology for local/cabinet use.

## 2. What Victor is missing — the catalog, ranked for a cabinet

Ordered by value on *this* machine, not by novelty.

1. **Layers / shift (hold P1-Start + button → admin).** The single highest
   value transform for an arcade cab, and the one every emulator already
   half-implements (RetroArch's "hotkey enable", MAME's UI keys). One
   modifier key turns 30 panel buttons into 60. PadForge's vocabulary is
   the right menu: Hold / Toggle / Latch / Cycle / Sticky.
2. **Key output, not just pad output** (roadmap E3). A cabinet needs
   Escape-to-exit, F1 menus, coin insert, save-state, volume. Today ksx can
   only produce pad state, so admin actions have no home. This is arguably
   more urgent than macros: it is what makes the panel *self-sufficient*.
3. **Turbo / autofire.** Shmups and NES-era games expect it. Hold → repeat
   at N Hz, or toggle-turbo. Bounded by §0.2: above ~30 Hz it aliases into
   nonsense at 60 Hz polling; the UI should cap and explain, not offer 100.
4. **Tap vs hold (dual-role keys).** Tap = A, hold = LT. Doubles a small
   panel. Carries the same latency tax as chords — same honesty rule.
5. **Digital → analog shaping.** An arcade stick is 8-way digital; many 3D
   games want *walk* vs *run*. Emit partial axis magnitude, optionally
   ramping over time (PadForge's "Ramp"), per-binding. Also the inverse:
   **4-way restriction** for games that break on diagonals (Pac-Man), and
   diagonal deadzone shaping.
6. ~~**SOCD policy, user-visible.**~~ **SHIPPED** — see §2.6 below. It cost
   *one* new primitive (a chord that outputs nothing) because
   chord-with-consumption already was the mechanism. `last-wins` /
   "snap tap" is the one mode still missing, and the reason is stated there.
7. ~~**NOT / exclusion conditions.**~~ **SHIPPED with chords** (§1b): the
   `when` guard made `unless` fall out free, exactly as predicted. MAME's
   `NOT`, in the same row as the binding it qualifies.
8. **Toggle-hold (sticky hold).** Press once → held until pressed again.
   Accessibility, and useful for triggers/auto-run.
9. **Double-tap / multi-tap activators** (Steam's model). Cheap once the
   clock exists for tap-hold.
10. **Negative edge / release-triggered bindings.** Fighting-game charge
    partitioning and "on release" actions. Trivial once the transform
    layer exists; strange without it.
11. **Cross-slot chords.** P1-Start + P2-Start = admin/exit. Needs the
    condition to reach across slot boundaries — a deliberate exception to
    slot isolation, worth it for cabinet ergonomics.
12. **Trackball / spinner → axis.** His trackball is deliberately left
    native for MAME today; a spinner (Arkanoid/Tempest) or trackball →
    right stick is what makes non-MAME games playable on a cab.
13. **Per-game auto-switching** of transforms, not just slots — the
    games.toml profile already exists; transforms should live at that
    layer too (a fighting profile with macros, a shmup profile with turbo).
14. **Input display / recording.** Training-mode input history on the
    Studio page: what the panel sent, what the pad published, side by
    side. Doubles as the debugging tool for everything in this document
    and reuses the replay-corpus machinery from M3.

### 2.6. SOCD cleaning — SHIPPED

*Simultaneous Opposing Cardinal Directions*: a stick can only be left OR
right, a panel can hold both, and what the pad then reports is a policy.
Tournaments legislate it — Capcom-style rules regulate simultaneous
opposing input, and **neutral** and **up-priority** are the compliant
behaviors — so it has to be stated, configurable, and the same for the
dpad and the stick.

**The insight (Victor's): chord-with-consumption IS the SOCD mechanism.**
A chord already suppresses its constituents; SOCD is only ever "swallow
one or both of two keys". So no engine rule was added. We were one
primitive short, and that primitive is *a chord that emits nothing*:

```rust
Binding::Consume        // output nothing; the value is the suppression
Chord::consuming(key, when)
```

- **neutral** = `[Left+Right] → Consume`. Both keys suppressed, nothing
  pressed in their place, so the axis falls to centre (via the existing
  opposite-axis snap, which sees no held opposite) and both dpad bits
  clear. Same for `[Up+Down]`.
- **up-priority** = `[Down+Up] → whatever UP drove`. Consumption is
  all-or-nothing per chord, so "keep Up" is said as "consume both and
  re-emit Up" — and re-emit it *in full*, every binding that key had, or
  its other outputs would vanish with it. Down is swallowed; Up survives.
  Horizontal still cancels: the rule is asymmetric on purpose (down-back →
  up-back must be a jump).

#### Configuration — generated, never hand-written

Per slot, in `config.toml` and `games.toml`:

```toml
[[slot]]
number = 1
preset = "street-fighter-p1"
socd = "up-priority"      # "off" (default) | "neutral" | "up-priority"
```

`ksx_core::socd::generate` reads the preset and emits the chords at plan
time (`run/plan.rs`), for **both** the dpad pair and the stick axes (lx/ly
and rx/ry), covering multi-bind by generating one chord per key pair that
can actually produce the opposition — `n × m` for `n` left keys and `m`
right keys, which on a real panel is 1×1. Generation is idempotent, and
`socd = "off"` generates *nothing*: the M3 replay corpus digest does not
move, and no config file gains a byte.

**A hand-written chord over a pair wins.** Generation skips any pair the
preset already chords by hand, and validation says so
(`SocdShadowedByChord`, advisory — the config works exactly as written).
An unguarded `consume = "Left"` row is inert and reported too
(`ConsumeWithoutGuard`): consumption is what a *chord* does.

#### The one mode ksx cannot do yet: last-wins / "snap tap"

Last-wins needs to know which direction was pressed **most recently** —
that is input *history*, and the engine is deliberately a pure function of
the currently-held key SET (§0.1), which is exactly what makes chords
free of clocks, deferral and latency. Adding an ordering memory is the
transform stage's job (§3), not a new binding shape, so it waits for it.
Note that some tournament rulesets restrict last-wins anyway; the two
modes that shipped are the ones those rules ask for.

Everything above is one of two additions to a currently-stateless mapping:

- **Time** (chords, tap-hold, macros, turbo, double-tap, ramps)
- **Context** (layers, sticky, NOT-conditions, toggles)

So: insert a **transform stage** between capture and pad state — a
per-slot deterministic state machine that consumes `(key, down, timestamp)`
and emits pad-state deltas, possibly *later* than the input that caused
them. Non-negotiable properties:

- **Hot path stays pure**: the capture thread still only timestamps and
  forwards. The transform machine runs on the engine thread, where
  allocation already doesn't happen per event; timers are a single
  ordered wheel, not a thread per macro.
- **Everything releases on the way out.** Session stop, escape gesture,
  hot-swap, crash — every exit path must neutralize pending timers and
  emit releases. The FIX 3 swap already proved the shape (neutral deltas
  for anything held); macros extend it.
- **Deterministic and replayable**: transforms must be a pure function of
  (events + timestamps), so the M3 replay corpus can test every one of
  them in CI with no hardware. This is what keeps a feature this big
  honest.
- **Config stays hand-editable TOML** and every transform is expressible
  in `ksx map`-style verbs, so the AI/CLI surface keeps parity with the
  GUI (CONTROL-SURFACE rule).

## 4. Sequencing (proposed)

Nothing here blocks M6/M7. Suggested order, cheapest-and-most-useful first:

1. **Mapper support for multi-bind** — zero engine work, it already runs;
   just stop treating a shared key as a conflict and show it honestly.
2. **E3 key output** — unlocks admin/exit/coin, the cabinet's real gap.
3. **Layers** — the biggest ergonomic win per line of code.
4. ~~**Chords**~~ — **DONE** (§1b), and done *without* the transform stage:
   consumption needs context, not time, so it landed as a guard on a
   binding with no clock, no deferral and no latency. What is left for the
   transform stage is the genuinely time-based half: turbo, tap-hold,
   double-tap, ramps — plus analog shaping, which needs neither. **SOCD
   cleaning also landed on top of chords** (§2.6), for the cost of one
   consume-only binding; only its last-wins mode still waits for history.
5. ~~**Macros**~~ — **DONE** (§1c). They did need the scheduler, and it
   turned out to be small: one ordered timer list on the engine thread, and
   a macro *step* modelled as an ordinary holder, so every release path
   chords already had covered macros for free. What the transform stage
   still owes §3 is the rest of the time-based half — turbo, tap-hold,
   double-tap, ramps — plus SOCD's last-wins mode, which needs history
   rather than a clock.
6. **Input display** alongside whichever of the above ships first; it is
   how the user (and we) will debug all of it.

## 5. Sequencing after the current batch (Victor's directives, 2026-08-06)

His four review points and four enhancements, folded in — with one
correction that changes what we promise.

### The frames correction (important, say it out loud)

Victor's jitter concern is real: wall-clock `ms` steps drift with the OS
scheduler. But `frames = N` **cannot** make a macro frame-exact from the
game's point of view, because we never learn the game's polling phase —
it samples on its own clock, unsynchronized with ours, and drifting. So:

- `frames = N` ships as an **ergonomic unit** (N × 16.667 ms) because
  fighting-game users think in frames. It is not a guarantee.
- The actual fix for jitter is **absolute-deadline scheduling**: every
  step's end is computed from the macro's start instant, never
  accumulated per step, so a late wake cannot compound across a 4-step
  macro. Combined with the §0.2 sampling minimum (≥2 poll intervals per
  step), that is as deterministic as an out-of-process mapper can be.
- Anyone needing true frame-exactness needs to be inside the game's
  frame loop; that is not a thing ksx can be, and pretending otherwise
  would be the dishonest kind of feature.

### Accepted as specified

- **Macro interruption beyond `on_release`**: `interrupt = "none" |
  "any-input" | "opposing"`. Aborts release everything in the same batch.
- **Many-key display cap**: the on-art tag shows the primary key plus a
  `+N` indicator; the legend carries the full list. (Already the design
  in the current build — his instinct matched it.)
- **JSON in / commented TOML out**: exactly the shipping design. The AI
  emits JSON (no malformed array-of-tables), the daemon validates and
  applies, disk keeps annotated TOML for humans and for the next session
  to read.
- **4-player concurrency**: one timer structure on the ENGINE thread for
  every slot; the capture thread never locks or allocates for a macro.

### The four enhancements, ranked and sequenced

1. **Turbo / auto-fire** — rides the macro scheduler, so it is nearly
   free once macros land. Capped and explained at the 60 Hz sampling
   ceiling (≈30 Hz on/off is the practical maximum; above that it
   aliases into dropped or phantom inputs).
2. **Layers + key output (E3)** — still the biggest cabinet win per line
   of code: hold P1-Start and the panel becomes an admin layer (save
   state, load state, volume, exit) emitting KEYSTROKES, which ksx
   cannot produce at all today. Layers without key output only get you
   half the value.
3. **Live input debugger — "the truth stream"** — Victor's sharpest
   framing: chords, SOCD and macros are *widening the gap* between what
   was pressed and what the game sees, and nothing on screen closes it.
   Split view: raw I-PAC feed on the left, published pad state on the
   right, timestamped. This is the only way to tell a hardware polling
   drop from a transform-stage logic bug. Cheaper than it looks — the
   virtual half is readable in the browser via the Gamepad API (§2026
   layer, item 1); only the physical half needs the live socket.
4. **Visual SOCD intervention** — amber D-pad highlight while the engine
   is actively scrubbing an illegal input, rather than rendering it as
   merely unpressed. Falls out of #3's state stream almost free, and it
   is the difference between "the engine did something" and "the engine
   did nothing".

## 6. What TAS tooling teaches us (Victor, 2026-08-06)

TAS (tool-assisted speedrun) tools — BizHawk/TAStudio, FCEUX, libTAS —
solved input timing problems adjacent to ours, and the differences are
as instructive as the similarities.

### The boundary, stated once and for all

TAS movie files are **frame-indexed** (`|..U..A.|`, one line per frame)
because the emulator *is* the clock: it advances a frame, then reads
input, deterministically. **TAS gets frame exactness by owning the frame
loop.** That is precisely what an out-of-process mapper can never do.

So: **inside the emulator, frames; outside it, absolute-deadline
milliseconds plus margin.** Every timing promise ksx makes lives on the
outside of that line, and `frames = N` is a unit for authoring, not a
guarantee of sampling (§5).

### Adopted

1. **The truth stream gets HISTORY, not just current state** (upgrades
   Enhancement A/§5.3). Every TAS tool ships a scrolling per-frame input
   display, because the interesting failures are transient — a step that
   was dropped, a chord that flashed, a SOCD scrub. Current-state-only
   would miss exactly the bugs the debugger exists to catch. Two columns,
   timestamped, scrolling: what the panel sent, what the pad published —
   which is TAS's own "pressed vs consumed" split.
2. **The piano roll is the right macro editor.** TAStudio shows frames as
   rows and controls as columns and you paint into cells. That beats a
   form with "add step" buttons badly, and it maps directly onto our
   model: rows = steps, columns = the slot's controls, cells = held or
   not. When a macro editor lands in Studio, this is its shape. Bonus: the
   same grid visualizes turbo and shows SOCD interventions in time.
3. **Surface lateness — with precision about whose.** TAS communities
   learned that inputs on lag frames vanish, and their fix was to make lag
   VISIBLE. Ours: when a macro step's scheduled window elapses, report how
   late OUR wake was (deadline vs actual). Be exact about the limit — we
   cannot know whether the game sampled a step, only whether we published
   it on time. Claiming otherwise would be inventing knowledge we don't
   have; reporting our own lateness is honest and actionable.
4. **Record both sides of the stream.** TAS re-verifies a whole run from a
   movie file. Our M3 replay corpus records the capture stream; extend it
   to record the PUBLISHED PAD STATES alongside, so a session replays as a
   true two-sided regression: same inputs must produce the same outputs
   through a rebuilt engine. Cheap — the recorder exists, this adds the
   second column — and it is the only automated way to keep chords, SOCD,
   macros and turbo honest as they compose.

### Deliberately not adopted

Anything depending on frame-advance, savestates or rerecording. Real
hardware, real games and a real OS scheduler give us a world that is not
pausable, rewindable or deterministic. Those parts of the TAS toolkit are
a useful illustration of the limits, not a design to copy.

### Policy note for M7 preset sharing

TAS is explicitly "not human play", and the fighting-game world draws a
hard line at macros compressing inputs a human could not perform. Local
cabinet play makes this a non-issue today. But when preset sharing ships
(M7), **macros are the presets people will argue about**: a shared
"Street Fighter P1" that silently contains a one-button super is a
different artifact from a button layout. Cheap now, expensive to
retrofit: mark presets that contain macros/turbo at share time so an
importer knows what they are getting, and let a cabinet owner refuse them
wholesale.

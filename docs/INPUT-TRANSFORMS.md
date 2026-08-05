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
| **Chord** (many physical → one virtual, simultaneous) | A + B → RT | **Not expressible** — needs an AND condition |
| **Macro** (one physical → a timed SEQUENCE) | P → ↓, ↘, →, A (hadouken) | **Not expressible** — needs a scheduler |

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
is only that the MAPPER UI has no way to express it (it assigns one key
per control and would overwrite). **Mapper work item**: allow a control to
accept a key already used by another control in the same preset without
treating it as a conflict to resolve — it is a multi-bind, and the legend
should show it as one ("P → A · B · RT").

### 1b. Chords — and the tradeoff Victor spotted

He identified the real problem before writing a line: *if A→A and B→B, and
the game's move is A+B, a chord A+B→RT means the game never sees A+B.*
Correct, and it generalizes: **a chord must consume its constituents, or
it double-fires.** There are only three honest options:

- **Consume** — A+B produces RT and nothing else. The game loses A+B.
- **Pass through too** — the game sees A, B *and* RT. Almost always wrong.
- **Defer** — on A, wait N ms to see whether B arrives. If it does → RT;
  if not → send A *late*. Correct, and it **charges every A press N ms of
  latency**. This is the tap-hold tax (QMK/kanata live with it); on a
  fighting cabinet where a 16 ms frame decides a match, it is a real cost.

**The recommendation, in order:**
1. **Prefer dedicated chord keys.** If the constituents are not bound
   individually, there is no ambiguity and no latency — the chord is just
   a two-key AND with zero cost. On an arcade panel with spare buttons,
   this is nearly always available.
2. If a constituent *is* individually bound, chords become opt-in with an
   explicit timing window, and the UI must state the latency cost on that
   key. Never silently.
3. Note the physical reality: a human hitting two arcade buttons "together"
   lands them 10–30 ms apart, so any window under ~25 ms will feel broken.
   Fighting games solve their own version of this with input leniency —
   and many already ship 2-button macro assignments in-game, which is a
   better place to solve it when available.

Model change required: `Binding` gains a condition — the cleanest shape is
a `when: [Key…]` (all-of) guard rather than a new binding *kind*, so a
chord is "this binding, but only while these other keys are also down."
That composes with everything else instead of forking the model.

### 1c. Macros — a different subsystem, not a bigger binding

Hadouken is ↓, ↘, → + punch **over time**. That is not a set, it is a
timeline, and it needs three things the engine does not have: a clock, a
queue, and a policy for what happens when reality interrupts.

Design sketch:
- A macro is a list of `(state-delta, hold-duration)` steps compiled ahead
  of time; the output thread walks it on a timer, publishing states.
- **Minimum step duration is a hard rule** (§0.2): default ≥33 ms, and the
  editor should refuse shorter unless the user opts into "may be missed".
- **Interruption policy must be explicit**: releasing the macro key
  mid-run → finish, or abort-and-neutralize? (Fighting games want
  *finish*; a "hold to auto-fire" macro wants *abort*.) Per-macro setting.
- **Crash safety is non-negotiable**: a macro in flight when ksx dies must
  not strand buttons. Our crash-only guarantee already releases everything
  when the pads vanish, but a macro must also be cancelled — and released —
  on session stop, escape gesture, and hot-swap (the same neutral-delta
  path FIX 3 added).
- **Fairness**: macros are a first-class arcade tradition (real cabinets
  wire one button to multiple micro-switches) but online play and some
  anti-cheat treat sequence automation differently. ksx should ship them
  without apology for local/cabinet use and state the caveat once.

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
6. **SOCD policy, user-visible.** Left+Right → neutral / first-wins /
   last-wins ("snap tap"), applied at submit for both dpad and stick. We
   already have a fixed neutral rule inside the DS4 mapper; it should be
   engine-level, configurable, and stated — tournaments legislate this.
7. **NOT / exclusion conditions.** MAME's input sequences support `NOT`;
   it is how a binding avoids firing while a modifier is held. Falls out
   free if chords are implemented as a `when` guard (§1b) — add `unless`.
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

## 3. The architecture this all implies

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
4. **The transform stage** (clock + context), then in order: turbo,
   tap-hold, SOCD policy, chords (with the latency warning), analog
   shaping.
5. **Macros** last of the big ones — they need the scheduler, the
   interruption policy, and the sampling rule, and they are the easiest to
   get subtly wrong.
6. **Input display** alongside whichever of the above ships first; it is
   how the user (and we) will debug all of it.

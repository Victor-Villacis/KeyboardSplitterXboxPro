# ksx Studio — design system

The one place that says what a thing should look like. `studio-ui/src/studio.css`
is the implementation; this file is the reasoning. If the two disagree, the CSS
is the bug.

Two screens use it: **Status** (`/`, StatusIsland.ts) and the **Mapper**
(`/map`, MapIsland.ts). Both are viewed on a desk monitor *and* on an arcade
cabinet panel from across a room, in a light and a dark theme, with and without
JavaScript. Everything below is chosen against those four constraints at once.

---

## 0. Why this exists

The pre-v14 page was a functional app with ad-hoc styling: forty-odd one-off
`font-size` values between `0.58rem` and `1.6rem`, spacing picked per rule
(`0.35rem`, `0.42rem`, `0.55rem`, `0.7rem`, `0.9rem`…), `outline: none` on the
two most-clicked controls on the mapper, and one accent colour doing the work of
"live", "primary", "link", "value" and "identity" simultaneously. Individually
none of that is wrong. Together it is the exact profile of an interface that was
*assembled*, and it is what "it feels very amateur" means.

The fix is not more polish on each rule. It is a system: a fixed set of values,
named by meaning, that every rule draws from — so consistency is the default and
inconsistency has to be typed on purpose.

### The concrete tells this pass was written against

Distilled from how 2026's best developer tools (Linear, Vercel/Geist, Stripe,
Raycast, Zed) actually build, and checked against PadForge — the nearest
neighbour to this app — screenshot by screenshot:

| Tell | Rule here |
| --- | --- |
| Spacing values that aren't on a grid | Every margin/padding/gap is a `--sp-*` step (§2) |
| A font size invented per component | Every `font-size` is a `--fs-*` step (§1) |
| More than ~10 colours in the palette | One neutral ramp + accent + 3 status + 1 identity (§3) |
| Drop shadows on everything | Shadows only on genuinely floating things (§5) |
| Default browser focus ring surviving anywhere | One ring, declared globally, never removed (§7) |
| Radii that aren't from a scale, controls rounder than their card | `--r-*`, controls ≤ card (§4) |
| Proportional digits in a live readout | `tabular-nums` on every mono/numeric surface (§10) |
| Header/cell alignment mismatch in a table | Column alignment is set once per column (§10) |
| Motion on high-frequency actions | Slot/tab switching is instant; motion is for rare events (§8) |

---

## 1. Type

One system sans + one mono. No webfonts: this page is served by a Rust binary on
localhost and must paint before anything else loads.

```
--font-sans  system-ui, -apple-system, "Segoe UI Variable Text", "Segoe UI", Roboto, …
--mono       ui-monospace, "Cascadia Mono", "Cascadia Code", Consolas, monospace
```

### Scale

Product UI runs smaller than a marketing page: **14 px is body**, not 16.

| Token | px | Used for |
| --- | --- | --- |
| `--fs-micro` | 11 | uppercase eyebrows, badges, pills, table headers |
| `--fs-xs` | 12 | mono tags, metadata, footnotes, hints |
| `--fs-sm` | 13 | secondary body, dense rows, legend rows |
| `--fs-md` | 14 | **body**, control text, buttons, inputs |
| `--fs-base` | 15 | emphasised body, primary-action text |
| `--fs-lg` | 17 | card titles, subheads, preset name |
| `--fs-xl` | 22 | modal titles |
| `--fs-2xl` | 28 | screen headline (narrow viewports) |
| `--fs-hero` | 38 | the session state line — the one thing read across a room |

### Weight

`400` read · `500` interact · `600` announce. **600 is the ceiling for UI
chrome.** Hierarchy comes from size + a narrow weight band + text colour, never
from bolding harder. `700` survives in exactly one place: mono legends and
keycap-style tags, where it is doing the job of a printed marking rather than of
a heading.

### Tracking

Tracking scales inversely with size, because large type is optically looser:

```
--track-hero     -0.02em    38 px display
--track-title    -0.011em   17–28 px headings
--track-eyebrow  +0.045em   11 px UPPERCASE only
```

Uppercase micro-labels are the single deliberate exception to "tighter as it
gets bigger" — small caps need the extra air. Everything else at body size sits
at 0.

### Line height

`--lh-flat 1` (single-line controls) · `--lh-tight 1.2` (headings) ·
`--lh-snug 1.35` (dense rows) · `--lh-normal 1.55` (body) · `--lh-loose 1.65`
(footer, long help).

Measure is capped: `82ch` for orientation copy, `78ch` for help text. A sentence
that runs the full 1280 px column is not readable, it is just wide.

---

## 2. Space

4 px atomic unit, 8 px practical increment. Nothing between steps.

```
--sp-1  4    --sp-2  8    --sp-3  12   --sp-4  16
--sp-5  20   --sp-6  24   --sp-8  32   --sp-10 40
--sp-12 48   --sp-16 64
```

Applied consistently: control padding `8–12`, card padding `20–24`, section gap
`20`, page gutter `24`. Dense-not-cramped is the goal — what reads as cramped is
almost never density, it is *inconsistent* density.

### Control geometry

One height ladder, so a button and the select beside it line up without either
being told twice:

```
--ctl-h-sm  28px   inline row accelerators (legend ✕, macro step verbs)
--ctl-h     36px   the default — buttons, selects, inputs, tabs, nav
--ctl-h-lg  44px   the primary action of a screen (Start / Stop)
```

**Touch:** under `@media (pointer: coarse)` the whole vocabulary grows to a
40 px minimum (36 px for the small tier). A cabinet panel is touched; a desk is
not, and paying the touch tax on both would make the desk view sparse for
nothing.

That growth is implemented by **redefining the three ladder tokens on `:root`**
inside the coarse query (studio.css §4.4b), not by listing the components that
should grow. The distinction is the whole finding of the 2026-08-08 responsive
pass: it *was* a list, and in the rendered output the list was false. `select`
and `input[type=text|number]` re-declare `min-height: var(--ctl-h)` in §4.4,
below the list and at equal specificity, so the later rule won and **every
field on every screen rendered 36 px on a touch panel** — /pads' three spawn
selects, /profiles' six new-profile fields, /setup's four selects, /devices'
alias box, all measured with touch emulation against the HTML the server
actually sends. A custom property set on `:root` is not in the cascade with the
component rules that read it, so nothing added later can shadow it, and every
consumer moves together — including §6.1's sticky `calc(var(--ctl-h) + …)`,
which stays correct because the header it offsets grew by the same amount.

A component that needs its own touch exception (`.maplink`, the tile's only
control, sized as a marking rather than as a button) declares it **after** that
component's own rules, for the same cascade reason.

---

## 3. Colour, as meaning

~90 % of both themes is the neutral ramp. Colour is spent, not decorated.

The ramp is **violet**, taken from the app's own mark. Everything in this
section is measured, and re-measured on every `cargo test` — see §3.6.

### 3.1 Why purple is the GROUND and not the accent

This is the decision most likely to be re-opened by somebody who looks at the
icon, sees a purple app, and reaches for purple as the accent colour. It was
tried, it was measured, and it fails. The numbers, so nobody has to guess:

| Candidate accent | On `--panel` `#1c1428` | Verdict |
| --- | --- | --- |
| `#8b5cf6` — the mark's violet | **4.20:1** | **FAILS** the 4.5 floor |
| `#a98ae0` — a lavender that passes | 6.27:1 | Passes, and is **the same hue as the ground** |
| `#2fd8c5` — teal, unchanged | **9.96:1** | Passes with room to spare |

Two separate problems, and the second is the one that matters:

1. **`#8b5cf6` is not legible enough.** 4.20:1 on the panel, under the floor.
   It clears 4.5 on `--bg` (4.53:1) and fails on the panel — i.e. it fails
   exactly where most text actually sits.
2. **The lavenders that pass stop meaning anything.** A colour is an accent
   because it is *not the ground*. `#a98ae0` reads 6.27:1 and is perfectly
   legible, but it is the ground's own hue at a different lightness, so "this
   control is live" becomes "this control is slightly brighter" — which is
   what §11 says is invisible from across a room.

So the ground took the purple and **the accent stayed teal** — and it got
*better*, not worse: teal is near-complementary to violet, so `#2fd8c5` scores
**9.96:1** on the new panel against **9.75:1** on the blue-steel it replaced.
That is also why this was a token swap and not a rewrite: `render.rs`,
`render_map.rs` and `MapIsland.ts` needed no semantic change, because no
colour changed *meaning*.

**The mark's four player colours do not come into the UI.** The icon uses
red/blue/green/yellow because a static image has no other way to say "four
players". The UI already has a token for that question — `--cool`, identity —
and it has `--danger` for red. Colouring slot 2 red because P2 is red on the
mark would read as *"slot 2 has failed"*.

### 3.2 The ramp

**Dark** — the default.

| Token | Value | Role |
| --- | --- | --- |
| `--bg` | `#120c1c` | page ground |
| `--bg-2` | `#0b0714` | recessed ground (also a real card ground) |
| `--panel` | `#1c1428` | a panel |
| `--panel-2` | `#171021` | nested inside a panel |
| `--panel-3` | `#241a33` | a control under the cursor |
| `--line-soft` / `--line` / `--line-strong` | `#2a1f3a` / `#3b2c52` / `#54406f` | separation ladder |
| `--text` / `--text-2` / `--text-3` | `#f0ebe0` / `#bcb0c9` / `#8f83a3` | the three tiers |
| `--accent` | `#2fd8c5` | **unchanged** |
| `--cool` | `#a98ae0` | identity |
| `--ok` / `--warn` / `--danger` | `#4ed185` / `#edbb46` / `#f4675f` | state |
| `--danger-fill` | `#c5312a` | Stop (was `#d93f36` — see §3.5) |

Text is **cream, not blue-white**: on a violet ground a cool white reads as a
second hue. It is still not pure white, for the cabinet reason below.

**Light** — warm paper, the same family read as a tint.

| Token | Value | Role |
| --- | --- | --- |
| `--bg` | `#f6f3ee` | page ground |
| `--bg-2` | `#ece7df` | recessed ground |
| `--panel` | `#ffffff` | a panel |
| `--panel-2` | `#efe9e2` | nested |
| `--panel-3` | `#e8e1d8` | hovered control |
| `--line-soft` / `--line` / `--line-strong` | `#eae3da` / `#ddd3ca` / `#bfb2a4` | separation ladder |
| `--text` / `--text-2` / `--text-3` | `#1c1428` / `#4c4059` / `#6d6180` | the three tiers |
| `--accent` | `#09645c` | see §3.5 |
| `--cool` | `#6b3fa8` | identity |
| `--ok` / `--warn` / `--danger` | `#156c43` / `#7c5805` / `#ac2a23` | state |

Every coloured role in the light theme is **darker than its pre-purple value**.
A cream ground has less luminance headroom than the blue-white one it replaced,
and the old values were tuned against `#ffffff`/`#f3f5f9`.

### 3.3 Roles

Components reference **roles**, never the ramp — that is what makes both themes
come out right from one rule.

| Role | Meaning |
| --- | --- |
| `--surface` / `--surface-sunken` | page ground / recessed ground |
| `--surface-raised` | a panel |
| `--surface-inset` | something nested inside a panel |
| `--surface-hover` / `--surface-overlay` | pressed-plate / floating surface |
| `--text-primary` / `--text-secondary` / `--text-tertiary` | the three tiers |
| `--border-subtle` / `--border-default` / `--border-strong` | separation ladder |
| `--accent`, `--accent-fill`, `--accent-on` | live, primary, selected |
| `--ok` / `--warn` / `--danger` | state |
| `--cool` | identity — a device, a persona, a group of keys |
| `--focus` | the focus ring, and nothing else |

### 3.4 What each colour is allowed to mean

- **Accent (teal)** — *live, primary, selected, and the current binding.* It is
  the Start button's fill, the running pill, the active slot tab, a bound
  control's ring, a bound key's chip. It is **not** used for decoration, card
  chrome, or "make it pop".
- **`--cool` (violet)** — *identity*: a persona name, a macro name, the
  badge that says "these keys are a group". Distinct from accent because
  "which device is this?" and "is this live?" are different questions. It is
  the ground's hue on purpose: identity is a *label*, not a state, and it must
  not compete with the accent for "live".
- **ok / warn / danger** — state only, and always as the **dot + tint +
  full-strength text** triad, never a large solid fill. `--danger-fill` (a solid)
  exists for exactly one control: `Stop`.
- Everything else is the neutral ramp.

### 3.5 Contrast — the measured floors

Text is ≥ 4.5:1 against the surface it sits on, in both themes. Dark text is
cream, never pure white — pure white blooms on a TV panel, which is what a
cabinet screen is.

**The pair that matters is the composed one.** A pill is not accent-on-panel,
it is accent on `--accent-dim` *over* a panel — §3.4's triad. Measuring the
token against the panel and calling it done is how three of the four defects
below survived review:

| Floor | Applies to | Why |
| --- | --- | --- |
| **4.5:1** | anything that renders as text, **including a role on its own tint** | WCAG 2.2 AA |
| **3.0:1** | the focus ring against every ground it can land on | WCAG 2.2 AA non-text (1.4.11) |
| **1.12:1** | hairline separators | *calibrated, not WCAG* — see below |

The 1.12 separator floor is not an accessibility threshold and does not pretend
to be one. Hairlines here are decorative (§5: "dark mode separates by surface
lightness + a hairline border"); the perceivable boundary 1.4.11 cares about is
the focus ring, held to 3.0. 1.12 is the faintest separator the *outgoing,
reviewed* theme shipped (dark `--line-soft` on `--panel`), so it encodes "no
worse than what we already accepted" and catches a token going flat.

#### The tightest numbers in the set

The whole table is printed by the gate (§3.6); these are the ones with the
least room:

| Pair | Ratio |
| --- | --- |
| light `--accent` on `--accent-dim-2` over `--bg-2` | **4.52** |
| light `--ok` on `--ok-dim` over `--bg-2` | 4.57 |
| light `--warn` on `--warn-dim` over `--bg-2` | 4.59 |
| light `--text-3` on `--panel-3` | 4.76 |
| light `--text-3` on `--bg-2` | 5.02 |
| dark `--text-3` on `--panel` | 5.04 |
| dark `--accent` on `--panel` | 9.96 |
| dark `--accent-on` on `--accent-fill` | 9.20 |

#### Four defects this pass found and fixed

1. **light `--accent` `#0b7d72` → `#09645c`.** The specified value measures
   4.53:1 on `--bg` — fine — but **3.33:1** once the accent's own 16 % tint
   sits on `--bg-2`, and 4.16:1 as plain text on `--panel-2`. That composition
   is `.pill-run`, `.pill-paused`, `.flash-ok`, `.btn-undo` and `.macdur`, i.e.
   the running-status pill. Darkened until the worst composition clears 4.5.
2. **dark `--danger-fill` `#d93f36` → `#c5312a`.** `--danger-on` on it was
   **4.16:1** — and had been since before this palette. It is a text-on-fill
   pair, independent of any ground, so no theme change was ever going to
   surface it. 5.10:1 now.
3. **light `--ok`/`--warn`/`--danger` darkened** (`#17784a`→`#156c43`,
   `#8a6205`→`#7c5805`, `#bf2f27`→`#ac2a23`): all three fell to ~3.9:1 on their
   own tint over the new cream `--bg-2`.
4. **`--surface-base` did not exist.** `.macrowdur` — the one editable field in
   the macro grid — set `background: var(--surface-base)`, an undefined token,
   which resolves to transparent. Every other input uses `--surface-inset`.

#### A fifth defect, found by the adversarial pass

5. **light `--text-3` `#6d6180` → `#685c7a`.** It cleared 4.5 on every ground
   except `--panel-3`, the hover fill, where it measured **4.41:1**. The gate
   had not caught it because the tier loop deliberately skipped that one pair,
   justified in a comment by "every rule that sets `--panel-3` also sets
   `--text` or `--text-2`" — an unreachability claim about markup, which is
   exactly the kind of reasoning §13.7 says must not live in a comment. The
   stylesheet already held the counter-example: `.tab .tabnum` is a descendant
   with its own `--text-tertiary`, so `.tab:hover`'s `color:` never reaches it.
   That element is not rendered today, so nothing was shipping at 4.41 — but
   the rule sat in the stylesheet one markup change away from it. Fixed the
   token (§13.6), deleted the carve-out, and the tier loop now checks all
   three tiers on all five grounds.

#### Recorded exemption: disabled controls

The status page's daemon-down block renders a dimmed `<select>` and a dimmed
`Start` button (`.controls.off`, `opacity: 0.40`). The label measures
**3.45:1** in dark and **2.36:1** in light, and both stay.

WCAG 2.2 SC 1.4.3 exempts text that is part of an *inactive* user interface
component, and the exemption is the right call rather than a loophole here:
"dimmer than the live controls" is the entire signal that the control is
unavailable, so raising it to 4.5 would remove the affordance it exists to
provide. Two things make it safe — the **reason** is not dimmed (the
`<p class="warn">` in the same block sits outside both opacity selectors and
is measured at full strength), and the control is genuinely inert rather than
merely styled to look it.

Worth knowing if you touch it: two rules set opacity on that button,
`.controls.off .btn` (0.40) and `.btn:disabled` (0.42). The three-class
selector wins, so **0.40 renders**. And because CSS `opacity` composites the
element's whole buffer over its parent, the text and the field fade *together*
— the pair cannot be measured as two tokens, which is why it went unmeasured
until now. The gate pins both numbers.

#### Recorded exemption: the controller art's printed markings

`--hw-xbox-b` and `--hw-xbox-x` carry white letters at **4.00:1** and
**3.94:1**, under the floor, and they stay. The art is a *picture of a device*,
not a label — repainting an Xbox B button would make the drawing wrong — and
the letter is not the accessible name: every hit zone gets an `aria-label`
naming the control and every key bound to it (`render_map.rs`). The gate pins
both ratios, so changing them is a decision somebody makes on purpose rather
than a regression.

The DS4 disc is the one value there that is *ours* rather than the hardware's;
it moved from blue-grey `#2b3346` to `#2e2740`, and all four Sony glyphs gained
contrast (worst: cross, 4.60 → 5.17).

### 3.6 The gate

Numbers in a document do not defend themselves, so these are re-derived on
every `cargo test`:

- **`crates/ksx-studio/tests/contrast.rs`** — parses `studio-ui/src/studio.css`
  (it does *not* restate the palette; a test that hardcodes the values is a
  second copy that drifts the same way), composites every tint over every
  ground, and checks 154 pairs across both themes. It also pins two
  hand-mirrored copies of the tokens that had **already drifted**: the
  anti-flash `PERSONALITY_CSS` in `render.rs`/`render_map.rs` (which was
  painting `#0b0e14` while the stylesheet had moved to `#0a0d13` — a wrong
  anti-flash colour looks exactly like the flash it exists to prevent), and
  the controller-art palette in `build.mjs` (an `<img>`-embedded SVG is its own
  document and cannot inherit a custom property).
- **`crates/ksx-cabinet/tests/contrast.rs`** — reads `theme::role` directly,
  because there the Rust constants *are* the theme, and checks the composed
  pairs that only exist on that surface: the focus plate, a state colour on
  `tint(colour, 38)`, `ACCENT_ON` on `OK`. 58 pairs. It also asserts the two
  surfaces agree on `--accent` — they diverge on *size* on purpose (body 28 vs
  14, hero 68 vs 38) and must not diverge on what "you are here" looks like.

Both print their full table under `--nocapture`; the failure message names the
pair, the ratio and the floor, and says to fix the **token**, not the component
rule that revealed it.

---

## 4. Radius

```
--r-xs 4   chips, key tags, macro cells
--r-sm 6   small buttons, code chips
--r-md 8   buttons, inputs, selects, tabs
--r-lg 12  cards, panels, toasts
--r-xl 16  modal
--r-pill   pills, the selection bar
```

Rule: **a control's radius is never larger than its container's.** A button
rounder than the card it sits in reads as a toy.

---

## 5. Elevation

Dark mode separates by **surface lightness + a hairline border**; a shadow on a
dark ground is a darker patch on something already dark and simply disappears.
Light mode reintroduces very soft shadows.

```
--e-1  a resting panel (barely there)
--e-2  the hero, the stage — the two things that ARE lifted
--e-3  popovers, toasts, the selection bar
--e-4  the modal
```

Real shadows are reserved for genuinely floating elements. Docked panels get
`--e-1` or nothing.

---

## 6. Component vocabulary

Everything on both screens is one of these. Adding a widget means adding it
here, not styling it in place.

| Class | Notes |
| --- | --- |
| `.btn` | secondary by default (most buttons are) |
| `.btn-primary` | **solid** accent fill — one per context |
| `.btn-ghost` | transparent until hovered |
| `.btn-danger` | solid — `Stop` only |
| `.btn-danger-ghost` | destructive but secondary: outline, end of the row |
| `.btn-lg` / `.btn-row`, `.btn-mini`, `.btn-sm` | the three size tiers |
| `.btn.is-loading` | spinner, fixed width, no layout shift |
| `select`, `input[type=text|number]`, `.bindlabel` | label-above-field |
| `.pill` (+ `-run`, `-ok`, `-warn`, `-down`, `-idle`, `-paused`) | state chip; carries a dot |
| `.card` | the one container; its `h2` is an eyebrow with an accent tick |
| `.card.hero` | the primary panel of a screen |
| `.alarm` / `.alarm.warn` / `.alarm.paused`, `.warnbox`, `.flash` | banners: left rule + tint + coloured title |
| `.drow` / `.dname` / `.dvalue` / `.ddetail` | key-value settings row |
| `.plist`, `.slottable`/`.strow`/`.stcell` | lists and the slot table |
| `.tabs` / `.tab`, `.topnav` / `.navlink` | segmented navigation |
| `details.card` + `summary` | disclosure (the macro editor) |
| `.mlayer` / `.modal` | overlay |
| `.toasts` / `.toast` (+ `-ok`, `-warn`, `-err`) | action reports |

---

## 7. States

**Every interactive element declares hover, active, focus-visible and disabled.**

- **Focus** — one global rule: `outline: 2px solid var(--focus); outline-offset:
  2px`, on `:focus-visible` only. Declared once at `:where(a, button, input,
  select, textarea, summary, [tabindex])` so nothing can be missed. The offset
  gap is what makes it read on both a dark and a light plate. Two controls used
  to do `outline: none` — the hit zones on the controller art and the legend
  rows, i.e. the two most-clicked controls on the mapper. They now keep the ring
  *and* their hover treatment.
- **Hover** — lightness only, never a hue change: surface goes one step up,
  border one step stronger.
- **Active** — the fill goes back down; the transition is the shortest one.
- **Disabled** — dimmed + `not-allowed`. Note the local rule that predates this
  system and still holds: a control that *cannot act right now* is dimmed but
  still clickable, because a click on it has to be able to say **why** — a
  `disabled` attribute swallows its own click.

---

## 8. Motion

```
--dur-1 90ms    state change under the cursor
--dur-2 150ms   something appearing
--dur-3 240ms   an overlay
--ease-out       cubic-bezier(0.22, 1, 0.36, 1)
--ease-standard  cubic-bezier(0.4, 0, 0.2, 1)
```

Nothing exceeds 240 ms. Only `opacity`, `transform`, and colour animate — never
anything that triggers layout.

**What deliberately does not animate:** switching slot, switching macro, opening
the learn modal's countdown. Anything invoked many times a session must feel
instant; motion is for the rare and the noteworthy (a toast arriving, the modal
opening, the running pill's slow pulse).

`prefers-reduced-motion: reduce` collapses every duration to ~0 globally.

---

## 9. Empty, loading, error

- **Empty is a state, not a blank box.** `.macgrid.empty` says what is missing
  *and* what to do about it, on a dashed plate.
- **Error is a banner with a way out.** Every `.alarm` names the failure, says
  what still works, and prints the exact command for *this* machine.
- **Loading**: this page polls every 2 s and never blanks — stale data with a
  visibly frozen timestamp beats a spinner. `.btn.is-loading` exists for the
  write path.

---

## 10. Dense data

- `tabular-nums` on every mono surface, so live numbers do not jitter as they
  update.
- Numbers right, text left; a column header uses its column's alignment.
- Hairline separators, **never** zebra striping in an interactive list — stripes
  multiply against hover/selected/disabled into a pile of greys that fight.
- Secondary metadata is a right-aligned accessory (the legend's key chips, the
  slot table's keyboard column), never floating mid-row.

---

## 11. Cabinet legibility

This app is read from six feet away on an arcade panel. What that changes:

- The session state line is `--fs-hero` (38 px) and is the only thing at that
  size — one glance answers "is it running?".
- Status is dot + word + colour, never colour alone.
- Focus/selection is border **and** fill **and** colour together (the console-UI
  rule), because at distance a hue shift alone is invisible.
- Hit targets grow to 40 px under `pointer: coarse`.
- Text is `#f0ebe0` (cream), not `#ffffff`.
- The cabinet's own theme (`crates/ksx-cabinet/src/theme.rs`) carries the same
  palette, and deliberately keeps a **lighter `DANGER`** (`#ff6b6b`, not the
  web's `#f4675f`): that surface tints harder — `tint(colour, 38)` over a plate
  that is already lifted — and the web red measures 4.44:1 there.

---

## 12. Information architecture

The system above is what things look like. This is where they go.

### Status — three tiers, and they look like three tiers

1. **Primary — Session.** A hero bar: the state at 38 px on the left, the one
   action (Start / Stop + Reload) on the right.
2. **Secondary — Virtual pads**, then **Profiles** (what starts them).
3. **Tertiary — System.** Drivers, autostart, daemon process, config root, as
   key-value rows on a quiet panel at the *bottom*. Previously these were two
   half-empty cards in the middle of the page, shouting as loudly as the
   session; one of them was 80 % whitespace.

At ≥ 68 rem Profiles and System sit side by side (7/5), so the page stops being
one long vertical narrative.

> Moving the plumbing panel below Profiles used to permute `SHOW_ORDER` in
> `render.rs`, because `createShow` slots were positional (dogfood ledger #4).
> As of 2026-08-06 that tax is gone: compiler 0.3.1 names show slots after
> their condition getter (`show:vigemOk`), `render.rs` injects by name, and
> moving a panel is a move — no renumbering, no seam edit.

### Mapper — the controller is the hero

Reading order top to bottom: **banners** (only when true) → **slot rail** →
**one-line hint** → **the controller** → **bindings** → **macros (closed)** →
**presets & files**.

- The **slot rail** is navigation, not content: a sticky bar with the segmented
  slot switcher and the current slot's identity beside it. It used to be a card
  of pills that read as the page's first *content*.
- The **hint** was eleven lines of prose sitting between the rail and the
  controller — the manual, printed on the wall in front of the thing it
  describes. It is now one sentence with the rest behind a disclosure.
- **Macros** is a `<details>`, closed on arrival. It is a piano roll, four
  policy explainers and a TOML block, and it used to occupy ~40 % of the page in
  front of a user who came to map a button. Closed is not removed: it is still
  server-rendered markup, one click away, and it costs no `createShow` slot.
- **Presets & files** was a bare row of four buttons, and the answer to *"which
  file am I editing, which slots share it, where do backups go?"* existed
  nowhere on screen. It is now a real management surface: the preset's identity
  (name, path on disk, newest backup), a table of every slot and the preset it
  binds (rows are also a way to switch slot), then the four actions graded by
  consequence with the destructive one pushed to the far end of the row in an
  outline.

**No capability moved out of reach**, and no verb changed. The preset table is
the *same* `slotTabs` array the rail is built from, rendered a second time —
`list:slotTabs#2:array`, exactly the naming the status page's two profile-row
lists already use.

---

## 13. Rules for changing this

1. No raw hex outside the token blocks. Components consume roles.
2. No one-off `font-size` or spacing value. If the scale lacks a step, the
   question is whether the step or the design is wrong.
3. Any new interactive element declares all four states before it ships.
4. Any new colour has to answer "what does it *mean*?" — if the answer is "it
   looks nice", it does not go in.
5. Verify by looking, in both themes, at 1600 / 1100 / 420, in every state the
   screen can be in.
6. **If you are editing a component rule to fix a colour, the token was
   wrong.** Fix the token. A component that needs a colour the roles cannot
   express is either a new role (name it, justify it, add it) or a bug.
7. **A new colour ships with its ratio, or it does not ship.** Add the pair to
   §3.6's gate. If a pair is a deliberate exemption it goes in the gate *as*
   an exemption, with the reason and the pinned number — not in a comment
   somewhere else, and not nowhere.

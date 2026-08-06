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

---

## 3. Colour, as meaning

~90 % of both themes is the neutral ramp. Colour is spent, not decorated.

### Roles

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

### What each colour is allowed to mean

- **Accent (teal)** — *live, primary, selected, and the current binding.* It is
  the Start button's fill, the running pill, the active slot tab, a bound
  control's ring, a bound key's chip. It is **not** used for decoration, card
  chrome, or "make it pop".
- **`--cool` (steel blue)** — *identity*: a persona name, a macro name, the
  badge that says "these keys are a group". Distinct from accent because
  "which device is this?" and "is this live?" are different questions.
- **ok / warn / danger** — state only, and always as the **dot + 12 %-tint +
  full-strength text** triad, never a large solid fill. `--danger-fill` (a solid)
  exists for exactly one control: `Stop`.
- Everything else is the neutral ramp.

### Contrast

Text is ≥ 4.5:1 against the surface it sits on, in both themes. Two values moved
for this pass: dark `--text-3` is `#7d8aa3` (≈ 4.8:1 on `--panel`), and the
light accent dropped from `#0e9c8d` (3.4:1 on white — it was failing as text and
as a button fill) to `#0b7d72` (≈ 5.0:1 both as text on white and as white text
on it). Dark text is `#e3e9f4`, never pure white — pure white blooms on a TV
panel, which is what a cabinet screen is.

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
- Text is `#e3e9f4`, not `#ffffff`.

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

> Moving the plugging panel below Profiles permuted `SHOW_ORDER` in `render.rs`,
> because `createShow` slots are still positional (dogfood ledger #4). The
> permutation is a block swap of the two profile-row shows against the six
> driver/autostart pills; nothing else moved.

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

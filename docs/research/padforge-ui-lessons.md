# PadForge UI lessons for ksx Studio (studied 2026-08-05)

Victor found [PadForge](https://github.com/hifihedgehog/PadForge) (hifihedgehog,
C#/.NET 10/WPF, v4.1.0, 2,843 commits, active) and asked what we can learn.
Verdict: **a lot about interface, nothing about code** — and its existence
validates several of our roadmap bets.

## What PadForge is (and is not)

Physical-controller → virtual-controller remapping over SDL3 input, with
HIDMaestro as its virtual-pad engine (Xbox One/Series, DualSense, Switch Pro,
225+ profiles — our planned M8 backend, live in production today). It does NOT
do per-keyboard capture: SDL3 sees gamepads/wheels, not individual keyboards,
so it cannot split an I-PAC into players — ksx's core job remains unserved by
it. Same neighborhood, different street; complementary more than competing.

**License wall**: CC BY-NC-SA 4.0 — incompatible with our MIT OR Apache-2.0.
We may study interaction patterns (ideas are free); we may NOT copy code,
assets, or 3D models. Everything below is pattern-learning only.

## The three interface patterns worth stealing

1. **The controller IS the interface.** Interactive 3D model (HelixToolkit) —
   rotate/zoom/pan — where "buttons, sticks, and triggers highlight while you
   press them," PLUS a flat 2D schematic with identical live state for small
   displays. Two lessons inside this one:
   - **Live input echo is the soul of the screen.** Press the physical thing,
     see the virtual thing light up. For ksx that demo is even better than
     theirs: press an arcade button on the panel and watch the mapped pad
     control glow on the phone. This requires our planned live socket (E7's
     WS/SSE), which just became the Studio's next milestone after the skeleton.
   - **They ship 2D and 3D as equals.** So can we: an SVG schematic first
     (ours, tiny, crisp, theme-able), the spinnable 3D model as the flagship
     upgrade (Three.js bundled into assets — the hardcoded CSP forbids CDNs —
     rendering a CC0 glTF controller model; model sourcing is the long pole,
     license-vet before adopting).

2. **Record-a-binding.** PadForge: press a physical button, then pick the
   output from a dropdown. For an arcade cabinet the natural direction is
   REVERSED: click the control on the on-screen pad ("P1 · A"), then press the
   panel button you want bound. One gesture, no dropdowns, uses hardware we
   already built (RawInput identify + monitor). This is the GUI face of the
   M7 `ksx map` verbs and must go through them (CONTROL-SURFACE.md rule: no
   GUI-only paths).

3. **One-screen dashboard.** Their main screen: polling rate, device count,
   every virtual slot, servers, driver health. Ours (the skeleton being built
   now) already has the same instinct: driver health, pads, autostart,
   profiles, daemon state on one page. Keep it one screen; resist tabs until
   something earns one.

Victor's brief, which these three patterns satisfy: **"the most complex tool
with the simplest interface"** — complexity lives behind progressive
disclosure (per-mapping thresholds, SOCD rules, personas), while the surface
is: look at the controller, touch what you want to change.

## Roadmap consequences

- Studio staging after the skeleton: **live socket → 2D SVG mapper with live
  echo + click-to-assign → 3D viewer** (same live state, Three.js island).
- Their SOCD options (last-wins / first-wins / neutral) are user-visible
  policy; our DS4 hat collapse is fixed neutral-on-conflict. When the mapper
  UI lands, surfacing our SOCD rule (and possibly making it configurable at
  the engine layer, where history exists) becomes a real feature request.
- HIDMaestro shipping inside a 2,843-commit active product is strong evidence
  for M8's viability (and its WGI double-input caveat needs re-testing against
  their current experience).
- Their "web controller: phones AS gamepads, 16 at once, no install" is the
  mirror image of our "phone as config remote" — noted as an E-idea, not
  scheduled.

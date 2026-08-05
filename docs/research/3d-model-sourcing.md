# 3D controller model sourcing for the stage-4 viewer (verified 2026-08-05)

All licenses verified on the actual asset pages / Sketchfab API, not
aggregator claims. Full agent report distilled; candidates ranked.

**Headlines**: no CC0 branded-shape model exists anywhere findable — every
accurate Xbox/PS replica is CC-BY 4.0 (fine for our MIT/Apache repo: the
asset credit rides in THIRD_PARTY_NOTICES/about, it does not infect code).
Sketchfab auto-serves glTF for every downloadable model. The scarce property
is SEPARATE MESHES per control — required for highlighting — and most
listings don't document it.

| # | Model | License | Geometry | Separate parts? |
|---|-------|---------|----------|-----------------|
| 1 | **Xbox One S (Animated) — BatonyRobson** (Sketchfab a62cf747…) | CC-BY 4.0 ✔ | 6,846 tris — ideal | **Likely YES** (has a real animation ⇒ movable nodes; only machine-verifiable candidate) |
| 2 | PS5 DualSense — Taohid (Sketchfab b7bb9c51…) | CC-BY 4.0 ✔ | 104k tris — needs decimation/draco | unverified |
| 3 | DS4 — shaielwolf (Sketchfab e3c2f0dc…) | CC-BY ✔ | 62k faces | unverified; matches our actual persona (DS4) |
| 4 | PS4 — albertduranll (Sketchfab c10e207b…) | CC-BY ✔ | 3.3k faces — featherweight but chunky | unknown |
| 5 | Low Poly Controller — Ginsta (poly.pizza fCyA3Ug79X) | CC-BY 3.0 ✔ | 303 tris | likely fused; stopgap only |
| 6 | Gamepad — Poly Haven | **CC0** | 10k tris photoreal | **INSPECTED: fused single mesh, no sticks/triggers — near-useless** |

**Trade dress**: CC licenses cover copyright, not Microsoft/Sony trade
dress. A tool whose purpose is emulating these controllers depicting them is
classic nominative use — low risk, judgment call. Extra margin: flatten the
Xbox nexus / PS glyph decal textures.

**Fallback (genuinely cheap)**: self-author a generic pad in Blender —
body + each control a named separate object matching ksx control ids
(btn_a, stick_l, trigger_r…), pivots designed for tilt/hinge animation,
one body serving both personas via face-cap material swap. ~1–2 days
stylized low-poly, 3–5 with baked PBR; sub-1 MB GLB. Flat-shaded
dev-tool aesthetic legitimately sidesteps trade dress entirely.

**Plan of record**: download #1 + (#2 or #3), inspect node trees before
wiring anything (Sketchfab downloads need an account — Victor clicks, or
lends a token). If the PlayStation side is fused → self-author it. Rejected
for web-hostile geometry: wilsonR Xbox 239k, Oxicid DualSense 353k,
Saba.Lanchava DS4 661k. Kenney/Quaternius/OpenGameArt/Khronos have no 3D
gamepads at all.

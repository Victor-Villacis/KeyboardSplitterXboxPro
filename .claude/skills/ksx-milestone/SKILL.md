---
name: ksx-milestone
description: Execute a ksx milestone (M3-M7) the standard way - playbook process, workflow shape, adversarial reviews, gates, live cab test, commit/push. Use when starting or resuming any ksx milestone.
---

# Executing a ksx milestone

You are working on ksx — the Rust rewrite of KeyboardSplitterXbox at
`C:\Projects\KeyboardSplitterXboxPro`. The user's milestone request (e.g. "M3")
comes as the skill argument or from conversation.

## Before writing any code

1. Read `docs/PLAYBOOK.md` (process rules), `docs/ARCHITECTURE.md` (pipeline,
   milestone table with exit criteria), and the milestone's relevant research in
   `docs/research/` (M3: keyboard-capture-2026.md; M5: prior-art §4; M6:
   keyboard-capture-2026.md §4/§7; all: design-architecture.md + design-risk-review.md).
2. Check the task list (TaskList) for the milestone's task and mark it in_progress.
3. Verify machine state matches expectations with `cargo run -q -p ksx-app -- doctor`
   before touching anything driver-related. Facts from `doctor` beat docs.

## Execution shape (from PLAYBOOK.md)

Run implementation as a Workflow: contracts (if new shared types are needed) →
parallel implementers with strict crate ownership → **2 adversarial reviewers**
with distinct lenses (correctness-vs-legacy reading `legacy/` C# directly;
crash/hang/recovery safety) — this ratio is mandatory for driver-touching
milestones. Reviewers fix mechanical issues, report semantic ones.

Every agent prompt must include: repo path, required reading list, the crate(s)
it owns, the gate commands, "no git commits", and the CLI rules (stable exit
codes, --json).

## Definition of done

1. The full gate is green (exact commands in PLAYBOOK.md §4).
2. The milestone's live cab test from ARCHITECTURE.md's table passes — run it
   for real; driver-touching tests use `--features cab-tests`.
3. Safety-critical milestones (M3/M4/M6): kill-recovery verified
   (`taskkill /f` → keyboards return <1 s) before calling it done.
4. One milestone commit (conventions in PLAYBOOK.md) + push to origin master (SSH).
5. Task marked completed; memory file `ksx-rust-rewrite.md` updated with the
   milestone result and any machine-state corrections.

## Safety rails (never skip)

- Never run ksx emulation and the legacy KeyboardSplitter.exe at the same time.
- No Windows feature updates on this machine until M6 is done.
- Before any capture-layer experiment, re-read `docs/RECOVERY.md` and confirm a
  spare non-captured keyboard exists.

# Milestone checklist

The workflow, made mechanical. **Every milestone runs this loop** — it's not
optional and not per-taste. Copy the checklist block into the milestone's blog
draft (`docs/blog/NN-*.md`) and tick items as you go. CLAUDE.md points every
AI-assisted session here; this file is the source of truth for "how a milestone
is done on Ziran OS."

The loop maps to the adopted gstack skills (in `.claude/skills/`):

```
Think    →  Plan    →  Build   →  Debug        →  Review        →  Reflect
office-      eng-plan   (edit)     investigate      kernel-review     retro +
hours +                            (as needed)                        document-
scope-guard                                                           milestone
```

## Copy this into each milestone's blog draft

```markdown
### Milestone N — <name> — checklist

Think
- [ ] /milestone-office-hours — six forcing questions answered in this draft
- [ ] /scope-guard — verdict recorded (Hold / Reduce / Reframe / Reject)

Plan
- [ ] /eng-plan — approach, machine-state preconditions, edge cases, verification

Build
- [ ] Working, demoable state reached (per PLAN.md §5 "done" column)
- [ ] `make` is clean: assembles, compiles, links; `make check-header` passes
- [ ] No new compiler warnings; every new `unsafe`/asm block says why it's sound

Debug (as needed)
- [ ] /investigate used for any non-obvious bug; 3-attempt rule respected
- [ ] The real debugging story captured (what broke, how long, the fix)

Review
- [ ] /kernel-review over the diff; findings fixed or explicitly deferred

Security (only once there's a boundary — see PLAN.md §8)
- [ ] /red-team run if this milestone added a privilege/syscall/FS boundary
      (skip with a one-line "no new attack surface" otherwise)

Reflect
- [ ] /retro — post-mortem answered honestly
- [ ] /document-milestone — STATUS.md ✅, README status, blog post promoted
- [ ] Teaching tool updated — if this milestone changed the boot output or added
      a demoable behavior, add its line(s) to `web/index.html`'s `SCREEN` and a
      tour step to `STEPS` (the tutorial grows with the OS)
- [ ] CI green (build + headless QEMU boot)
- [ ] Committed and pushed (docs and code as separate commits where it helps)
```

## Notes

- **Think/Plan before Build, always.** The forcing questions and the eng-plan are
  cheap and repeatedly catch "I didn't define done" and "I assumed the wrong CPU
  state" — the two things that actually stall kernel work.
- **The `int3`-style self-test rule.** Wherever a milestone can include a built-in
  proof that the mechanism works end to end (like M4's breakpoint self-test),
  add one. It's the difference between "working" and "accidentally working."
- **Security is conditional, not skipped silently.** If a milestone adds no attack
  surface, write the one-line reason. If it adds a boundary, `/red-team` runs.
- **Honesty over tidiness.** In this environment the QEMU boot is exercised by CI,
  not always locally — say what was actually verified where, and never invent a
  debugging story that didn't happen.

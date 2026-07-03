---
name: retro
description: Run a short retrospective at the end of a milestone — what broke, how long it took to find, the actual fix, and what to change next time. Produces the honest raw material for the milestone's blog post. Adapted from gstack's /retro.
---

# retro

gstack's `/retro` is a weekly team retrospective with per-person metrics. Solo,
the useful core is different: a milestone post-mortem that captures the
debugging story truthfully, because per PLAN.md §8 the "what broke and how I
found it" is the more honest and more interesting post — not "it works now."

Adapted from gstack's `/retro` (github.com/garrytan/gstack, MIT).

## When to use

At the end of each milestone, right after it reaches its "done" state and before
you move on — while the pain is still fresh enough to remember accurately.

## Prompts

Answer briefly and honestly. Vague, flattering answers make a worse blog post
and a worse next milestone.

1. **What actually happened vs. the `/eng-plan`?** Where was the plan right,
   where was it naive?
2. **What broke, and how long until you understood why?** The real number.
   "Two evenings on a page fault that turned out to be a missing `& !0xfff`."
3. **What was the actual root cause and fix?** In one or two sentences, the way
   you'd explain it to someone about to hit the same wall.
4. **What tool or technique earned its keep?** (GDB command, a QEMU flag, a
   serial print you added.) Worth reusing → note it for the next `/investigate`.
5. **What assumption cost you the most time?** This is usually the post's lede.
6. **One thing to do differently next milestone.** Concrete and small.

## Output

A short retro block appended to the milestone notes / blog draft. Then hand off
to `/document-milestone` to fold it into STATUS.md and the published post. Keep
the messy version — the dead ends are the value, not noise to sand off.

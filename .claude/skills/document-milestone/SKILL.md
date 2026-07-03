---
name: document-milestone
description: After a milestone reaches done, update the tracked docs and turn the retro notes into a publishable blog post. Refreshes STATUS.md, README status, and the docs/blog post for the milestone. Adapted from gstack's /document-release.
---

# document-milestone

gstack's `/document-release` refreshes stale docs after shipping. Here, the
"release" is a completed milestone, and the docs are the ones that make this
project's story legible: STATUS.md, the README snapshot, and the milestone's
blog post — the public trail that is itself a success criterion (PLAN.md §1).

Adapted from gstack's `/document-release` (github.com/garrytan/gstack, MIT).

## When to use

After `/retro`, once a milestone is done and its story is captured.

## Update, in order

1. **STATUS.md** — flip the milestone to ✅ with a one-line note on what landed;
   update the "Verification state" and "Next up" sections. Keep the table honest
   (🚧 for anything partial).
2. **README.md** — update the "Current status" table and, if the boot flow or
   demoable behavior changed, the "What happens when it boots" section.
3. **docs/blog/NN-*.md** — promote the draft (office-hours framing + eng-plan +
   retro war story) into a finished post. The spine is already there from the
   earlier skills; this is shaping, not writing from scratch. Update
   `docs/blog/README.md`'s index row from "outline" to "drafted"/"published."

## What a good post contains (PLAN.md §8)

- The learning goal, stated plainly.
- **What broke, how long it took to find, and the actual fix.** This is the
  point — do not sand it into a tidy "then it worked."
- Employer-agnostic, people-agnostic.
- Where useful, the exact command / register / table entry that mattered, so a
  reader hitting the same wall gets real help.

## Output

The three doc updates as a small, self-contained commit (docs only — keep it
separate from code commits so the history stays readable). Do not open a PR or
publish anywhere unless explicitly asked.

# Attacking the lies about disk layout

*Milestone 16 — the second security milestone. A flag hidden in the RAM disk with
no name, a crafted directory entry that reads it right out, and the one-field fix
that turns "in the image" into "in the data." The confused deputy from Milestone
15, one layer down: offsets instead of pointers.*

> New to this? Read [docs/concepts/filesystem.md](../concepts/filesystem.md)
> alongside — it builds ZranFS from nothing (a "file" is a header's lie about a
> byte array; a directory is that lie told recursively) and has the
> "Milestone 16: the aliased extent" section this post is the story behind.
> [Milestone 15](15-the-confused-deputy.md) is the same lesson one ring up.

Back in Milestone 12, I planted a secret on the RAM disk: the bytes
`FLAG{ziran-boundary-leak}`, written to the end of the image with **no directory
entry pointing at them**. `ls` can't see it, `cat` can't read it — navigation can
only follow entries, and no entry names the flag. The self-test proved it:
`read_path("/FLAG")` returns `NotFound`.

That boundary was real, and it held. But it was only ever the *navigation*
boundary. Milestone 16 is the milestone that asks the harder question — not "can I
walk to the secret?" but "can I craft a disk that hands it to me anyway?" — and
the answer, until this milestone, was yes.

> **The lesson, up front: a bounds check is only as trustworthy as the bound it
> compares against.**

Milestone 11's `mount` checked that every file's bytes were *in the image*. That
sounds like a bounds check. It is a bounds check. It's just a bounds check against
the wrong bound — the whole image, secret included — and "in bounds" against the
wrong bound is not a bound at all.

### Milestone 16 — break the filesystem boundary — checklist

```
Think
- [x] /milestone-office-hours — six forcing questions answered in the eng-plan
- [x] /scope-guard — verdict: Hold (tightening the toy format's own check is the
      milestone), with a Reduce to the data_end ceiling only

Plan
- [x] /eng-plan — approach, machine-state preconditions, edge cases, verification

Build
- [x] Working, demoable state: a crafted image captures the flag; the fix contains it
- [x] `make` clean: assembles, compiles, links; the FS stays panic-free on all input
- [x] No new compiler warnings

Debug
- [x] No build-time bug hunt — the eng-plan front-loaded the traps

Review
- [x] /kernel-review — no critical/high; 1 medium (the KIND_DIR gap), low fixes

Security
- [x] /red-team — strict mount total and holds vs. every entry-level attack;
      the KIND_DIR scope-asymmetry found and closed; residuals documented

Reflect
- [x] /retro — the honest post-mortem (below)
- [x] /document-milestone — STATUS.md ✅, README, concept doc, this post, web tour
```

## Office hours: what "done" means

The forcing question that shaped M16 was Q1 — *what will I understand after this
that I don't now?* The answer wasn't "how a filesystem parser works." It was
sharper: **the boundary the parser enforces and the boundary I *think* it enforces
are different, and the gap is exactly the size of the wrong bound.** M11's check
felt like it confined files. It didn't; it confined them to the image, which is
not the same as confining them to the data.

So "done" isn't "the secret has no name." That was true a milestone ago. Done is a
*contrast*, asserted in one boot:

1. **Navigation HELD.** `read_path("/FLAG")` → `NotFound`. No entry names it.
2. **Loose extent LEAKS → captured.** A crafted image whose file entry aliases the
   secret's bytes, run through the M11-era check, mounts and reads the flag —
   asserted **byte-equal** to the planted constant. A real capture.
3. **Strict extent CONTAINS.** The *same* image through the fixed reader →
   `ExtentEscapesData`. Flag safe.
4. **Strict still PERMITS.** The legitimate image mounts under the strict reader
   and every file reads byte-exact — the guard against a "fix" that just rejects
   everything.

## Scope-guard: isn't adding a format field "building a real filesystem"?

The objection to take seriously: a new on-disk superblock field, a format version
bump — that smells like growing a real filesystem, a PLAN §1 non-goal. The verdict
was **Hold**. The non-goal is FAT/ext-style features: writes, journaling, real
durability. Adding *one integer* to a hand-built teaching format so its own
validation can be tightened is not that — it's sharpening the existing trust
boundary, which is the literal content of Milestone 16. And it rhymes with M12,
which gave the previously-reserved byte meaning as the `kind` field. Each milestone
grows the format by exactly one field.

The **Reduce** kept it honest: the fix is a *ceiling* only (`data_end`), not a full
"every byte owned by exactly one file" accounting. That coverage machinery is a
real allocator's job and open-ended; the flag capture doesn't need it. The floor
(a file may still alias *other* files' bytes below `data_end`) and a generated
fuzzing flood are named as red-team territory, not built.

## Eng-plan: name where the data ends

The whole fix is one idea: the reader has to know where the legitimate data stops.
Format **v3** repurposes the superblock's last reserved word as **`data_end`** —
the offset where addressable file data ends. The writer sets it to the start of
the secret; the secret lives in `[data_end, total)`, past it. The strict `mount`
confines every entry's extent to `<= data_end`. An extent reaching past it is
`ExtentEscapesData`.

The teaching structure mirrors Milestone 15's `SYS_WRITE` / `SYS_WRITE_UNCHECKED`
pair: ship two readers. `Fs::mount` is strict; `Fs::mount_loose` keeps the M11
`total_size` bound. The self-test drives the *same* crafted image through both — it
leaks through one and is contained by the other. Same bytes, one comparison apart.

The eng-plan also caught the trap that would have wasted an evening: the format bump
breaks the hand-built corrupt-image tests (the `deep` and `diamond` cases write
their *own* superblocks, with the old reserved word as zero). A zero `data_end`
would now trip the new `BadDataEnd` check *before* those tests reached their
intended `TooDeep` / `DirNotForward` errors. So: rebuild and boot after the
writer/reader change and *before* adding the attack, so a format regression shows up
alone. It did exactly what the plan said it would, and the fix (write a valid
`data_end` in those images) was already written down.

## What got built

The crafted attack is one directory entry. Clone the boot image; repoint root entry
0 to a file named `leak` whose `offset = data_end`, `length = 25` — its extent is
now precisely the secret's byte range. Nothing about it is malformed by M11's rules.
`Fs::mount_loose` accepts it; `read_path("/leak")` returns the flag. `Fs::mount`
rejects it at validation. Straight off the serial log:

```
[m16] CAPTURED: a crafted file extent aliased the secret -> loose mount leaked "FLAG{ziran-boundary-leak}"
[m16] CONTAINED: strict mount rejected the escaping extent (ExtentEscapesData) -- flag safe
[m16] PERMITS: the legit boot image mounts strict and reads byte-exact -- a check, not a wall
```

The capture asserts byte-equality with the planted constant — a crafted entry that
returns *some* bytes proves nothing; it has to be the flag. The containment asserts
the specific `ExtentEscapesData`, not a generic size error. Boundary cases are pinned
too: an extent ending exactly *at* `data_end` passes, one byte past is rejected.

## Red-team: two adversaries, one gap

M16 is a security milestone, so the boundary got attacked in earnest — by fanning
out two independent reviewers with different charters: one auditing soundness
(does it stay panic-free and total?), one hunting exfil paths (can any crafted
image still return the secret?).

Both came back with the strict reader holding: no crafted image makes `mount` panic
or hang, and no entry-level attack returns the flag. The two ways to reach the
reserved bytes are the two the code documents and accepts — **forging the whole
superblock** to move `data_end` (an in-image field; the honest residual, and
exactly Milestone 15's "don't validate against attacker-controlled metadata" lesson
recurring), and **floor-aliasing** (a file overlapping *other* files' bytes below
`data_end` — leaks structure, never the secret).

And then both reviewers, independently, found the *same* real gap — which is how I
knew it was real without adjudicating. My first fix put the `data_end` ceiling
inside the `KIND_FILE` branch of the validator. But a directory table is an extent
too, and it went unchecked: a `KIND_DIR` entry could legally point its table into
the reserved `[data_end, total)` region, and `ls` renders entry bytes as *names* —
a disclosure channel `read_path` confinement never touched. It happened to be safe
for *this* secret only by coincidence: 25 bytes can't hold a 32-byte directory
entry, and the flag's bytes fail the `kind` re-check. Safe by accident of the data,
not by the check.

The fix: hoist the ceiling check *above* the file/directory split, so it applies to
every entry uniformly. One line moved; the guarantee went from coincidental to
principled. It's pinned now with a synthetic image whose directory table sits past
`data_end`, asserting `ExtentEscapesData`.

## Retro: the honest part

As with the last two milestones, nothing broke at build time (one trivial compile
error — I renamed a parameter in the body and not the signature). The real finding
came from review, not the compiler, and it's the lede: **I secured the door the
flag walks out of and left a window in the same wall.** The secret is *read*
through `read_path`, so I confined the read path — and forgot the parser has a
*second* extent-consuming path, the directory recursion, whose name-rendering is
its own way out.

The generalizable lesson is a checklist item, not a mood: when you add a bound to a
parser, enumerate *every* place the bounded quantity is consumed — here, file
extents *and* directory-table extents — and enforce the bound at the earliest common
point, before the type split, not per-branch. Structurally, the check belonged above
the `match`. Writing it there first would have made the asymmetry impossible instead
of a finding two reviewers had to catch.

The other thing worth keeping: running two adversaries with *orthogonal mandates*
and watching them converge. When a soundness auditor and an exfil hunter both land
on the same line, that's near-certainty. For a security boundary, don't run one
reviewer twice — run two with different jobs.

## Takeaway for the next person

If you're tightening a parser's bounds check:

- **Compare against the right bound.** "In the image" is not "in the data." A file
  that can name any in-bounds byte can name the bytes you were hiding. Find the
  bound that means *legitimate*, and if the format doesn't record it, add the field
  that does.
- **Confine every consumer of the bounded quantity, before the type split.** A
  filesystem reads extents in more than one place. Guard them at the common point,
  or you'll guard the one you were thinking about and miss the one you weren't.
- **Ship the loose reader next to the strict one.** The proof isn't "it rejected the
  bad image" — it's the same image leaking through the old check and contained by
  the new one, with a real file still passing. The contrast is the milestone.
- **Be honest about the residual.** `data_end` is an in-image field; a forged
  superblock reopens the leak. Naming that — rather than pretending a toy FS is
  hardened — is the whole spirit of the track. You break your own boundary to learn
  exactly where it ends.

Both flags are captured now — one behind the ring boundary, one behind the disk
layout — and both taught the same thing from opposite ends of the kernel: the CPU
and the parser will both faithfully do what a bounds check *tells* them, and a check
that trusts the wrong number is a check that leaks. The core arc was done at
Milestone 12; the security track is what made the boundaries real by trying, in
earnest, to get around them.

# The confused deputy

*Milestone 15 — the first security milestone. A flag planted on a kernel-only
page, a ring-3 program that tries in earnest to steal it, and the one door the CPU
cannot guard for you: the moment the kernel dereferences a pointer its caller
chose.*

> New to this? Read [docs/concepts/privilege.md](../concepts/privilege.md)
> alongside — it covers rings and CPL, the two bits that *are* the boundary (a
> segment's DPL and a page's U/S), and — for this post specifically — the
> "confused deputy" section at the end. [Milestone 13](13-the-first-wall.md) built
> the wall; this post attacks it.

Milestone 13 ended with a wall: a program running in ring 3, held back by the
hardware from touching kernel memory or running privileged instructions. The
enforcement test even watched the wall *stop* things — `cli` → #GP, a kernel read
→ #PF, both from CPL 3. It was tempting to call the boundary understood.

But a wall you've only watched from the outside is a wall you've assumed. M15's
whole job is to attack it in earnest — plant something worth stealing behind it,
then try to steal it — and to find the one place the wall isn't a wall at all.

> **The lesson, up front: the CPU stops *unauthorized access*, but only software
> stops *authorized misuse*.**

The CPU faults a ring-3 program that reads a kernel page directly. It does
*nothing* when the kernel — running in ring 0, at the program's request —
dereferences a pointer that program handed it. That gap has a name: the **confused
deputy**. Closing it is not a hardware bit; it's a function the kernel has to
remember to call. This milestone is about feeling exactly where the hardware stops
helping.

### Milestone 15 — break the privilege boundary — checklist

```
Think
- [x] /milestone-office-hours — six forcing questions answered in the eng-plan
- [x] /scope-guard — verdict: Hold (copy-from-user is the object of study, not
      hardening); the CR4.UMIP demo Reduced to optional, then cut

Plan
- [x] /eng-plan — approach, machine-state preconditions, edge cases, verification

Build
- [x] Working, demoable state: a ring-3 program captures the flag, then is contained
- [x] `make` clean: assembles, compiles, links; every new unsafe says why
- [x] No new compiler warnings

Debug
- [x] No build-time bug hunt — the design was front-loaded (as in M13)

Review
- [x] /kernel-review — no critical/high; 3 low-severity findings fixed

Security
- [x] /red-team — the validated path holds against every angle; one real DoS
      found and fixed; findings pinned as CI-asserted invariants

Reflect
- [x] /retro — the honest post-mortem (below)
- [x] /document-milestone — STATUS.md ✅, README, this post, concept doc, web tour
```

## Office hours: what "done" means

The forcing question that shaped M15 was Q1 — *what is the single thing I'll
understand after this that I don't now?* The honest answer was not "how ring 3
works" (M13 covered that). It was: **the boundary the CPU enforces and the boundary
the kernel has to enforce are different boundaries, and I've only built one of
them.**

So "done" isn't "a ring-3 program couldn't read the flag." A ring-3 program reading
the flag directly already fails — that's just M13's #PF with a new target. Done is
a *contrast*, asserted in one boot:

1. **Direct read → HELD.** Ring 3 does `mov rax, [SECRET_VA]`; #PF, U/S bit set,
   caught at CPL 3. The flag is never read.
2. **Confused deputy → CAPTURED.** Ring 3 asks a syscall to read `SECRET_VA` for
   it, and — with no validation — the kernel obliges and prints the flag. The
   captured bytes are asserted **byte-equal** to the planted constant, so it's a
   real capture, not console noise.
3. **Validated deputy → CONTAINED.** The same syscall call, now behind a
   copy-from-user check, returns `-1` and reads nothing.
4. **Validated deputy → still PERMITS.** The check passes a *legitimate* user
   buffer through. A validator that rejected everything would pass test 3 while
   being useless — this is the guard against "accidentally working."

Test 2 is the one that matters. Anyone can build a wall; the milestone is watching
the wall be *bypassed by the guard you put in front of it*, and then watching one
line of code close the gap.

## Scope-guard: is copy-from-user just "hardening"?

The obvious objection: adding pointer validation to a syscall sounds exactly like
"security hardening," a PLAN §1 non-goal. The verdict was **Hold**, and the reason
is the load-bearing distinction of this whole project. *Hardening for the world* —
making the OS withstand a determined attacker — stays out. But `copy_from_user`
here isn't defense against an outside threat; it's the **object of study**. The
milestone exists to watch the deputy leak the flag without it and hold with it. You
cannot learn what the check does without building both sides of it.

The one thing that *was* hardening got cut: permanently enabling `CR4.UMIP` to
close M13's `sgdt`/`sidt` leak. That's mitigating a boundary for its own sake, on a
kernel we've decided to leave deliberately breakable. It stays a footnote, not a
feature.

## Eng-plan: three pieces and one contract

- **The secret.** A 32-byte `FLAG{…}` on a dedicated **kernel-only** page at
  1.5 GiB, mapped with `map_page` (not `map_user_page`), so its leaf is U/S = 0.
  A fixed virtual address lets the attack blobs hardcode it as an immediate,
  keeping them the same style of hand-assembled position-independent blob as M13.
- **Two syscalls.** `SYS_WRITE(ptr, len)` — the kernel's first *pointer-carrying*
  syscall — and its deliberately-broken twin `SYS_WRITE_UNCHECKED`, identical
  except it skips the check. Same attack, one branch apart.
- **The blobs.** Four tiny ring-3 programs: a direct read (expects #PF), the
  unchecked-deputy leak, the validated-deputy rejection, and a legitimate write.
- **The contract — `user_range_ok(ptr, len)`.** Return true only if *every* page
  of `[ptr, ptr+len)` is `PRESENT | USER`, proven by a page-table walk that **never
  dereferences the memory**. This is the subtle part: a validator that reads the
  pointer to test it *is itself the confused deputy* — it faults on an unmapped
  pointer and succeeds on the very kernel page you're protecting. So you read the
  page *tables*, not the page, ANDing the U/S bit down the walk exactly as the MMU
  does. Reject overflow (`checked_add`) and — the part I got wrong the first time —
  non-canonical addresses.

## What got built

The validator walks `PML4 → PDPT → PD → PT`, requiring present-and-user at every
level and terminating correctly on a huge page — the same structure as the
existing `translate`, but AND-ing U/S instead of computing an address. The write
syscall bounds the length first (an unbounded copy on a caller's length is an
attack by itself), then — for the validated arm only — calls `user_range_ok`
before copying into a small kernel buffer. The unchecked arm just copies.

The four outcomes, straight off the serial log:

```
[m15] HELD: direct ring-3 read of the secret → #PF at CPL 3, flag never read
[m15] SYS_WRITE_UNCHECKED emitted 32 bytes: "FLAG{ring3-cant-read-kernel-mem}" (CPL=3)
[m15] CAPTURED: the unchecked deputy leaked the flag → "FLAG{ring3-cant-read-kernel-mem}"
[m15] SYS_WRITE rejected by copy_from_user: [0x60000000, +32) is not user-readable (CPL=3)
[m15] CONTAINED: copy_from_user rejected the kernel pointer — nothing leaked
[m15] PERMITS: the validated SYS_WRITE passed a legitimate user buffer — a check, not a wall
```

Read those two middle pairs together: the *same* kernel pointer, `0x60000000`, is
leaked by one syscall and rejected by the next. That diff is the milestone.

## Red-team: attacking my own guard

This is a security milestone, so the boundary got attacked in earnest — this time
by fanning out an independent adversarial reviewer whose only job was to make
`user_range_ok` hand back the flag. It couldn't, and *why* it couldn't is the best
thing I learned all milestone.

**The aliasing angle — held, and instructive.** The secret's physical frame is
allocated from low RAM, and the kernel identity-maps the first 1 GiB — so the flag
bytes are readable at *two* virtual addresses: `SECRET_VA = 0x60000000` and their
low identity alias. The attack: pass the *alias* to `SYS_WRITE`, hoping the walk
that denies `SECRET_VA` accepts the low address. It doesn't — but the reason is
subtle. When M13's `map_user_page` mapped the user pages, it permanently loosened
the *shared upper* tables (`PML4[0]`, `PDPT[1]`) to U/S = 1 so the walk could reach
a user leaf. You might fear that loosening exposed the identity window. It didn't:
the identity map descends through a *different* entry, `PDPT[0]`, which is never
loosened — because `map_user_page` **asserts** its target is above the 1 GiB
identity window, so no user mapping ever descends through `PDPT[0]`. That assert,
which reads like a sanity check, is doing real security work. One U/S = 0 anywhere
on the walk denies, and the identity alias hits one at `PDPT[0]`; the direct path
hits one at the secret's own PD entry. The wall holds because of a bit two levels
up that I'd have called incidental.

**The one real bug.** The validator walks the page tables using bits 0–47 of the
pointer (nine bits per level, four levels). But the syscall dereferences the
*full 64-bit* pointer. So a **non-canonical** address — high bits set, low 48 bits
naming a real user page — passes the walk, and then the kernel's own dereference
raises **#GP** (non-canonical access), in ring 0, on a fault path built only to
recover ring-3 faults. A ring-3 program couldn't steal the flag this way, but it
could *crash the kernel*. The fix is one comparison: reject any range whose end
crosses 2^47, the canonical low-half boundary, which every legitimate user pointer
sits far below.

That finding is the post's lede, so let me say it plainly: **I validated
*permission* and forgot *addressability*.** Present-ness of the pages protects
against #PF. It says nothing about #GP from a malformed address. Two fault classes,
one of them glossed — in the exact function whose entire purpose is to make the
subsequent dereference safe.

`/kernel-review` came back clean otherwise — no critical or high findings, the
validator's U/S logic confirmed correct, every hand-assembled blob checked
byte-for-byte against its comment and the ABI. Its two low findings were hygiene:
zero the capture buffer after a leak so the flag doesn't sit resident in a static
(in a module whose whole point is "don't leak the flag"), and spell out in the
unchecked syscall's doc that it's a crash primitive too, not just a leak. Both
fixed. And the red-team's three rejection cases — kernel page, non-canonical
pointer, overflow — are now asserted directly at boot, so they can't regress.

## Retro: the honest part

As with M13, almost nothing broke *during the build*. It booted to all four
outcomes on the first full run, because the eng-plan had already named the traps
(don't dereference to test; validate before copy so the copy can't fault; place the
secret with `map_page` so its leaf stays U/S = 0). The value of the plan was that
the bugs didn't compile.

The break came from the *review*, not the compiler, and that's the point worth
generalizing. The non-canonical-pointer DoS is exactly the kind of bug that a
happy-path demo — and even a boot that asserts four correct outcomes — sails right
past. It took an adversarial reader going looking for "what input passes the check
and then breaks downstream" to find it. Time-to-find: minutes, because someone was
hunting, not waiting for a crash report.

So the one thing to do differently next time is a checklist item, not a mindset:
when writing a validator, **enumerate the fault classes the downstream operation
can raise** — #PF, #GP, alignment — and confirm the check covers each, not just the
one that motivated writing it. "Don't dereference to test the pointer" was in the
plan. "And make sure the pointer you'll eventually dereference can't fault for a
*different* reason" was not.

## Takeaway for the next person

If you're adding your first copy-from-user path:

- **Validate the page tables, never the pointer.** Reading the user address to
  check it *is* the confused deputy — it faults on bad input and succeeds on the
  kernel memory you're guarding. Walk the tables; touch nothing.
- **Permission is not addressability.** Proving the pages are present and
  user-readable stops #PF. It does not stop a #GP from a non-canonical or
  misaligned address — and that dereference runs in ring 0, where a fault is fatal.
  Bound the range to the canonical user half explicitly.
- **The strongest test is a contrast, not a pass.** "It rejected the bad pointer"
  proves little on its own; "the same pointer leaks through the unchecked twin and
  is rejected by the checked one, and a real buffer still passes" proves the check
  is the thing doing the work.
- **Attack your own guard, out of process.** The bug wasn't in the wall; it was in
  the guard I put in front of the wall. A reviewer whose only goal is to bypass
  your validator finds things your own tests, written to confirm it works, never
  will.

The wall held. The guard in front of it had a gap — and the gap was the lesson.
Next, the same idea one layer down: the filesystem's extent check (M16), which
validates that data lives *in the image* but not *within a file's own bounds* — a
confused deputy made of disk offsets instead of pointers.

# What I actually learned building an OS with AI

*A capstone reflection. Not a milestone post — the standalone essay. The core
arc (boot → memory → interrupts → tasks → shell → files) is complete: twelve
milestones, ending in a file manager you can `cd` around in. Here is what the
build was really about, and what held up.*

## The frame

There are two boring stories you could tell about this project, and neither is
the one I want to tell.

The first is "I made an AI build an operating system." That's slop with a
narrator — output nobody verified, reviewed, or understood, dressed up as an
achievement. The second is "I hand-typed a kernel, the old-fashioned way," which
is a fine thing to do and not what happened here.

The interesting thing is in between, and it's a *method*: rigorous
AI-assisted systems engineering, tested in the least forgiving domain I could
find. Web code that's wrong throws an exception and you read a stack trace.
Kernel code that's wrong at ring 0 triple-faults and the machine silently
reboots — no trace, no line number, just a black screen and the smug little QEMU
logo again. If a disciplined AI-assisted loop can produce correct systems code
*there*, where the feedback is a reboot instead of an error message, it can
produce it anywhere. That was the real experiment. The OS was the test rig.

## What I set out to learn

Not "how to build an OS." What I wanted was to take the abstractions I'd trusted
on faith for years and turn each one into a mechanism small enough to hold in my
head. The measure of success was never the feature list — it was the number of
things that stopped being magic.

A few of them clicked hard enough that I can still feel it:

- A **task** is a saved stack pointer. That's the whole secret. Switching tasks
  is swapping `rsp` (and the callee-saved registers) so the next `ret` resumes
  *someone else's* call stack instead of yours. The scheduler is bookkeeping
  around that one move.
- **Preemption** is the exact switch a task would have made voluntarily — forced
  on it by a timer it can't refuse. `preempt()` literally calls the same yield
  path underneath. The timer doesn't change *how* you switch; it changes *when*
  the scheduler can be entered, and that's where every concurrency hazard lives.
- A **file** is a header's lie about a flat run of bytes. A **directory** is the
  same lie, told one level down — a file whose bytes happen to be more
  `{name, offset, length}` entries. Hierarchy costs one `kind` byte. The reading
  is nearly free; the hard part is proving you can read a tree you don't trust
  without looping, hanging, or overflowing.

None of those are things I *learned* in the sense of memorizing. They're things
I can now derive, because I watched them get built one honest layer at a time.

## The harness is the whole point

Here is the part that separates this from slop, and it isn't a vibe — it's a
mechanism, the same as everything else.

Every milestone ran the same loop, and the loop was mandatory, not per-taste:

- **office-hours** — six forcing questions to sharpen the target *before any
  code*: the one thing I'll understand, "done" as observable behavior, the
  smallest version that still teaches it.
- **scope-guard** — an explicit defense of the non-goals, because the biggest
  risk in a hobby OS isn't a bug, it's scope creep toward a "real" feature.
- **eng-plan** — the machine-state preconditions written down: which CPU state,
  which locks, which invariants this step depends on and can silently violate.
- **build** — the mechanical part.
- **adversarial, multi-agent code review** — a staff-engineer pass tuned for
  `unsafe`, assembly, and the invariants that make bare-metal bugs *silent*.
- **retro** and **document** — what broke, how long it took, the actual fix, in
  public.

And it *caught real bugs before they shipped*. This is the claim I can back with
commits:

- In the shell milestone, review found a boot that would **panic on the first
  keystroke** — the self-test asserted the input ring was empty, but `sti` runs
  before the test, so a key pressed during boot would already be sitting there.
  Fix: drain, don't assert. It also found the deadlock family that scares me
  most: **allocating under an interrupt-guarded spinlock**. A task preempted
  mid-allocation, holding the heap lock, then re-entered through the timer — a
  quiet hang, never a crash, invisible until the exact wrong tick.
- In the file-manager milestone, review found two ways a *crafted image* could
  take the machine down. A **diamond DAG** re-validating shared subtrees
  **exponentially**, so `mount` would hang. And a **deep chain** recursing past
  the guard-page-less **16 KiB** task stack, so `mount` would silently overflow.
  Neither was reachable with the fixed boot image. Both are exactly the surface
  an adversary would reach for.

The division of labor is the thing to notice. The human owns the vision, the
non-goals, and the loop. The AI does mechanical work *inside* the loop. And the
trail — self-tests asserted by CI, the reviews, the honest write-ups including
the dead ends — is the proof.

## The most transferable lesson

If I keep one sentence from this whole project, it's this: **a termination proof
is not a safety proof.**

The file-manager validator descends an untrusted directory tree. It had a genuinely
elegant termination argument — a forward-ordering rule (a child's table must
start strictly after its parent's) that made looping *impossible*. The comment
said "provably cannot loop," and that was *true*.

It was also only a third of what mattered. Terminating in bounded *depth* says
nothing about bounded *time* — a diamond DAG terminates and still runs
exponentially. And it says nothing about bounded *stack* — a deep chain
terminates and still overflows your 16 KiB. "Terminates," "bounded time," and
"bounded stack" are three separate guarantees, and quietly assuming the first
implies the other two is precisely how a validator ships a denial-of-service
while passing every "does it work?" test.

The fix names all three. A **monotonic high-water mark** makes total work
*linear* (each table validated at most once; re-entry rejected — which turns a
diamond DAG into a clean `DirNotForward`). A **`MAX_DEPTH = 32`** cap bounds the
*stack* (`TooDeep`). The forward-ordering rule keeps the *termination*. Corrupt
images the self-test rejects went from eight to **ten**. This lesson generalizes
far past kernels: any time you write "provably X," enumerate the resource bounds
separately, or you're bluffing.

## What surprised me

Four milestones in a row — the scheduler, the shell, the read filesystem, and
the file manager — shipped with **zero runtime debugging**. No triple-faults, no
GDB bug-hunts, no recompile-and-pray.

That surprised me, and the *why* matters, because it wasn't that the work was
easy — context switches and recursive validators are exactly the things that eat
days. The rigor moved the *finding* of bugs before the running instead of after.
The eng-plan's hazard analysis caught the reentrant-deadlock surface on paper.
The deterministic self-tests turned "did preemption work?" into a CI line that
either prints or hangs the boot. The adversarial review caught the two DoS
defects while they were latent. Bugs found before the machine runs cost minutes;
bugs found after a silent reboot cost days. Rigor up front simply *beat*
debugging after the fact.

The other surprise: the hardest skill wasn't technical — it was **scope
discipline**, taken over and over as the minimal cut that still teaches the
thing, and then *writing down why*. A custom read-only
filesystem instead of FAT16 (so no `mkfs.fat` build-magic hides how bytes become
a directory). The old PIT timer instead of the local APIC (written down as its own
future milestone so it couldn't leak back in). An `hlt`-and-poll idle loop
instead of a real blocking wait. Every one of those was a temptation to build
something more "real," and every "no" is recorded with its reasoning.

## Where the human still mattered

I don't want to oversell the AI. The clearest lesson about its limits is the
"provably cannot loop" comment above: the model will confidently write a proof
that's *half*-true and read as fully true, because the half it proved is real
and the half it skipped is invisible unless you go looking. It needed the
adversarial second pass to become honest. And left alone, scope creeps — there
is always a plausible next feature, and the model is agreeable about all of
them. Someone has to hold the non-goals like a grudge.

So the judgment stayed human: what *not* to build, when a claim is too strong,
which failure mode actually matters versus which is theater. The AI is a superb
mechanic inside a well-specified frame and an unreliable author of the frame
itself. Knowing which is which is the actual skill.

## Close

Let me be honest about what this is. It's a hobby OS. It boots under QEMU, runs a
preemptive scheduler and a shell, and lets you walk a read-only filesystem tree.
It is emphatically *not* for real use — no networking, no users, no security
hardening, no real hardware beyond the emulator. Those aren't gaps; they're the
non-goals, chosen on purpose and defended every milestone.

The honest success criterion was never the feature list. It was understanding
gained, plus a public trail that includes the dead ends — the boot that would
have panicked on a keystroke, the validator that would have hung, the "proof"
that was only a third of one. Those aren't embarrassments to sand off. They're
the whole value. A highlight reel teaches nobody.

And the best part is that none of this asks for your trust. It's all verifiable.
Read the self-tests that CI asserts on every push. Read the reviews and the
retros. Watch a real recorded session drive the shell over the serial line —
`ps`, `ls`, `cd`, `cat`, the `is a directory` refusal, all live. Find the
`FLAG{ziran-boundary-leak}` planted on the disk — bytes with no directory entry,
unreachable by any `ls` or `cat`. The security milestones (M15/M16) went on to
steal it, and it's now a live **capture-the-flag against the kernel itself**, where
you can too. Or just boot the thing yourself — real QEMU compiled to WebAssembly
runs the actual 64-bit kernel to an interactive shell **in your browser** at
[ziranos.pages.dev](https://ziranos.pages.dev). Type `ls`, `cd docs`,
`cat filesystem.txt`. It's all right there — the mechanisms, and the proof that
they're mechanisms and not magic.

That was the point.

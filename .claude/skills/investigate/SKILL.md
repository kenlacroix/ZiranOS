---
name: investigate
description: Root-cause a kernel bug with disciplined hypothesis testing instead of guess-and-recompile. Use when the kernel triple-faults, hangs, reboots, or misbehaves and the cause isn't obvious. Enforces a stop-and-reassess after 3 failed fixes. Adapted from gstack's /investigate.
---

# investigate

The kernel's defining failure mode: something goes wrong and there is no stack
trace, no log, just a reboot or a hang. Flailing — change something, recompile,
see if it boots — burns days. This skill imposes the discipline that actually
converges: hypothesis, prediction, test, one variable at a time.

Adapted from gstack's `/investigate` (github.com/garrytan/gstack, MIT).

## When to use

Triple fault / silent reboot, a hang, garbage on screen, a fault that fires
"later" than the code that caused it (classic paging), or any "it worked before"
regression.

## First, make the failure legible

Before hypothesizing, get *any* signal. In rough order of leverage:

- `make debug` + `make gdb` — freeze at reset, single-step, read registers.
- QEMU `-d int,cpu_reset` — logs every interrupt/exception and the CPU state at
  a reset. A triple fault becomes a readable sequence of faults.
- `-d int` shows the vector: #DE(0), #PF(14), #GP(13), #DF(8) — the vector alone
  often names the bug class.
- Serial prints bracketing the suspect region — the last line printed bounds
  where it died.

## The loop

1. **State the symptom precisely.** "Resets right after `mov cr0` in
   enable_paging," not "paging is broken."
2. **One hypothesis.** A specific, falsifiable cause. ("CR3 holds a virtual, not
   physical, address.")
3. **Prediction.** If true, what would GDB / the int log show? Go look. Let the
   evidence kill or confirm the hypothesis — don't fix yet.
4. **Change one thing.** Then re-test.
5. **Log the attempt** (what you tried, what happened) so you don't loop.

## The 3-attempt rule

After **three** failed fix attempts on the same bug, STOP. Do not try a fourth
variation. Step back and question a *lower* assumption: is the toolchain/linker
placing things where I think? Is the precondition from `/eng-plan` actually
true? Re-read the relevant Intel SDM / OSDev section. Reproduce the smallest
possible case. Often the bug is one layer beneath where you were looking.

## Output

The root cause in one sentence, the evidence that proved it, and the fix — this
is exactly the material the milestone blog post is built from (`/retro`,
`/document-milestone`). Record the war story; it's the honest part.

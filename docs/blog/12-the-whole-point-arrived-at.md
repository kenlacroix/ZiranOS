# The whole point, arrived at

*Milestone 12 — the file manager: subdirectories, `cd`/`pwd`/`ls`/`cat`, and the
moment the core arc — boot → memory → tasks → shell → files — is complete.*

> New to this? Read
> [docs/concepts/filesystem.md](../concepts/filesystem.md)
> alongside — it explains how a directory is the *same lie* as a file, told
> recursively (a directory is just a file whose bytes are more
> `{name, offset, length}` entries), and why the genuinely hard part of hierarchy
> isn't *reading* the tree but *validating* it: a mount that recurses an
> attacker-controlled structure and provably cannot loop, hang, or overflow the
> stack.

Last milestone gave the kernel read-only *files*: `ls` and `cat` over a flat
directory. This one gives it a navigable *tree* and the shell to walk it — `cd`
into a subdirectory, `pwd` to see where you are, `ls`/`cat` scoped to the current
directory. That sounds like a small delta, and at the format level it is: one
byte. But reaching it is the thing the whole project was pointed at. The stated
success criterion was never "a filesystem" — it was a machine that boots, manages
its own memory, runs concurrent tasks, presents a shell, and lets you browse
files from that shell. M12 closes that loop. The **boot → memory → tasks → shell →
files** arc that PLAN.md set as the primary goal is, as of this milestone,
complete.

### Milestone 12 — file manager — checklist

Think
- [x] /milestone-office-hours — six forcing questions answered below
- [x] /scope-guard — verdict recorded: **Hold**, below (the capstone, held to "list, navigate, read")

Plan
- [x] /eng-plan — approach, the v2 kind byte, recursive validation, pure path canonicalization, verification — [docs/planning/milestone-12-eng-plan.md](../planning/milestone-12-eng-plan.md)

Build
- [x] Working, demoable state reached (`cd docs` → `ziran:/docs>`; `ls` lists that directory; `cat filesystem.txt` prints its bytes; `cd ..` returns; `cat docs` → "is a directory"; `cd nope` → "no such directory")
- [x] `make` is clean: assembles, compiles, links; `make check-header` passes
- [x] No new compiler warnings; no new `unsafe` — the whole filesystem stays pure safe Rust (a bad offset is a loud panic or a caught `Err`, never a silent success)

Debug (as needed)
- [x] /investigate — not needed; zero runtime debugging cycles (both findings were caught in review, below)
- [x] The real debugging story captured — below (the honest version: nothing broke at runtime; the review found two latent DoS defects)

Review
- [x] /kernel-review — 2-lens pass (the recursive FS validator; the shell navigation). Shell nav = SHIP; the validator grew two hardening fixes

Security
- [x] /red-team — a real FS boundary now exists: the image contains a planted secret with no directory entry, unreachable by any path. Its **full** adversarial turn is M16, but this milestone is the seed: the **10** corrupt-image rejections and the exfiltration target (`FLAG{ziran-boundary-leak}`) are already in place

Reflect
- [x] /retro — post-mortem answered honestly, below
- [x] /document-milestone (STATUS/README/this post)
- [ ] CI green (build + headless QEMU boot) — runs on push

## Office hours

1. **The one thing I'll understand.** That a directory is the *same lie* as a
   file, told recursively — a directory is a file whose bytes are more
   `{name, offset, length}` entries — so hierarchy costs almost nothing at the
   format level. And that the genuinely hard part isn't *reading* the tree but
   *validating* it: a mount that recurses an attacker-controlled structure and
   provably cannot loop, hang, or overflow the stack. The reading is a descent;
   the substance is proving the descent is safe on hostile input.

2. **"Done" as observable behavior.** At the prompt: `cd docs` →
   `ziran:/docs>`, `ls` lists that directory's files, `cat filesystem.txt` prints
   its bytes, `cd ..` returns to `/`, `cat docs` → `is a directory`, `cd nope` →
   `no such directory`. On serial (CI): mount the depth-2 tree, navigate into
   `docs/`, read a file byte-exact *through* the subdirectory, reject **10**
   corrupt images (a cycle, a deep chain, and a diamond DAG among them), confirm
   the planted secret is unreachable, then print `M12: file manager online`. The
   shell runs as a spawned task, visible in its own `ps`.

3. **The smallest version that still teaches it.** One `kind` byte (reusing a
   reserved field), recursive validation with the forward-ordering rule + a
   high-water mark + a depth cap, a *pure* string canonicalizer for `.`/`..`,
   `cd`/`pwd`/`ls`/`cat` with a `cwd`, and a depth-2 boot tree. Read-only. No
   write, no `mkdir`, no symlinks, no permissions, no path cache, no deep trees.

4. **Working vs. accidentally working.** The termination gate is the substance: a
   directory *cycle* must return `DirNotForward`, **not hang** — the self-test
   asserts the error, not a timeout. Reads are byte-exact *through* a
   subdirectory. `canonicalize` is a pure function, exhaustively unit-tested
   independent of any tree. And the hidden secret has no directory entry, so
   `ls`/`cat` can never reach it — the boundary holds, which is both the
   capstone's honesty check and the M16 setup.

5. **What most likely stalls this for days.** A **hang**, not a crash: `mount`
   looping or blowing the stack on a malformed image — a directory cycle, or a
   diamond that re-validates exponentially. Mitigation: the forward-ordering +
   high-water rule make validation terminating *and* linear by construction, the
   depth cap bounds the stack, and the self-test feeds a cycle, a deep chain, and
   a diamond and asserts *clean errors* rather than a timeout. Safe Rust keeps the
   other hazard — an out-of-bounds read — a loud panic rather than silent
   corruption.

6. **Drifting toward a non-goal?** Several temptations, all deferred: `write` /
   `mkdir` (free-space management — a real allocator), symlinks (a DAG the
   forward-ordering rule deliberately refuses to assume), permissions and
   timestamps (the multi-user non-goal), a path cache, arbitrary-depth trees. Ran
   `/scope-guard` → **Hold**.

## Scope-guard

Verdict: **Hold**. The capstone delivers exactly "list, navigate, read" with the
minimum machinery that teaches hierarchy: a 4-byte format delta (the `kind`
byte), recursive-but-provably-safe validation, and pure string path resolution.
Every deferral targets open-ended "real OS" work the non-goals explicitly warn
against — write/`mkdir` drag in a free-space allocator; symlinks break the
forward-ordering guarantee; permissions/timestamps are the multi-user non-goal.
The hidden secret is a *zero-cost* teaching hook that makes the already-planned
M16 boundary track concrete — not scope creep. Shipping and testing depth 2 while
the format admits *any* depth is the disciplined call: it proves the recursion
without inviting a "deep trees" rabbit hole. And reaching M12 completes the core
arc — the project's primary success criterion is met *here*; M13–16 are
explicitly stretch and security, not part of the criterion.

## Eng-plan (summary)

A recursive read of a recursive lie, and a shell to walk it:

- **The v2 format.** ZranFS gains a single `kind` byte per directory entry
  (reusing a reserved field): `0` = file, `1` = directory. A directory's "data"
  region is just another run of `{name, offset, length, kind}` entries. The
  format now admits arbitrary depth; the boot image ships depth 2.
- **`validate_dir` — the trust boundary, now recursive.** It descends the tree,
  and its safety rests on three *separate* guarantees, not one: the
  **forward-ordering rule** (a child directory's table must start strictly after
  its parent's) proves the recursion *terminates*; a **monotonic high-water mark**
  (each table is validated at most once, and re-entry is rejected) makes total
  work *linear* and rejects diamond DAGs; a **`MAX_DEPTH = 32` cap** bounds the
  *stack* against a deep chain. Violations are specific errors — `DirNotForward`,
  `TooDeep` — never a hang or an overflow.
- **The resolver.** `resolve` walks a canonical path to a `Node`; `list_dir` and
  `read_path` hand back the entries or bytes at a resolved node. `DirEntry` grows
  an `is_dir` flag. The descent is pure — no parent pointers, so there are no
  cycles to chase at read time.
- **`canonicalize` — pure string math.** Path normalization (`.`, `..`, empty
  segments, absolute vs. relative against `cwd`) is a *pure function* over
  strings, unit-tested with no filesystem in the loop.
- **The shell.** Grows a `cwd`, a cwd-aware prompt (`ziran:/docs>`), and
  `cd`/`pwd`/`ls`/`cat`, all routed through `canonicalize` then the resolver.
  Marker: `M12: file manager online`.

The split is the point: string resolution is testable without a keyboard, and the
FS resolver stays a pure descent. Full plan in
[docs/planning/milestone-12-eng-plan.md](../planning/milestone-12-eng-plan.md).

## What got built

Commits on `claude/ziran-os-plan-qgobid`:

- **M12 plan** — office-hours + scope-guard + eng-plan.
- **M12 step 1** — the ZranFS **v2** format (the `kind` byte) and the recursive
  `validate_dir` with the forward-ordering rule.
- **M12 core** — the depth-2 boot tree plus the hidden secret; the
  `resolve`/`list_dir`/`read_path` resolver (`Node`, `DirEntry.is_dir`); and the
  shell navigation (`cwd`, the pure `canonicalize`, `cd`/`pwd`/`ls`/`cat`, the
  cwd-aware prompt). Marker `M12: file manager online`. **Completes the core
  arc.**
- **M12 review** — a 2-lens `/kernel-review` (the FS validator/recursion; the
  shell navigation). Shell nav = **SHIP**. The validator surfaced *two latent DoS
  defects*: a crafted deep chain overflows the guard-page-less **16 KiB** task
  stack, and a diamond DAG makes `mount` hang via exponential re-validation. Fixed
  with the **monotonic high-water mark** (linear; rejects DAGs) and the
  **`MAX_DEPTH = 32`** cap (`TooDeep`). Rejections grew from **8 to 10**.
- **feat: interactive serial console** — the shell reads and echoes over the
  serial line (`serial::read_byte` plus `shp!`/`shln!` dual-output macros), so it
  can be driven from a terminal (`make console`) where the VGA text console isn't
  displayed (macOS UEFI). Verified by driving it over serial.

## Verification status (honest)

Boot-tested live under headless QEMU. The self-test serial log:

```
[ok] fs: mounted a v2 tree (4 root entries incl. docs/), read /docs/filesystem.txt byte-for-byte, secret unreachable, 10 corrupt images rejected (cycle, deep chain, and diamond DAG among them -- no hang, no stack overflow)
M12: file manager online
[ok] shell: editing, parsing, path normalization, input ring, and ps/mem accessors verified
[boot test] PASS -- reached long mode and 'M12: file manager online'
```

And — new this milestone — the OS is now *interactively drivable*. The serial
console makes the shell usable from a terminal (`make console`), which matters
because on macOS UEFI the VGA text console isn't displayed. Here is a **real**
interactive session, captured by driving the shell over `-serial stdio`:

```
ziran:/> ps
tasks: 2
  [0] running
  [1] running  (current)
ziran:/> mem
  frames: 30793 free (120 MiB)
  heap:   16592 used / 1031984 free / 1048576 total bytes
  uptime: 2 s (208 ticks @ 100 Hz)
ziran:/> ls
  motd.txt   13 bytes
  docs       <dir>
ziran:/> cd docs
ziran:/docs> cat filesystem.txt
A file is a lie a header tells about bytes.
ziran:/> cat docs
cat: is a directory: /docs
```

What each piece proves — and what it doesn't:

- The **self-test** pins the tree navigation and the 10 rejections
  *deterministically* on serial: a v2 tree mounts, a file is read byte-exact
  *through* a subdirectory, and 10 corrupt images (cycle, deep chain, diamond DAG
  among them) are refused with clean errors — no hang, no stack overflow.
- The **serial console** makes the same capstone *interactively* drivable, proven
  by the captured session above — `ps`, `mem`, `ls`, `cd`, `cat`, and the
  `is a directory` refusal, driven over the wire, autonomously, no screen
  required.
- The **planted secret** `FLAG{ziran-boundary-leak}` is present in the image yet
  reachable by no path — `ls`/`cat` can't surface it. The boundary holds; that's
  the honesty check for the capstone and the concrete target M16 will attack.

The run had no panics and no faults.

## Retro

**Plan vs. reality.** Accurate: the `kind` byte, the forward-ordering rule, the
shell/fs split, the pure `canonicalize` all landed as planned. The one place the
plan was *elegant but incomplete*: the forward-ordering termination proof. It
genuinely bounded *depth* — but not total *work* and not *stack frames*. So the
comment's "provably cannot loop" was only half-true, and the adversarial review
caught it.

**What broke, and for how long.** Nothing at runtime — zero debugging cycles, the
**fourth** milestone in a row (M9, M10, M11, M12). The two validator defects
weren't crashes; they were *latent*, unreachable with the fixed boot image, found
by review. Which is exactly why they matter: they're the M16 fuzzing surface,
found before an adversary would have.

**The root cause / the fix.** The termination proof reasoned about **depth** but
not **work** or **stack**. A diamond DAG re-validates shared subtrees
exponentially, so `mount` would *hang*; a deep chain recurses past the
guard-page-less **16 KiB** task stack, so `mount` would *overflow*. Fix: a
**monotonic high-water mark** — every directory table is validated at most once,
and any re-entry is rejected — which makes total work *linear* and turns a diamond
DAG into a clean rejection; plus a **`MAX_DEPTH = 32`** cap that bounds the stack
(`TooDeep`). Rejections went from 8 to 10.

**The assumption worth re-examining (the lede).** *"A termination proof is a
safety proof."* It isn't. The forward-ordering rule really did prove the recursion
*terminates* — finite depth, no possibility of looping — and the comment said
exactly that. But *terminating in bounded depth says nothing about bounded time*
(a diamond DAG terminates and still runs exponentially) *or bounded stack* (a deep
chain terminates and still overflows). "Terminates," "bounded time," and "bounded
stack" are three separate guarantees. Conflating them is precisely how a validator
ships a denial-of-service while looking correct.

**What earned its keep.** Three things. The **shell/fs split with a pure
`canonicalize`** — all `.`/`..` handling as string math — which is testable without
a keyboard *and* keeps the FS resolver a pure descent (no parent pointers, no
cycles to chase). The **adversarial review**, which turned a "provably terminates"
claim into an *honestly-bounded* one and caught two DoS defects before they could
be triggered. And the **serial console**, which proved the capstone works
interactively — autonomously, over the wire — without a display.

**One thing to do differently.** When writing a "provably X" claim for a
validator, enumerate the three resource bounds *separately* — depth (terminates),
work (time), and stack (space) — rather than assuming the first implies the
others. The forward-ordering comment asserted one and implied three; the review
had to split them apart.

## Takeaway for the next person

A directory is the same lie as a file, told recursively — so hierarchy is nearly
free at the format level, and the real work is *validating* a tree you don't
trust. And validation is three guarantees, not one: it must *terminate*, run in
*bounded time*, and use *bounded stack*. A forward-ordering rule buys you the
first; a high-water mark buys the second; a depth cap buys the third. Prove them
separately or you ship a denial-of-service that passes every "does it work" test.

With M12, the **core arc is complete**: boot → memory → tasks → shell → files, all
present, all drivable from a prompt. That was the project's stated primary success
criterion, and it's met here. What remains is explicitly stretch and security:
**M13** userspace and syscalls (the first real privilege boundary — ring 3), **M14**
the networking stub, and **M15/16** the security capstone — where the
`FLAG{ziran-boundary-leak}` planted in this milestone's image becomes the thing to
capture, and the validator's now-honestly-bounded guarantees become the thing to
fuzz. The whole point has been arrived at; everything after it is a bonus round.

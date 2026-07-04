# Ziran OS — Project Plan
*Working title: **Ziran** (自然 — "self-so," that which arises naturally without external forcing). Swap freely — search/replace "Ziran" throughout.*

---

## 1. Vision

A hobbyist, from-scratch operating system built to understand what's actually happening beneath every abstraction layer currently being trusted on faith. Not intended for real-world use, adoption, or production deployment. Success is defined by:

- Depth of understanding gained
- A working, bootable system reaching at least a simple file manager
- A public trail of blog posts documenting the process, including the parts that didn't work
- **A teaching tool** — the OS boots in the browser next to a guided, interactive
  tutorial, so anyone (starting with the author) can *learn each layer while it
  runs*, not just read about it (see §3)
- A body of work that sits credibly next to app-level projects (MoodHaven, StillHaven, Steward) as evidence of range — from PWA down to bare metal

**Explicit non-goals:** POSIX compliance, real hardware support beyond QEMU/common virtualization, networking stack (beyond a stretch-goal stub), *production* security hardening, multi-user support, or anyone besides the author actually using it.

> **A deliberate refinement (added later):** *production security hardening*
> stays a non-goal — this OS will never be defensible against a real adversary,
> and that's fine. But **adversarial self-testing is now an explicit learning
> track**: once there's an actual privilege boundary or on-disk data to protect
> (userspace/syscalls, a filesystem), attacking your *own* kernel — trying to
> escape ring 3, read memory you shouldn't, corrupt the syscall boundary — is
> one of the best ways to understand what those boundaries really are. Breaking
> it to learn is in scope; hardening it for the world is not. See §9.

## 2. Why This Project

The throughline across current work is sovereignty and reduced surface area for trust — privacy-first local apps, self-hosted homelab, Rust chosen partly for its lack of hidden runtime. An OS is the most literal expression of that: no OS underneath, no abstractions that weren't personally chosen. It's also structurally identical to the somatic practice narrative already in use — self-led, self-taught, no formal guidance, built from first principles rather than a course or mentor. Different domain, same pattern.

## 3. Can It Run in a Browser AND a VM?

**Yes, both, and they serve different purposes:**

| Environment | Tool | Purpose |
|---|---|---|
| Primary dev/debug loop | **QEMU** (+ GDB for stepping through crashes) | Fast iteration, real interrupts/registers visible, scriptable |
| Secondary debug (deep register-level) | **Bochs** | Slower but more introspectable when QEMU's abstraction hides something |
| Public-facing / blog embed | **v86** (JS/WASM x86 emulator — see copy.sh/v86) | Boots the *same* image, no porting required, runs client-side in any browser tab |

The browser path is not a separate build target — it's the same multiboot-compatible kernel image, loaded into a different emulator. This means blog posts can eventually embed a literal "click to boot" demo. That's a strong differentiator for a hobbyist OS blog — most never get an interactive artifact, they get screenshots.

### 3a. The browser demo is really a *teaching tool* (elevated)

The most honest reconciliation of "I don't want AI to just build this and hand me
a finished thing" is to make the public artifact itself a **learning
instrument**. So the v86 embed is promoted from a novelty to a first-class track
(`web/index.html`): the real kernel boots beside a guided tour that works on a
**predict → observe → explain** loop — the reader guesses what a boot line means,
watches it happen in the live machine, then gets the explanation with links to
the *actual source file* and the from-first-principles concept docs
(`docs/concepts/`).

Design principles for the teaching tool:
- **The real machine is the hero.** It boots the actual image, not a video.
- **Predict before reveal.** Every concept starts as a question, so the learner
  does the thinking first — the whole point.
- **Every claim links to real code.** Nothing is asserted that you can't go read.
- **It grows with the OS.** Each milestone adds its output line and a tour step —
  this is part of the milestone checklist (`docs/MILESTONE_CHECKLIST.md`).
- **Dual-mode so it never breaks.** Live v86 when the assets are reachable; a
  faithful simulated replay of the same boot otherwise.

### 3b. Teaching-tool roadmap — from watching to acting

The tool is most useful when the reader *does the thinking and then acts on the
real machine* — predicting, running, and especially **breaking** it — rather than
reading a narrated replay. Captured ideas, tagged by what unblocks them; pull the
relevant ones into each milestone's checklist as it fits.

**Now (no new kernel features needed):**
- **"Break it" experiments.** Per step, a predict → reveal of a *deliberately
  broken* variant: a `switch_context` missing a callee-saved `pop`; a timer
  handler that skips the EOI → one tick, then silence. Seeing the triple-fault or
  hang that a concept doc only *describes* is the single most instructive thing a
  live emulator can do that a blog cannot — and it doubles as the security track's
  "observe the failure mode" habit. *(Prototyped for M9: the missing-EOI hang.)*
- **Register/state peeks.** Show the concrete value behind an abstraction — e.g.
  `rsp = 0x40008ff0`, 16-aligned, at the M9 context switch.
- **Inline source.** Embed the specific lines under discussion (with the `SAFETY`
  comment visible), closing the "every claim links to real code" loop without
  making the reader leave the page.

**M10+ (needs the shell):**
- **Type into the live machine.** Once the shell lands, let the reader run `help`,
  `mem`, `ps` in the v86 console; the step becomes "predict what `ps` shows, then
  run it." The M9 payoff specifically: a `ps`-style view of the two tasks plus a
  live tick counter makes preemption *visible* rather than asserted in a log.

**Ongoing (durability as it grows toward 16 milestones):**
- Per-milestone deep links + a "you are here" spine tied to the `ROAD` array, so a
  reader arriving from a blog post lands on the right step.
- Guard the dual-mode fallback: keep the simulated replay byte-faithful to the
  real boot each milestone, so it does not silently rot.
- Surface each milestone's "what broke" thread in the tour — the honest,
  differentiated content the project values.

**Non-goal reminder:** none of this needs networking. The security/red-team track
(§8) attacks *local* boundaries only; a "pentest" angle in the tool means letting
the reader break the kernel's own invariants (ring-3 escape, syscall/FS abuse from
M13 / M11–12), never reach the network.

## 4. Core Technical Decisions

- **Language:** Rust, `no_std`, targeting `x86_64-unknown-none` (the Tier-2 stable bare-metal target; a custom target JSON is an option later if a milestone needs `build-std`).
- **Bootloader — decided:** *hand-write the CPU-mode transitions, let GRUB handle the real-mode dance.* The image carries a hand-written **Multiboot2** header and does its own 32-bit → long-mode transition in assembly (`boot/`). GRUB loads it. This keeps the genuinely instructive part — CPU checks, page-table setup, the paging/EFER/CR0 sequence that *is* entering long mode — hand-written and understood, while not gating every later milestone on debugging a raw stage-1/stage-2 boot sector. The raw-boot-sector version remains a fair future exercise / blog post; it is explicitly **not** a prerequisite. Decision made — not to be relitigated mid-project (see §6).
- **Build tooling:** `cargo`, `nasm`, `ld`, `qemu-system-x86_64`, `grub-mkrescue`, `gdb`. The pipeline is spelled out explicitly in the `Makefile` rather than hidden behind a build tool.
- **Version control:** standard git repo, public on GitHub from day one (even pre-boot) so the commit history *is* part of the story.
- **CI:** GitHub Actions assembling, compiling, linking, and **booting** the image under headless QEMU on every push — a "still boots" signal from the first bootable milestone onward.

## 5. Milestone Roadmap

Each milestone = one working, demoable state + one blog post. Live status is tracked in `STATUS.md`.

| # | Milestone | What "done" looks like | Blog angle |
|---|---|---|---|
| 0 | Toolchain & environment | QEMU, Rust, target spec, and repo scaffolding all working; empty kernel compiles | "Setting up to build nothing" |
| 1 | Bootloader | Multiboot header found, CPU checks pass, transition begins in protected mode | "The first 512 bytes" |
| 2 | Long mode + kernel entry | Jump into 64-bit long mode, hand off to a Rust `no_std` kernel entry point | "Handing off to Rust" |
| 3 | VGA text output | Kernel prints formatted text to screen without BIOS | "Making it talk back" |
| 4 | GDT / IDT / interrupts | Exception handlers installed; catch a divide-by-zero, page fault, etc. without a crash-reboot | "Teaching it to fail gracefully" |
| 5 | Keyboard input (PS/2) | Kernel reads keystrokes, echoes to screen | "It listens now" |
| 6 | Physical memory management | Frame allocator tracks usable RAM regions (via the multiboot memory map) | "Learning what memory it has" |
| 7 | Paging / virtual memory | Kernel manages its own page tables, can map/unmap pages | "Building the illusion of memory" |
| 8 | Heap allocator | `alloc` works — `Vec`, `Box`, `String` usable in the kernel | "Getting `Vec` to work felt bigger than it should" |
| 9 | Timer + scheduling | PIT/APIC timer drives a cooperative, then preemptive, scheduler with 2+ tasks | "Multiple things happening at once, sort of" |
| 10 | Simple shell | Interactive command line: `help`, `echo`, `clear`, `mem`, `ps` | "It has a prompt now" |
| 11 | Filesystem | Read support for a simple format (FAT16, or a minimal custom FS on a RAM disk) | "Files are just convincing lies about disk layout" |
| 12 | **File manager** | Shell commands to list, navigate, read, and (optionally) write files — the stated end goal | "The whole point, arrived at" |
| 13 (stretch) | Userspace / syscalls | Basic ring 3 separation, a minimal syscall interface | stretch post |
| 14 (stretch) | Networking stub | Loopback or a trivial virtio-net driver, "hello" over a socket | stretch post |
| 15 (security) | Break the privilege boundary | From ring 3, deliberately attempt to read kernel memory / execute privileged instructions / pass bad syscall args — and watch the CPU + kernel stop you (or find where they don't) | "Trying to break out of my own jail" |
| 16 (security) | Break the filesystem boundary | Craft inputs that make the FS read/write outside a file's bounds; fuzz the parser; try to reach data a caller shouldn't | "Attacking the lies about disk layout" |
| — | Teaching tool (`web/`) | v86 embed + guided predict→observe→explain tour; grows one step per milestone | "You can boot it right here — and learn how it works" |

**Realistic pacing (hobbyist, part-time):** Milestones 0–3 are a focused weekend-to-two-weeks. Milestones 4–9 are the long middle — months of intermittent work, with stretches of no visible progress while a single bug is chased. Milestones 10–12 move faster once memory and interrupts are solid.

## 6. Risks / Known Hard Parts

- **Debugging is opaque by default** — a bad kernel triple-faults and the machine just reboots with no stack trace. `make debug` (GDB stub) and QEMU's `-d int,cpu_reset` mitigate this, but it remains the single biggest source of stalled momentum. Normalise it in the blog rather than hiding it.
- **Paging bugs are silent and delayed** — a bad page-table entry might not fault until much later, making cause and effect hard to connect. Budget extra time.
- **Scope creep toward "real" features** — the temptation to add networking, real filesystems, or POSIX behavior. The non-goals list exists to guard against this.
- **Bootloader hand-write vs. use-existing** — decided in §4. Do not relitigate mid-project.

## 7. Open Discussion Points

- Any recommended resources beyond the usual (OSDev wiki, *Writing an OS in Rust*, Intel SDM) worth prioritizing early.
- ~~Whether FAT16 read support or a custom minimal filesystem is the better learning trade for milestone 11.~~ **Resolved (M11): a custom minimal read-only filesystem (ZranFS) on an in-kernel RAM disk.** A valid FAT16 image realistically needs `mkfs.fat` — a host-tool dependency and an opaque prebuilt blob, exactly the "build magic that hides how bytes become data" §4 forbids; the custom format builds its disk in readable Rust and teaches the "file = header's lie about flat bytes" lesson without FAT's accidental complexity. FAT16 read is deferred as a genuinely valuable future post ("reading a format the world actually uses"), not rejected. Decided — not to be relitigated (see §6). Reasoning in `docs/planning/milestone-11-eng-plan.md`.
- Gotchas specific to running v86 against a real multiboot image (vs. its usual Linux/DOS demo images).

## 8. Security & Adversarial Self-Testing (learning track)

A parallel track, not a phase — it only becomes possible *after* there's
something to attack, and it's about understanding, not defense.

**When it starts.** There's no meaningful attack surface until the kernel has a
**privilege boundary** (ring-3 userspace + a syscall interface, milestone 13) or
**persistent data** (a filesystem, milestones 11–12). Before that, everything
runs in ring 0 with full power — there's nothing to "break out of." So the
security milestones (15–16) sit after those.

**What it looks like.** All of this is running the author's own code on the
author's own hardware/VM — authorized testing of a system you fully own:

- **Privilege escape attempts.** From a ring-3 process, try the things that
  *should* fault: execute a privileged instruction (`cli`, `hlt`, writing a
  control register), read a kernel-only page, jump into kernel code directly.
  Each one is a lesson in exactly what the ring boundary and page permissions
  enforce — and a chance to find a gap where they don't.
- **Syscall boundary abuse.** Pass out-of-range pointers, unmapped addresses,
  huge lengths, and malformed arguments across the syscall interface. The kernel
  must validate everything crossing the boundary; fuzzing it finds the checks you
  forgot. (This is the classic "confused deputy" surface.)
- **Filesystem/parser fuzzing.** Feed the FS deliberately corrupt structures and
  boundary-case inputs; try to read or write outside a file's extent.

**The mindset (see the `red-team` skill).** Think in terms of the boundary being
tested, the specific invariant an attack would violate, and what observable proof
(a fault caught, or data leaked) settles whether the boundary holds. Every
finding — including "the boundary held, here's why" — is blog material and a real
lesson in what these mechanisms actually do.

**Still not a goal:** making the OS *withstand* a determined attacker. We break
it to learn where the edges are, then write down what we found.

## 9. Blog / Narrative Notes

- Employer-agnostic, people-agnostic — same rule as other blog content.
- Each milestone post includes: what broke, how long it took to find, and the actual fix — the debugging story is the more honest and more interesting post, not just "it works now."
- The "self-led, self-discovered" thread ties directly to the existing somatic-practice narrative — worth an explicit cross-reference post once there's enough OS material (e.g., "Two things I built from scratch with no one watching").

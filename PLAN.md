# Ziran OS — Project Plan
*Working title: **Ziran** (自然 — "self-so," that which arises naturally without external forcing). Swap freely — search/replace "Ziran" throughout.*

---

## 1. Vision

A hobbyist, from-scratch operating system built to understand what's actually happening beneath every abstraction layer currently being trusted on faith. Not intended for real-world use, adoption, or production deployment. Success is defined by:

- Depth of understanding gained
- A working, bootable system reaching at least a simple file manager
- A public trail of blog posts documenting the process, including the parts that didn't work
- A body of work that sits credibly next to app-level projects (MoodHaven, StillHaven, Steward) as evidence of range — from PWA down to bare metal

**Explicit non-goals:** POSIX compliance, real hardware support beyond QEMU/common virtualization, networking stack (beyond a stretch-goal stub), security hardening, multi-user support, or anyone besides the author actually using it.

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
| — | Browser demo | v86 embed on the blog, loading the actual kernel image | "You can boot it right here" |

**Realistic pacing (hobbyist, part-time):** Milestones 0–3 are a focused weekend-to-two-weeks. Milestones 4–9 are the long middle — months of intermittent work, with stretches of no visible progress while a single bug is chased. Milestones 10–12 move faster once memory and interrupts are solid.

## 6. Risks / Known Hard Parts

- **Debugging is opaque by default** — a bad kernel triple-faults and the machine just reboots with no stack trace. `make debug` (GDB stub) and QEMU's `-d int,cpu_reset` mitigate this, but it remains the single biggest source of stalled momentum. Normalise it in the blog rather than hiding it.
- **Paging bugs are silent and delayed** — a bad page-table entry might not fault until much later, making cause and effect hard to connect. Budget extra time.
- **Scope creep toward "real" features** — the temptation to add networking, real filesystems, or POSIX behavior. The non-goals list exists to guard against this.
- **Bootloader hand-write vs. use-existing** — decided in §4. Do not relitigate mid-project.

## 7. Open Discussion Points

- Any recommended resources beyond the usual (OSDev wiki, *Writing an OS in Rust*, Intel SDM) worth prioritizing early.
- Whether FAT16 read support or a custom minimal filesystem is the better learning trade for milestone 11.
- Gotchas specific to running v86 against a real multiboot image (vs. its usual Linux/DOS demo images).

## 8. Blog / Narrative Notes

- Employer-agnostic, people-agnostic — same rule as other blog content.
- Each milestone post includes: what broke, how long it took to find, and the actual fix — the debugging story is the more honest and more interesting post, not just "it works now."
- The "self-led, self-discovered" thread ties directly to the existing somatic-practice narrative — worth an explicit cross-reference post once there's enough OS material (e.g., "Two things I built from scratch with no one watching").

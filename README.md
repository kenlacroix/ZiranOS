# Ziran OS

> **自然 (zìrán)** — "self-so": that which arises naturally, without external forcing.

A hobbyist, from-scratch x86_64 operating system, written to understand what is
actually happening beneath every abstraction usually trusted on faith. There is
no OS underneath it and no abstraction in it that wasn't deliberately chosen.

It is **not** meant for real-world use. Success is measured in understanding
gained, in a system that boots and eventually runs a simple file manager, and in
an honest public trail of how it was built — including the parts that didn't
work.

See **[PLAN.md](PLAN.md)** for the full vision, technical decisions, and
milestone roadmap, and **[STATUS.md](STATUS.md)** for where things actually
stand right now.

---

## Why this exists — and why it isn't AI slop

**What I set out to learn** wasn't "how to build an OS." It was what is actually
happening beneath every abstraction I'd been trusting on faith — and the way to
learn that is to turn each abstraction into a mechanism you can hold in your head:

- a **task** is just a saved stack pointer; switching is swapping `rsp` so a
  `ret` resumes someone else's call stack.
- a **file** is a header's lie about a flat run of bytes; a **directory** is the
  same lie, one level down.
- **preemption** is the exact switch a task would make voluntarily — forced by a
  timer it can't refuse.
- and the harder, transferable ones: *a termination proof is not a safety proof*;
  never allocate inside an interrupt-guarded lock; a trust boundary validates
  everything or it isn't one.

**Isn't this just AI slop?** It's built with AI, openly (see
[CLAUDE.md](CLAUDE.md)) — and that's the interesting part, not something to hide.
Slop is unverified, unreviewed, undocumented output. This is the opposite, and
you can check it:

- **Verified.** Every milestone ships a deterministic self-test that runs on boot
  and is asserted by CI — not "looks fine" but *proves* specific properties: a
  freed frame is reused, a file's exact bytes round-trip, **ten** malformed disk
  images are rejected (a directory *cycle* among them, without hanging).
- **Reviewed.** Every milestone got an adversarial, multi-agent code review that
  found *real* latent bugs before they shipped — a deadlock that only triggers
  when the timer preempts mid-lock; a filesystem validator a crafted image could
  hang or stack-overflow. The fixes are in the commit history and the blog.
- **Honest.** Every post documents what broke, what was deliberately deferred, and
  the checks left un-done — the opposite of a highlight reel.

The claim isn't "I hand-typed an operating system." It's: **this is what
disciplined AI-assisted systems engineering looks like** — the human owns the
vision, the [non-goals](PLAN.md), and a mandatory *Think → Plan → Build → Review →
Reflect* loop; the AI does the mechanical work inside that harness; and the whole
trail is here to prove the rigor. Slop is what you get when you skip the harness.
This is the harness.

---

## Current status

The kernel boots via a **hand-written Multiboot2 + long-mode transition** in
assembly, hands off to a `no_std` Rust kernel that prints to the screen, and
installs interrupt handlers so a fault is reported instead of silently rebooting
(it catches a deliberate breakpoint and keeps running), takes keyboard input
over the PS/2 controller — typing echoes to the screen, interrupt-driven — and
reads the Multiboot2 memory map to stand up a bitmap **physical-frame allocator**
over usable RAM (alloc, free, and reclaim), builds its own **page tables** —
switching `CR3` off the boot map to virtual memory it controls (`map`/`translate`/
`unmap`) — stands up a **heap** so Rust's `Vec`, `Box`, and `String` work, and
drives a **preemptive scheduler** — a 100 Hz PIT timer interrupt hands the CPU
between tasks, so two tasks that never yield are still interleaved (a task is just
a saved stack pointer, switched by hand-written assembly) — and now runs an
interactive **shell** as one of those tasks: the keyboard interrupt drops
keystrokes into a lock-free ring, and the shell drains it to run `help`, `echo`,
`clear`, `mem`, and `ps`. It grew a read-only **filesystem** — a minimal custom
format on an in-kernel RAM disk, where a "file" is a header's lie about a flat run
of bytes, and a *directory* is the same lie told recursively — so the shell is a
**file manager** you navigate with `cd`, `pwd`, `ls`, and `cat`. That completes
the core arc (boot → memory → tasks → shell → files) and covers milestones 1–12 —
the project's primary goal, a working system that reaches a simple file manager.
Everything assembles, compiles, and links on stable Rust; CI boots the image under
headless QEMU on every push. **Drive it yourself:** `make console` puts the shell
on your terminal over the serial line.

New here? Start with the plain-language explainers in
[`docs/concepts/`](docs/concepts/) — e.g. [interrupts](docs/concepts/interrupts.md),
written from first principles alongside the code. Or explore it in your browser:
the **[`web/`](web/) teaching tool** walks a guided predict → observe → explain
tour over milestones M0–M12 (with an ELI5 toggle for plain-language explanations,
and the "why this exists / not AI slop" manifesto up front). It comes in three
levels of realness, each labelled honestly:

- **the tour** replays a *faithful reconstruction* — every line on that green
  screen is exactly what the real kernel prints, character for character (a
  64-bit kernel can't run in the lightweight browser emulators, so this is a
  replay, not live);
- **[a real recorded session](web/replay.html)** — the actual kernel captured
  byte-for-byte over the serial line (`make console`), replayed with real timing;
- **a genuine live boot** — real QEMU compiled to WebAssembly
  (`web/build-qemu-wasm.sh`) booting the actual 64-bit kernel (via `-kernel`) in the
  tab. This **works** — verified booting to an interactive `ziran:/>` shell in a
  browser, both by hand and by an automated headless-Chrome test (`make web-test`),
  and it flushed out two latent boot bugs along the way. The "Boot the real kernel"
  button is live; the ~17 MB of assets just aren't publicly **hosted** yet — run it
  now with `make serve` (or self-host).

Hosting is written up in [`docs/DEPLOY.md`](docs/DEPLOY.md) (Cloudflare Pages; with
the `.data` now trimmed to ~0.5 MB the whole thing fits under Pages' 25 MB/file cap,
so R2 is optional).

| Range | State |
|------|-------|
| M0 Toolchain & scaffolding | ✅ done |
| M1 Bootloader (Multiboot2, 32-bit entry) | ✅ done |
| M2 Long mode + Rust entry | ✅ done |
| M3 VGA text output | ✅ done |
| M4 GDT/IDT/interrupts | ✅ done |
| M5 keyboard input (PS/2) | ✅ done |
| M6 physical memory (frame allocator) | ✅ done |
| M7 paging / virtual memory | ✅ done |
| M8 heap (`Vec`, `Box`, `String`) | ✅ done |
| M9 timer + preemptive scheduling | ✅ done |
| M10 interactive shell | ✅ done |
| M11 filesystem (read) — `ls`/`cat` | ✅ done |
| M12 **file manager** (end goal) — `cd`/`pwd`/`ls`/`cat`, subdirectories | ✅ **done** |
| M13 userspace / ring 3 / syscalls — the first privilege boundary | ✅ **done** |
| M15 break the privilege boundary (security) — flag capture, confused-deputy syscall | ✅ **done** |
| M14 networking stub · M16 break the filesystem boundary (security) | ⬜ stretch |

## What happens when it boots

```
GRUB (Multiboot2)
  → boot/boot.asm            32-bit: verify CPU, build page tables, enter long mode
  → boot/long_mode_init.asm  64-bit: load segments, call into Rust
  → kernel_main (src/lib.rs) banner, then bring up memory, the heap, the timer,
                             a scheduler, and a RAM-disk filesystem tree; drop a
                             program into ring 3 that calls back via `int 0x80`
                             (M13); plant a FLAG on a kernel-only page and let ring 3
                             attack it — a direct read faults, a syscall without
                             copy-from-user leaks it, the validated syscall contains
                             it (M15); then spawn a shell task and idle — a `ziran:/>`
                             prompt you navigate (`help`, `ps`, `mem`, `cd`, `pwd`,
                             `ls`, `cat`)
```

Every step is readable and hand-written; there is no `bootimage`/`build.rs`
magic hiding the handoff. The one deliberate convenience is letting GRUB do the
real-mode dance so the interesting kernel work isn't gated on debugging a raw
boot sector — the trade-off is argued in [PLAN.md §4](PLAN.md).

## Repository layout

```
boot/                   hand-written boot assembly
  multiboot_header.asm    the Multiboot2 header GRUB looks for
  boot.asm                32-bit entry: CPU checks, page tables, long-mode switch
  long_mode_init.asm      64-bit entry: segment setup, call kernel_main
  isr.asm                 256 interrupt entry stubs + the shared trampoline
  switch.asm              the context switch (switch_context) + task trampoline
src/                    the no_std Rust kernel
  lib.rs                  kernel_main, panic handler, halt loop
  vga_buffer.rs           VGA text-mode writer + println! macros
  serial.rs               16550 UART (COM1) writer — what CI reads back
  interrupts.rs           the IDT, the interrupt dispatcher, IF helpers
  port.rs                 shared port-mapped I/O (inb/outb)
  pic.rs                  8259 PIC: remap, mask, end-of-interrupt
  keyboard.rs             PS/2 scancode -> character translation
  multiboot.rs            parse the Multiboot2 memory map GRUB hands us
  frame_allocator.rs      bitmap physical-frame allocator over usable RAM
  paging.rs               kernel-built 4-level page tables (map/translate/unmap)
  heap.rs                 the free-list heap behind #[global_allocator]
  pit.rs                  8254 PIT: the periodic 100 Hz timer (IRQ0)
  task.rs                 tasks, the context switch, the round-robin scheduler
  shell.rs                the interactive shell, run as a scheduler task
  fs.rs                   a read-only filesystem (ZranFS) over a RAM disk
linker.ld               places the Multiboot header first, kernel at 1 MiB
grub/grub.cfg           one-entry GRUB menu for the bootable ISO
Makefile                the whole build/run/debug pipeline, spelled out
.github/workflows/ci.yml build + headless boot smoke test on every push
CLAUDE.md               the per-milestone workflow (adapted from gstack)
web/                    browser teaching tool: guided tour + real-session replay + live boot (qemu-wasm)
docs/concepts/          plain-language explainers, written to teach
docs/blog/              the narrative, one post per milestone
docs/DEPLOY.md          how the web tool is hosted (Cloudflare Pages + R2)
docs/MILESTONE_CHECKLIST.md  the loop every milestone runs
```

## Building and running

Requires: a Rust toolchain (pinned to stable in `rust-toolchain.toml`), `nasm`,
`ld`, and for booting `qemu-system-x86_64`, `grub-mkrescue`, and `xorriso`.

**Fastest path — one command:**

```sh
./run              # build it and drop into the interactive shell on this terminal
```

`./run` auto-detects the platform toolchain (no flags to remember), checks you
have the tools (and tells you what to `brew install` if not), then boots the
kernel with its shell wired to your terminal. Type `help`, `ps`, `mem`, `ls`,
`cd docs`, `cat filesystem.txt`; quit with `Ctrl-A` then `X`.

The individual steps, if you want them:

```sh
rustup target add x86_64-unknown-none   # one-time; also in rust-toolchain.toml

make               # assemble + compile + link  -> build/kernel.bin
make check-header  # sanity-check the Multiboot2 magic
make iso           # build a bootable ISO         -> build/ziran.iso
make run           # boot it in a QEMU window
make console       # boot with the shell on THIS terminal (serial) — type at ziran:/>
make run-headless  # boot with no window; pass iff the long-mode marker appears
```

### Debugging a crash

A bad kernel triple-faults and the machine just reboots with no stack trace —
the single biggest source of stalled momentum in OS work. Two mitigations:

```sh
make debug   # boots QEMU frozen, with a GDB stub on :1234
make gdb     # in another terminal: attach GDB to it
```

## Notes on this environment

The image is verified here to **assemble, compile, and link** into a valid
Multiboot2 ELF (magic + checksum checked). The interactive QEMU boot is exercised
by CI (GitHub runners have QEMU + GRUB); it had not been run in the authoring
sandbox, where QEMU wasn't installable.

## Resources leaned on

- The OSDev wiki
- Philipp Oppermann's *Writing an OS in Rust* series
- The Intel SDM (Vol. 3, system programming) for the long-mode transition

## License

**MIT** — see [LICENSE-MIT](LICENSE-MIT).

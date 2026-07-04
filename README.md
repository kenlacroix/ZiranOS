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

## Current status

The kernel boots via a **hand-written Multiboot2 + long-mode transition** in
assembly, hands off to a `no_std` Rust kernel that prints to the screen, and
installs interrupt handlers so a fault is reported instead of silently rebooting
(it catches a deliberate breakpoint and keeps running), takes keyboard input
over the PS/2 controller — typing echoes to the screen, interrupt-driven — and
reads the Multiboot2 memory map to stand up a bitmap **physical-frame allocator**
over usable RAM (alloc, free, and reclaim), and builds its own **page tables** —
switching `CR3` off the boot map to virtual memory it controls (`map`/`translate`/
`unmap`). That covers milestones 1–7. Everything assembles, compiles, and links on
stable Rust; CI boots the image under headless QEMU on every push.

New here? Start with the plain-language explainers in
[`docs/concepts/`](docs/concepts/) — e.g. [interrupts](docs/concepts/interrupts.md),
written from first principles alongside the code. Or boot it in your browser and
learn as it runs: the **[`web/`](web/) teaching tool** loads the real kernel in a
PC emulator beside a guided predict → observe → explain tour (`make web`, then
serve `web/`).

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
| M8+ heap, scheduling, shell, FS | ⬜ next |

## What happens when it boots

```
GRUB (Multiboot2)
  → boot/boot.asm            32-bit: verify CPU, build page tables, enter long mode
  → boot/long_mode_init.asm  64-bit: load segments, call into Rust
  → kernel_main (src/lib.rs) print a banner to VGA + serial, then halt
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
src/                    the no_std Rust kernel
  lib.rs                  kernel_main, panic handler, halt loop
  vga_buffer.rs           VGA text-mode writer + println! macros
  serial.rs               16550 UART (COM1) writer — what CI reads back
  interrupts.rs           the IDT and the exception/interrupt dispatcher
  port.rs                 shared port-mapped I/O (inb/outb)
  pic.rs                  8259 PIC: remap, mask, end-of-interrupt
  keyboard.rs             PS/2 scancode -> character translation
linker.ld               places the Multiboot header first, kernel at 1 MiB
grub/grub.cfg           one-entry GRUB menu for the bootable ISO
Makefile                the whole build/run/debug pipeline, spelled out
.github/workflows/ci.yml build + headless boot smoke test on every push
CLAUDE.md               the per-milestone workflow (adapted from gstack)
web/                    the browser teaching tool (boots the real kernel + guided tour)
docs/concepts/          plain-language explainers, written to teach
docs/blog/              the narrative, one post per milestone
docs/MILESTONE_CHECKLIST.md  the loop every milestone runs
```

## Building and running

Requires: a Rust toolchain (pinned to stable in `rust-toolchain.toml`), `nasm`,
`ld`, and for booting `qemu-system-x86_64`, `grub-mkrescue`, and `xorriso`.

```sh
rustup target add x86_64-unknown-none   # one-time; also in rust-toolchain.toml

make               # assemble + compile + link  -> build/kernel.bin
make check-header  # sanity-check the Multiboot2 magic
make iso           # build a bootable ISO         -> build/ziran.iso
make run           # boot it in a QEMU window
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

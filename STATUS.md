# Status

Live progress tracker. Each milestone maps to one demoable state and one blog
post; see [PLAN.md §5](PLAN.md) for the full roadmap.

Legend: ✅ done · 🚧 in progress · ⬜ not started

| # | Milestone | Status | Notes |
|---|-----------|--------|-------|
| 0 | Toolchain & environment | ✅ | Stable Rust + `x86_64-unknown-none`, `nasm`, `ld`. Kernel compiles; `Makefile` + CI in place. |
| 1 | Bootloader (Multiboot2, protected mode) | ✅ | `boot/multiboot_header.asm` + `boot/boot.asm`: header found, multiboot/CPUID/long-mode checks, page tables built. |
| 2 | Long mode + Rust entry | ✅ | PAE + EFER.LME + CR0.PG, 64-bit GDT, far jump; `long_mode_init.asm` calls `kernel_main`. |
| 3 | VGA text output | ✅ | `vga_buffer.rs` writes 0xb8000 directly, with scrolling + `println!`. Serial mirror in `serial.rs`. |
| 4 | GDT / IDT / interrupts | ✅ | IDT wired for all 256 vectors (`boot/isr.asm` + `src/interrupts.rs`); `#BP` recovers, `#PF`/others report (CR2 + decoded error) and halt. `int3` self-test. See `docs/concepts/interrupts.md`. IST/TSS deferred. |
| 5 | Keyboard input (PS/2) | ⬜ | Next. Assign a vector to the keyboard IRQ, program the PIC, and enable hardware interrupts (`sti`) for the first time. |
| 6 | Physical memory management | ⬜ | Will consume the Multiboot2 memory map (pointer already stashed in RDI at handoff). |
| 7 | Paging / virtual memory | ⬜ | |
| 8 | Heap allocator (`alloc`) | ⬜ | |
| 9 | Timer + scheduling | ⬜ | |
| 10 | Simple shell | ⬜ | |
| 11 | Filesystem (read) | ⬜ | |
| 12 | **File manager** (end goal) | ⬜ | |
| 13 | Userspace / syscalls (stretch) | ⬜ | First real privilege boundary — enables the security track. |
| 14 | Networking stub (stretch) | ⬜ | |
| 15 | Break the privilege boundary (security) | ⬜ | Adversarial self-testing; needs M13. See `/red-team` + PLAN.md §8. |
| 16 | Break the filesystem boundary (security) | ⬜ | Needs M11–12. |
| — | v86 browser demo | ⬜ | Same multiboot image, embedded on the blog. |

## Verification state

- **Assembles / compiles / links:** verified — `make` produces a valid Multiboot2
  ELF (`make check-header` confirms the magic).
- **Boots under QEMU:** exercised by CI (`make run-headless`), which asserts the
  kernel's long-mode serial marker appears.

## Next up (Milestone 5 — keyboard input)

1. Program the 8259 PIC: remap the IRQs off the CPU's exception vectors (they
   collide by default) and unmask the keyboard line.
2. Add a handler for the keyboard IRQ vector that reads the scancode from the
   PS/2 data port (0x60).
3. Enable hardware interrupts for the first time (`sti`) — until now only
   synchronous exceptions could fire.
4. Translate scancodes to characters and echo them to the screen.

Deferred from M4 (fold in when robustness matters): a proper Rust-built GDT with
a TSS + IST stack for the double-fault handler, so even a stack overflow reports
instead of triple-faulting. The `ist` field in each IDT entry is already stubbed.

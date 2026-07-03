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
| 5 | Keyboard input (PS/2) | ✅ | PIC remapped to 0x20–0x2F + masked to keyboard only (`src/pic.rs`); IRQ1 handler reads scancodes and echoes (`src/keyboard.rs`, US QWERTY + shift); `sti` enabled; backspace works; spurious-IRQ safe. See `docs/concepts/keyboard-and-pic.md`. |
| 6 | Physical memory management | ⬜ | Next. Consume the Multiboot2 memory map (pointer already stashed in RDI at handoff) and build a frame allocator over usable RAM. |
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

## Next up (Milestone 6 — physical memory management)

1. Parse the Multiboot2 boot information structure (its address is already in
   RDI at the `kernel_main` handoff) to find the memory map.
2. Walk the memory-map entries to learn which physical regions are usable RAM
   vs. reserved.
3. Build a frame allocator that hands out and reclaims 4 KiB physical frames,
   skipping the regions the kernel and boot structures already occupy.

Deferred (fold in when robustness matters): a proper Rust-built GDT with a TSS +
IST stack for the double-fault handler, so even a stack overflow reports instead
of triple-faulting. The `ist` field in each IDT entry is already stubbed.

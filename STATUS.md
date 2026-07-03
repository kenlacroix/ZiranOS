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
| 4 | GDT / IDT / interrupts | ⬜ | Next. Install an IDT, handle divide-by-zero and page faults without triple-faulting. |
| 5 | Keyboard input (PS/2) | ⬜ | |
| 6 | Physical memory management | ⬜ | Will consume the Multiboot2 memory map (pointer already stashed in RDI at handoff). |
| 7 | Paging / virtual memory | ⬜ | |
| 8 | Heap allocator (`alloc`) | ⬜ | |
| 9 | Timer + scheduling | ⬜ | |
| 10 | Simple shell | ⬜ | |
| 11 | Filesystem (read) | ⬜ | |
| 12 | **File manager** (end goal) | ⬜ | |
| 13 | Userspace / syscalls (stretch) | ⬜ | |
| 14 | Networking stub (stretch) | ⬜ | |
| — | v86 browser demo | ⬜ | Same multiboot image, embedded on the blog. |

## Verification state

- **Assembles / compiles / links:** verified — `make` produces a valid Multiboot2
  ELF (`make check-header` confirms the magic).
- **Boots under QEMU:** exercised by CI (`make run-headless`), which asserts the
  kernel's long-mode serial marker appears.

## Next up (Milestone 4)

1. Load a proper GDT from Rust (replacing the minimal boot-time one).
2. Build and load an IDT.
3. Handler for `#DE` (divide error) and `#PF` (page fault) that reports and
   recovers/halts instead of triple-faulting.
4. Double-fault handler with its own IST stack — the safety net that turns an
   otherwise-silent triple fault into a legible message.

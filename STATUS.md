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
| 6 | Physical memory management | ✅ | `multiboot.rs` parses the Multiboot2 memory map (pointer already in RDI at handoff); `frame_allocator.rs` is a bitmap allocator over usable RAM with alloc/free/reclaim, reserving frame 0, the kernel image (`kernel_start`/`kernel_end` linker symbols), and the boot structure. Self-test + `M6:` marker asserted by CI. See `docs/concepts/physical-memory.md`. |
| 7 | Paging / virtual memory | ⬜ | Next. First customer of the frame allocator — allocate frames to hold page tables. |
| 8 | Heap allocator (`alloc`) | ⬜ | |
| 9 | Timer + scheduling | ⬜ | |
| 10 | Simple shell | ⬜ | |
| 11 | Filesystem (read) | ⬜ | |
| 12 | **File manager** (end goal) | ⬜ | |
| 13 | Userspace / syscalls (stretch) | ⬜ | First real privilege boundary — enables the security track. |
| 14 | Networking stub (stretch) | ⬜ | |
| 15 | Break the privilege boundary (security) | ⬜ | Adversarial self-testing; needs M13. See `/red-team` + PLAN.md §8. |
| 16 | Break the filesystem boundary (security) | ⬜ | Needs M11–12. |
| — | Teaching tool (`web/`) | 🚧 | Dual-mode v86 embed + predict→observe→explain tour, covering M0–5. Grows one step per milestone. See `web/README.md`. |

## Verification state

- **Assembles / compiles / links:** verified — `make` produces a valid Multiboot2
  ELF (`make check-header` confirms the magic).
- **Boots under QEMU:** exercised by CI (`make run-headless`), which asserts both
  the long-mode marker and `M6: frame allocator online`. Also boot-tested
  natively on macOS (Apple Silicon) under UEFI — see `docs/POST_UPDATE_SETUP.md`.
- **Frame allocator:** the boot self-test proves alloc hands out distinct,
  aligned frames; reclaim returns a freed frame; and double-free / out-of-range
  free are rejected without corrupting the free count.

## Next up (Milestone 7 — paging / virtual memory)

1. Build a fresh set of x86-64 page tables, allocating the frames to hold them
   from the Milestone 6 frame allocator.
2. Map the kernel and switch `CR3` to the new tables — the first memory the
   kernel addresses through mappings *it* chose, not the boot identity map.
3. This is also what finally lets the kernel touch frames above the 1 GiB
   identity map that the allocator can already track.

Deferred (fold in when robustness matters): a proper Rust-built GDT with a TSS +
IST stack for the double-fault handler, so even a stack overflow reports instead
of triple-faulting. The `ist` field in each IDT entry is already stubbed.

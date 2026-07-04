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
| 7 | Paging / virtual memory | ✅ | `paging.rs` builds the kernel's own 4-level page tables from M6 frames, identity-maps physical memory, verifies them in-code, and switches `CR3` off the boot map. Exposes `translate`/`map_page` (fallible)/`unmap_page`; self-test round-trips a frame mapped above the 1 GiB window. See `docs/concepts/virtual-memory.md`. |
| 8 | Heap allocator (`alloc`) | ✅ | `heap.rs`: a linked-list free-list allocator as the `#[global_allocator]`, over a 1 MiB heap eagerly mapped at 1 GiB (M7 `map_page` onto M6 frames). `Vec`/`Box`/`String` work; self-test proves reclaim (free → re-alloc reuses the address). See `docs/concepts/heap.md`. |
| 9 | Timer + scheduling | ✅ | `pit.rs` programs the 8254 at 100 Hz (mode 2, IRQ0); `task.rs` is a round-robin scheduler over the heap where a task is a saved stack pointer, switched by `boot/switch.asm`'s `switch_context`. Cooperative first (`yield_now`, clean `ABABAB`), then preemptive: the timer IRQ drives `preempt`, so two tasks that never yield still interleave. Scheduler lock is interrupt-safe (IF-guarded). Self-tests + `M9:` markers asserted by CI. See `docs/concepts/timer-and-scheduling.md`. |
| 10 | Simple shell | ✅ | Interactive REPL running as a **task** on the M9 scheduler (`src/shell.rs`). The keyboard IRQ became a pure producer that pushes bytes into a lock-free SPSC ring (`keyboard::push`/`pop`); the shell task consumes it — echo, backspace, dispatch — and `hlt`s when idle. Built-ins `help`/`echo`/`clear`/`mem`/`ps`; `ps`/`mem` snapshot live scheduler/heap state under the IF-guarded, allocation-free discipline. Rewiring echo out of the IRQ also retired a latent VGA-lock deadlock. Self-test pins the (pure) editing/parsing/ring/accessor logic; `M10:` marker asserted by CI. See `docs/concepts/input-and-shell.md`. |
| 11 | Filesystem (read) | ✅ | `src/fs.rs`: a minimal read-only **ZranFS** over a heap RAM disk. `boot_image()` is an in-kernel `mkfs` (writer) that packs files into a `Vec<u8>` — 16-byte superblock (`ZRFS` magic, version, file_count, total_size), 32-byte dir entries (name + offset + length), flat data. `Fs::mount` is the reader **and** the whole trust boundary: it re-derives the directory from raw bytes trusting only the array, validating magic/version/total_size/dir-fits/every-extent-in-bounds with checked arithmetic. `list`/`read` are then total, panic-free, zero-copy — no `unsafe`. The shell gains `ls`/`cat`. Self-test proves exact bytes + **5** corrupt-image rejections (the M16 seed); `M11:` marker asserted by CI. Custom over FAT16 (no `mkfs.fat` build magic — PLAN §7). See `docs/concepts/filesystem.md`. |
| 12 | **File manager** (end goal) | ✅ | **The end goal — reached.** ZranFS **v2** adds subdirectories (a directory is a file whose bytes are more entries; one `kind` byte, the delta). `Fs::mount` recurses the tree and is provably safe on any input: a monotonic **high-water-mark** keeps directory tables globally disjoint (validated once → linear, DAGs/cycles → `DirNotForward`) and a `MAX_DEPTH` cap bounds the stack (`TooDeep`). The shell gains `cd`/`pwd`/`ls`/`cat` with a cwd + a pure, unit-tested `canonicalize` (`.`/`..` as string math, no disk parent-pointers) and a `ziran:/docs>` prompt. Driveable in a terminal via the new **serial console** (`make console`). Self-test navigates the tree, reads a subdir file byte-exact, and rejects **10** corrupt images (cycle, deep chain, diamond DAG — no hang, no overflow); `M12:` marker asserted by CI. A hidden `FLAG{…}` (no dir entry) is planted for M16. Completes the core arc. See `docs/concepts/filesystem.md`. |
| 13 | Userspace / syscalls (stretch) | 🚧 | First real privilege boundary — enables the security track. Think→Plan done (`docs/planning/milestone-13-eng-plan.md`). **Build steps 1–2/6 landed:** (1) `src/gdt.rs` rebuilds the GDT in Rust (null, k-code 0x08, k-data 0x10, u-code 0x18/DPL3, u-data 0x20/DPL3) + a TSS with RSP0 (+ IST1 stack); `lgdt`, far-return CS reload, `ltr`; `tr=0x28` asserted. (2) `src/paging.rs` gains a `USER` (U/S) flag + `map_user_page`, which sets U/S on the leaf **and every table on the walk** (OR-ing it into the shared `PML4[0]`) — the CPU ANDs U/S down the whole chain, so one missing bit anywhere denies ring 3; `user_map_self_test` re-walks and proves U/S=1 at all 4 levels + leaf. Boots clean through M12 to the shell; kernel memory stays supervisor-only. (3) `src/interrupts.rs` gains a DPL-3 `int 0x80` gate (`set_user_handler`, `type_attr=0xEE`) + a `syscall` dispatch arm (`SYS_PRINT`/`SYS_EXIT`, return value written to `ctx.rax`); `syscall_self_test` issues `int 0x80` from ring 0 and confirms it dispatches (reports `CPL=0`, returns 0) — the CPL readout that step 4 will assert is 3 from ring 3. Next: the ring-3 `iretq` excursion (`src/usermode.rs`) + `M13: userspace online`. No ring-3 code runs yet. |
| 14 | Networking stub (stretch) | ⬜ | |
| 15 | Break the privilege boundary (security) | ⬜ | Adversarial self-testing; needs M13. See `/red-team` + PLAN.md §8. |
| 16 | Break the filesystem boundary (security) | ⬜ | Needs M11–12. Objective: **exfiltrate a hidden secret** (bytes on the RAM disk with no directory entry) by aliasing them past a file's extent — targets M11's deliberately-loose extent check. Groundwork (the hidden bytes) is planted in M12's `mkfs`. See PLAN §8. |
| — | Serial console (`make console`) | ✅ | The shell is drivable over COM1. `src/serial.rs` gained a non-blocking `recv`/`read_byte`; the shell task polls it, so keystrokes in and output out flow over the serial line — the only interactive console when the VGA text buffer isn't displayed (UEFI/OVMF). Wire it to your terminal with `make console` (or `./run`); it's also what the recorded web session was captured from. |
| — | Teaching tool (`web/`) | 🚧 | Guided predict→observe→explain **tour** over M0–M12, with an **ELI5** toggle (plain-language lens) and the "why this exists / not AI slop" **manifesto** up front. The tour is a *faithful reconstruction* — exact kernel output, replayed — because a 64-bit kernel can't run in v86/the lightweight browser emulators. Plus **[a real recorded session](web/replay.html)** (`web/ziran-session.cast`) captured over serial. A **genuine live boot** — real QEMU compiled to WebAssembly (`web/build-qemu-wasm.sh`, `web/qemu-wasm.html`) booting the actual 64-bit kernel via `-kernel` in the tab — **works: verified booting to an interactive `ziran:/>` shell in a browser** — by hand, and by an automated **headless-Chrome** end-to-end test (`web/browser-test.py` / `make web-test`, which boots the page and drives `ps`). Bring-up surfaced + fixed two latent boot bugs — an unloaded GOT on the flat/`-kernel` path, and SSE not explicitly enabled — and one time-dilation quirk (the preemption self-test warns instead of panicking under qemu-wasm's browser timing). Its "Boot the real kernel" button is enabled (`QEMU_WASM_READY=true`); the assets (~17 MB total, `.data` trimmed to 0.5 MB) just aren't publicly **hosted** yet — run it via `make serve` / self-host. Hosting write-up: `docs/DEPLOY.md`. See `docs/planning/web-live-boot-qemu-wasm.md`. |

## Verification state

- **Assembles / compiles / links:** verified — `make` produces a valid Multiboot2
  ELF (`make check-header` confirms the magic).
- **Boots under QEMU:** exercised by CI (`make run-headless`), which asserts both
  the long-mode marker and `M12: file manager online`. Also boot-tested natively on
  macOS (Apple Silicon) under UEFI — see `docs/POST_UPDATE_SETUP.md`. **Interactive:**
  `make console` wires COM1 to the terminal so the shell is drivable there (the VGA
  text console isn't displayed under UEFI/OVMF).
- **Frame allocator:** the boot self-test proves alloc hands out distinct,
  aligned frames; reclaim returns a freed frame; and double-free / out-of-range
  free are rejected without corrupting the free count.
- **Paging:** the kernel runs on page tables it built itself (surviving the `CR3`
  switch proves it mapped its own code/stack/GDT/IDT); the self-test maps a frame
  at a virtual address above the 1 GiB identity window and round-trips a sentinel
  through both the virtual and physical address.
- **Heap:** `Vec`/`Box`/`String` work; the self-test grows a `Vec` past its
  capacity, allocates across four pages, and proves reclaim (free → re-alloc
  returns the same address) — plus that runtime allocation touches no frames.
- **Shell / input:** the boot self-test drives the shell's *pure* logic on serial
  without a keyboard — line editing (echo/backspace/submit), command parsing (the
  `echo` tail, unknown verbs), the lock-free input ring's FIFO round-trip, and the
  `ps`/`mem` accessors (idle task present, heap free in range). Interactive typing
  (`help`, `echo`, `clear` output on VGA) is a manual `make run` check — CI can't
  type, but the logic is factored into pure functions so it is pinned regardless.
- **Filesystem (M11+M12):** the boot self-test builds a **v2 directory tree**,
  navigates into `docs/`, reads a subdirectory file's **exact bytes** (writer and
  reader share no types), and rejects **ten** deliberately corrupt images each with
  its specific `FsError` — the M11 five (bad magic/version/size/truncated/extent)
  plus a directory **cycle** (`DirNotForward`, *without hanging*), a **deep chain**
  (`TooDeep`, no stack overflow), and a **diamond DAG** (`DirNotForward`, no
  exponential re-validation). The pure `canonicalize` path math is unit-tested
  exhaustively. Interactive `cd`/`ls`/`cat` is drivable over `make console`.

## Next up (Milestone 13 — userspace / syscalls, stretch)

The core goal is met; from here the roadmap is stretch + the security track. Its
**eng-plan is being written now** (Think → Plan phase); no kernel code exists yet.

1. Basic **ring-3 separation** and a minimal **syscall interface** — the first
   real privilege boundary, which is what the security milestones need to exist.
2. This unlocks **M15** (break the privilege boundary): from ring 3, try to read a
   kernel-only page or run a privileged instruction and watch the CPU stop you.
3. **M16** (break the filesystem boundary) is already seeded: the hidden
   `FLAG{…}` is planted on the M12 disk, unreachable by `ls`/`cat` — the exfil
   target is a crafted image that aliases its bytes past a file's extent.

Deferred (fold in when robustness matters):
- A proper Rust-built GDT with a TSS + IST stack for the double-fault handler, so
  even a stack overflow reports instead of triple-faulting. The `ist` field in
  each IDT entry is already stubbed — and M9's per-task stacks have no guard page,
  which makes this more valuable than before.
- A **local APIC timer** (with calibration and moving off the 8259s) as the
  "real" successor to the M9 PIT — a named future milestone. The M9 scheduler is
  timer-source agnostic, so it's a drop-in later.
- **Allocation-free scheduler mutation under the IF-guarded lock.** `task::spawn`
  still allocates (the task stack, plus `Vec`/`VecDeque` growth) while holding
  `SCHED` with interrupts off — safe today because it only runs at setup, before
  any other allocating task exists, but a milestone that spawns tasks at runtime
  must make it allocation-free first (as `task::snapshot` already is). See the M10
  eng-plan and the note on `spawn`.
- A **lock-free emergency writer** for the panic/fault path, which still prints
  through the VGA `WRITER` spinlock (a fault mid-`print!` would hang) — pre-dates
  M10 but its window is wider now that the shell prints continuously.

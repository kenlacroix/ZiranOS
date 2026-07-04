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
| 13 | Userspace / ring 3 / syscalls (stretch) | ✅ | **The project's first CPU privilege boundary — the security track is now open.** `src/gdt.rs` rebuilds the GDT in Rust with ring-3 code/data descriptors + a TSS whose RSP0 catches the ring 3 → 0 transition (`tr=0x28`); `src/paging.rs` adds a `USER` flag + `map_user_page` that sets U/S on the leaf **and every table on the walk** (the CPU ANDs it down the whole chain; kernel leaves stay U/S=0); `src/interrupts.rs` adds a DPL-3 `int 0x80` gate + `syscall` dispatch. **The excursion** (`src/usermode.rs` + `boot/usermode.asm`): a position-independent blob entered by a fabricated `iretq` (CS 0x1b/SS 0x23) prints `'Z'` via `int 0x80` **from CPL 3**, then returns to ring 0 by rewriting the ISR's saved frame. **Enforcement test:** `cli` → #GP and `mov rax,[0xb8000]` → #PF, both caught at CPL 3 (verified by serial + QEMU `-d int`: `v=80`/`v=0d`/`v=0e cpl=3`). `/kernel-review` (no critical/high; 4 low fixes) + `/red-team` (9/9 ring-3 escapes blocked; `sgdt`/`sidt` info-leak found, UMIP would close it) done. Concept doc `docs/concepts/privilege.md`, blog post `docs/blog/13-the-first-wall.md`, web tour step. See `docs/planning/milestone-13-{eng-plan,red-team,retro}.md`. Marker `M13: userspace online`. |
| 14 | Networking stub (stretch) | ⬜ | |
| 15 | Break the privilege boundary (security) | ✅ | **Flag captured, then contained — the confused-deputy lesson.** A `FLAG{…}` is planted on a **kernel-only (U/S=0)** page at 1.5 GiB (`src/usermode.rs`, `map_page` → supervisor leaf). Two ring-3 attacks: a **direct** read of the secret VA → **#PF** (the M13 hardware boundary, held); and the kernel's first **pointer-carrying syscall** as the *confused deputy*. `src/interrupts.rs` adds `SYS_WRITE(ptr,len)` (validated) and `SYS_WRITE_UNCHECKED` (the deliberately-broken twin). The validator `paging::user_range_ok` walks the page tables requiring `PRESENT|USER` at **every** level (ANDing U/S like the MMU), rejecting overflow/non-canonical/kernel ranges **without dereferencing**. Result (all four asserted, CI-pinned): unchecked deputy **leaks** the flag with ring-0 power; validated deputy **rejects** the same kernel pointer (-1, nothing read); validated deputy still **permits** a legit user buffer. **The lesson:** the CPU stops *unauthorized access*, but only software (`copy_from_user`) stops *authorized misuse*. `/kernel-review` clean (no critical/high; 3 low fixes) + `/red-team` (validated path holds vs. aliasing/TOCTOU/arithmetic; the identity-map alias of the secret is stopped by the un-loosened `PDPT[0]` — `map_user_page`'s `virt≥IDENTITY_LIMIT` assert is load-bearing security). Red-team found + fixed **one real DoS**: `user_range_ok` accepted a non-canonical pointer that then #GP'd the kernel in ring 0 — closed by rejecting `end > 2^47`, and pinned as a CI invariant. `sgdt`/`sidt` info-leak still open (UMIP off, a deliberate non-goal). Concept doc `docs/concepts/privilege.md`, blog `docs/blog/15-the-confused-deputy.md`, web tour step. See `docs/planning/milestone-15-eng-plan.md`. Marker `M15: privilege boundary`. |
| 16 | Break the filesystem boundary (security) | ⬜ | Needs M11–12. Objective: **exfiltrate a hidden secret** (bytes on the RAM disk with no directory entry) by aliasing them past a file's extent — targets M11's deliberately-loose extent check. Groundwork (the hidden bytes) is planted in M12's `mkfs`. See PLAN §8. |
| — | Serial console (`make console`) | ✅ | The shell is drivable over COM1. `src/serial.rs` gained a non-blocking `recv`/`read_byte`; the shell task polls it, so keystrokes in and output out flow over the serial line — the only interactive console when the VGA text buffer isn't displayed (UEFI/OVMF). Wire it to your terminal with `make console` (or `./run`); it's also what the recorded web session was captured from. |
| — | Teaching tool (`web/`) | 🚧 | Guided predict→observe→explain **tour** over M0–M13 + M15, with an **ELI5** toggle (plain-language lens) and the "why this exists / not AI slop" **manifesto** up front. The tour is a *faithful reconstruction* — exact kernel output, replayed — because a 64-bit kernel can't run in v86/the lightweight browser emulators. Plus **[a real recorded session](web/replay.html)** (`web/ziran-session.cast`) captured over serial. A **genuine live boot** — real QEMU compiled to WebAssembly (`web/build-qemu-wasm.sh`, `web/qemu-wasm.html`) booting the actual 64-bit kernel via `-kernel` in the tab — **works: verified booting to an interactive `ziran:/>` shell in a browser** — by hand, and by an automated **headless-Chrome** end-to-end test (`web/browser-test.py` / `make web-test`, which boots the page and drives `ps`). Bring-up surfaced + fixed two latent boot bugs — an unloaded GOT on the flat/`-kernel` path, and SSE not explicitly enabled — and one time-dilation quirk (the preemption self-test warns instead of panicking under qemu-wasm's browser timing). Its "Boot the real kernel" button is enabled (`QEMU_WASM_READY=true`); the assets (~17 MB total, `.data` trimmed to 0.5 MB) just aren't publicly **hosted** yet — run it via `make serve` / self-host. Hosting write-up: `docs/DEPLOY.md`. See `docs/planning/web-live-boot-qemu-wasm.md`. |

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
- **Privilege boundary (M15):** the boot self-test plants a `FLAG` on a
  kernel-only page and asserts four outcomes, so CI pins the whole lesson: a
  **direct** ring-3 read of the secret takes a **#PF** (held); the **unchecked
  deputy** (`SYS_WRITE_UNCHECKED`) leaks the flag and the capture is asserted
  **byte-equal** to the planted constant (a real capture, not console noise); the
  **validated deputy** (`SYS_WRITE` + `user_range_ok`) rejects the same kernel
  pointer and emits nothing; and the validated deputy still **passes a legitimate
  user buffer** (proving the check permits, not just denies). Three direct
  validator assertions pin the red-team's findings — `user_range_ok` denies a
  kernel identity page, a non-canonical pointer, and an overflowing range.

## Next up (the security track + stretch)

The core goal (M12) is met, the first privilege boundary (M13) is built, and the
first security milestone (M15) is done — the privilege boundary has been attacked
in earnest and the confused-deputy syscall closed. From here the roadmap is the
remaining security milestone and the stretch goals — in any order:

1. **M16 — break the filesystem boundary** is already seeded: the hidden `FLAG{…}`
   is planted on the M12 disk, unreachable by `ls`/`cat` — the exfil target is a
   crafted image that aliases its bytes past a file's extent. This is the direct
   analogue of M15's confused deputy, one layer down: M15 showed that a syscall
   validating *permission but not addressability* is only half a check; M16 targets
   the FS's extent check, which validates *in the image* but not *within a file's
   own region*.
2. **M14 — networking stub** (loopback / trivial virtio-net), a separate stretch.
3. **Close the M13/M15 `sgdt`/`sidt` leak (optional).** A one-line `CR4.UMIP`
   write turns the two non-privileged store-descriptor reads into #GP from ring 3.
   Left open on purpose — it is *hardening*, a PLAN §1 non-goal — but a good
   "watch the mitigation land" demonstration if a future post wants it.

Deferred (fold in when robustness matters):
- A **TSS + IST stack for the double-fault handler**, so even a stack overflow
  reports instead of triple-faulting. M13 built the TSS and wired an IST1 stack
  into it, but no IDT gate uses IST yet (`ist` is still 0); pointing the #DF gate
  at IST1 is the remaining step. M9's per-task stacks (and M13's RSP0 stack) still
  have no guard page, which makes this more valuable than before.
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

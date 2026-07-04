# Milestone 13 — userspace / ring 3 / syscalls — eng-plan

Locks the Think→Plan before building. The **end goal** (PLAN §5, stretch row 13):
"Basic ring 3 separation, a minimal syscall interface." This is the project's
**first CPU privilege boundary** — the first time code runs that the *hardware*,
not the kernel's own goodwill, holds back. Everything to date has run in ring 0
with full power; there was nothing to "break out of." M13 builds the jail M15
then tries to break out of (PLAN §8, privilege-boundary bullet).

The milestone teaches one thing: **a privilege boundary is enforced by the CPU,
not by the kernel.** You do not write an `if` that checks "is this code allowed
to touch that page." You set two bits — the descriptor's DPL and the page's U/S —
and from then on the silicon faults *for* you. The hard part is not the syscall;
it is standing up the four cooperating pieces (GDT descriptors, a TSS with RSP0,
a DPL-3 gate, U/S=1 mappings) so that the ring 0→3→0 round-trip lands on a valid
stack instead of triple-faulting.

Companion concept doc to write during the build: `docs/concepts/privilege.md`
(rings, the iretq frame, the TSS, the U/S bit — from first principles, house
style like `interrupts.md` / `virtual-memory.md`).

---

## 1. Office hours (the six forcing questions)

**Q1 — The single thing I'll understand after this that I don't now.**
That a privilege boundary is a *hardware* property: ring 3 code cannot read a
ring-0 (U/S=0) page or run a privileged instruction (`cli`, `hlt`, `mov cr3`),
and the CPU enforces this on every instruction with zero kernel involvement —
the kernel only *configures* the boundary (DPL + U/S bits), the CPU *is* it.

**Q2 — "Done" as an observable.** On serial, in one boot:
1. A user-mode blob runs at **CPL 3** and makes **one** syscall (`int 0x80`),
   which the kernel services and returns from cleanly — printed, e.g.
   `[m13] syscall from ring 3: SYS_PRINT 'Z' (CS=0x1b, CPL=3)`.
2. A second blob **deliberately violates** the boundary — reads a kernel page
   (`mov rax, [0xb8000]`) or runs `cli` — and the CPU faults (**#PF** with the
   U bit set, or **#GP**). The handler reports it with the faulting **CS showing
   CPL=3**: `[m13] blocked ring-3 violation: #GP at RIP=… CS=0x1b (CPL=3)`.
3. Marker `M13: userspace online`.

**Q3 — The smallest version that still teaches it.** ONE user "task": a tiny
position-independent blob (a handful of bytes: load a register, `int 0x80`,
loop/halt-via-syscall) copied into a freshly allocated frame mapped
**user-accessible above the 1 GiB identity window**, entered by a hand-crafted
`iretq`. One syscall: `SYS_PRINT` that takes a **char in a register** (no user
pointer to validate yet). No ELF, no loader, no scheduler integration, no libc.
The blob lives in the kernel image but *runs* from a U/S=1 copy — because the
kernel's own pages are U/S=0 and ring 3 literally cannot execute from them.

**Q4 — Working vs. accidentally working.** The trap is a blob that "runs fine"
but is *secretly still in ring 0* (e.g. the iretq frame's CS had RPL=0, or the
`ltr`/DPL wiring is wrong and `int 0x80` was serviced from ring 0). Then nothing
is being enforced and the demo proves nothing. The **gate is the deliberate
violation**: if `cli`/`mov [0xb8000]` from the blob is *caught as a fault whose
saved CS has CPL=3*, the CPU was genuinely in ring 3 and genuinely stopped it.
"It printed and didn't crash" is not enough; "it tried to cheat and the hardware
said no, from CPL 3" is the proof. Assert `ctx.cs & 3 == 3` on both the syscall
path and the fault path.

**Q5 — What could stall this for days.** The classic: a **triple fault on the
ring 3→ring 0 transition**. When `int 0x80` (or any fault) fires in ring 3, the
CPU switches to ring 0 and loads RSP from **TSS.RSP0**. If there is no TSS
loaded (`ltr` never run), or RSP0 is garbage/misaligned, the CPU can't push the
interrupt frame → **#DF** → and if the #DF handler *also* can't get a stack →
triple fault → silent reboot, no output. The current kernel has **no TSS at
all** (`grep` confirms) and every IDT gate has **IST=0**. Tool: `make debug` +
GDB — `info registers` (is CS=0x1b after iretq?), examine the TSS bytes and
RSP0, dump the GDT descriptors' DPL nibble, and read the fault frame's CS. Also
QEMU `-d int,cpu_reset` to see the fault chain before the reboot.

**Q6 — Non-goal drift.** Real risk here. "Userspace" pulls toward a real process
model, an ELF loader, a POSIX-ish syscall ABI, `fork`/`exec`, multiple user
processes — all PLAN §1 non-goals (no POSIX, no multi-user). See §2; the answer
is a hard **Hold** with an explicit deferral list.

---

## 2. Scope-guard verdict

**Verdict: Hold**, with a tightly-scoped cut.

Ring 3 + a syscall is squarely in scope — it is the named stretch milestone and
the *prerequisite* for the entire security track (PLAN §8: "no meaningful attack
surface until the kernel has a privilege boundary"). It teaches a real hardware
mechanism the rest of the project has only gestured at. But "userspace" is the
single biggest scope-creep magnet in the roadmap, so the cut is drawn tight:

**Ship:** exactly ONE user execution — a tiny in-kernel blob copied into a
U/S=1-mapped frame, entered via `iretq`, making ONE syscall (`int 0x80`,
`SYS_PRINT`) that the kernel services and returns from — **plus** the
deliberate-violation test (read a kernel page / run `cli` → caught as a fault
with CPL=3). That is the whole milestone.

**Defer, explicitly:**
- A real syscall **ABI** — one ad-hoc `int 0x80` with the number in `rax` is
  enough; no errno, no arg-count conventions, no vDSO.
- **`fork`/`exec` / a process model** — the "user program" is an `extern "C"`
  byte blob, not a loaded, scheduled process. No PCB, no address-space per
  process.
- **An ELF loader** — the blob is copied raw; parsing ELF is a non-goal detour.
- **Multiple user processes** — one blob, one address space (the kernel's, with
  a user region added). Multi-user is a PLAN §1 non-goal.
- **Full `copy_from_user` validation** — the minimal cut passes data in a
  register (no user pointer). Passing a user *buffer* + validating its range is
  the immediate next step and the deliberate M15 "confused deputy" surface;
  keep it out of the first cut.
- **A libc / runtime** — the blob is raw instructions.
- **Scheduler integration of user tasks** — run the ring-3 excursion inline from
  a dedicated `usermode::self_test()`, not as a `task::spawn`ed peer. Folding
  ring 3 into the preemptive scheduler (saving/restoring the user's ring-3 frame
  across a timer tick, per-task RSP0) is real work and a clean later milestone.

If any of these creeps in mid-build, stop and re-open this section — that is the
§6 "scope creep toward real features" risk made concrete.

---

## 3. Eng-plan

### 3.1 Approach

Four pieces, each grounded in a module that exists today. New code concentrates
in a new **`src/gdt.rs`** (the current GDT is a static table in `boot/boot.asm`
and cannot hold a runtime-computed TSS base), a new **`src/usermode.rs`** (the
blob, the mapping, the `iretq` entry, the self-test), small additions to
`src/paging.rs` (a `USER` flag) and `src/interrupts.rs` (a DPL-3 gate + a
`0x80` dispatch arm), and one tiny asm helper for the `iretq` launch.

**(a) GDT — three new descriptors, rebuilt in Rust.** Today `boot/boot.asm`'s
`gdt64` holds only the null descriptor (0x00) and one flat ring-0 64-bit code
segment (0x08, `KERNEL_CODE_SELECTOR` in `interrupts.rs`). Long mode still needs,
for a ring-3 round-trip:
- **Ring-3 code** (DPL=3): `(1<<44)|(1<<47)|(1<<43)|(1<<53)|(3<<45)` — executable,
  present, 64-bit, DPL 3. Selector `0x18`, used as `0x1b` (RPL 3).
- **Ring-3 data/stack** (DPL=3): `(1<<44)|(1<<47)|(1<<41)|(3<<45)` — writable
  data, present, DPL 3. Selector `0x20`, used as `0x23`. `iretq` to CPL 3
  *loads* SS from the frame, so a present DPL-3 data descriptor must exist.
- **TSS descriptor** (a 16-byte *system* descriptor in long mode): base = the
  address of our TSS, limit = `size_of::<Tss>() - 1`, type `0x9` (available
  64-bit TSS), present, DPL 0. Selector `0x28`.

Because the TSS base is a Rust address unknown at assembly time, the cleanest
move is to **rebuild the whole GDT in `src/gdt.rs`** (mirroring how `interrupts.rs`
builds the IDT), `lgdt` it, reload CS via a far return / `retfq`, load the data
selectors, then `ltr(0x28)`. The boot GDT got us into long mode; from `gdt::init()`
on we own it. (Alternative — patch just the TSS descriptor into the asm table at
runtime — is more fragile and keeps CS selector logic split across two languages;
rejected.)

**(b) TSS with RSP0.** A `#[repr(C, packed)]` 104-byte 64-bit TSS. The one field
that matters for M13 is **RSP0** (offset 4): the stack pointer the CPU loads when
an interrupt/trap moves it from ring 3 to ring 0. Point it at a dedicated,
16-byte-aligned kernel stack (a static `[u8; 16 KiB]` in `.bss`, or a heap
`Box<[u8]>` leaked for the kernel's life — like the boot stack, no guard page).
Optionally set **IST1** to a second known-good stack and point the #DF (and maybe
#GP/#PF) gate at it — see the honest unknown in §3.4/§3.5.

**(c) Syscall mechanism — recommend `int 0x80` via a DPL-3 IDT gate.** Two
options:
- **`int 0x80`, IDT gate with DPL=3.** Reuses *everything* already built: the
  256 stubs in `boot/isr.asm`, `isr_common` (which already saves the full
  `InterruptContext`, already `iretq`s, and — crucially — already relies on the
  CPU having switched to the RSP0 stack on the privilege change). The only
  additions are: a gate at vector `0x80` whose `type_attr` is `0xEE` (present |
  **DPL 3** | 64-bit interrupt gate) instead of the current hardcoded `0x8E`
  (DPL 0), and a `0x80` arm in `interrupt_dispatch` that reads the syscall number
  from `ctx.rax` and args from `ctx.rdi`/`rsi`/`rdx`, services it, and writes the
  return value back into `ctx.rax` (which `isr_common` restores on `iretq`).
  A DPL-3 gate is *required*: an `int 0x80` from ring 3 through a DPL-0 gate
  raises **#GP** (the CPL ≤ gate-DPL check), which would look like a bug.
- **`syscall`/`sysret`.** Needs `EFER.SCE`, and the `STAR`/`LSTAR`/`SFMASK` MSRs
  (STAR's selector layout is rigid; SFMASK must clear IF on entry). It does **not**
  switch stacks for you — the handler runs on the *user* stack until it manually
  swaps to a kernel stack (the classic footgun), so it needs its own asm entry and
  a per-CPU kernel-stack stash. More mechanism, more ways to silently run on the
  wrong stack.

**Recommendation: `int 0x80`.** It makes the *CPU* do the privilege check and the
RSP0 stack switch we specifically want to observe, reuses the M4 ISR path
end-to-end, and keeps the diff to "one DPL-3 gate + one dispatch arm." `syscall`/
`sysret` (and its MSR dance) is a clean *future* post, not this cut.

**(d) U/S=1 mappings — a new `USER` flag, plus the intermediate-table gotcha.**
`paging.rs` currently defines `PRESENT` / `WRITABLE` / `HUGE` only. Add
`pub const USER: u64 = 1 << 2;` (the U/S bit). Two subtleties that are the whole
game:
- The low 1 GiB is identity-mapped with **2 MiB huge pages, flags
  `PRESENT|WRITABLE` (U/S=0)**. So the entire kernel is *already* ring-0-only —
  ring 3 reading `0xb8000` or `0x10_0000` faults for free. That is exactly the
  enforcement the violation test exercises; we do **not** loosen it.
- `map_page` fails with `MapError::HugePage` inside that window, so **user pages
  must live above 1 GiB** (like `paging::self_test`'s `0x4000_0000`). Good — the
  user region gets its own fresh PDPT/PD/PT subtree, so marking those
  intermediates U/S=1 cannot loosen any kernel mapping.
- **The gotcha:** `map_page` hardcodes created intermediate tables as
  `next | PRESENT | WRITABLE` — **no USER**. The x86 page walk ANDs U/S down the
  *whole* chain: if *any* of PML4e/PDPTe/PDe lacks U/S=1, ring 3 cannot touch the
  leaf **even if the leaf's U/S=1**. So we need a `map_user_page(virt, phys,
  flags)` (or a threaded flag) that ORs `USER` into the intermediates it creates
  *and* into the leaf. Miss this and the user blob #PFs on its very first
  instruction fetch — a fast, loud failure, but name it so it isn't a mystery.

### 3.2 Preconditions

- **Long mode**, paging **on** with per-page U/S control (M7 `map_page`, extended
  with `USER`). CR3 points at the kernel-built PML4 (M7).
- **IDT loaded** (M4) with all 256 stubs; the `0x80` gate is re-set to DPL 3
  before any ring-3 code runs.
- **A valid, 16-byte-aligned RSP0 kernel stack** installed in the TSS, and the
  TSS loaded with `ltr`, **before** the first `iretq` to ring 3. This is the
  precondition whose violation triple-faults (Q5).
- **Interrupts:** run the whole `usermode::self_test` with the shell/scheduler
  *not yet* started, or at least with the excursion not preemptible. The user
  blob will run with IF=1 (its `iretq` frame sets RFLAGS=0x202) so it *can* be
  interrupted by the timer — but since we are not integrating with the scheduler
  this cut, prefer to run it **before `sti`/the scheduler** (like the other
  self-tests in `kernel_main`), so a timer tick can't try to preempt a ring-3
  context we have no code to save yet. Alternatively enter with IF=0 in the
  frame; the syscall/violation both complete without needing interrupts.
- **On exit:** the excursion returns to `kernel_main`'s init flow in ring 0 with
  the kernel GDT/IDT/CR3 unchanged and the RSP0 stack unused (drained).

### 3.3 Data flow

```
kernel_main
  └─ gdt::init()      build GDT (null, k-code, u-code@0x18, u-data@0x20, TSS@0x28)
  │                   lgdt ; reload CS (retfq) ; load data segs ; ltr(0x28)
  │                   TSS.rsp0 = top of a dedicated 16 KiB kernel stack
  └─ interrupts: set vector 0x80 gate to DPL 3 (type_attr 0xEE)
  └─ paging: add USER; map_user_page for the user code + user stack frames
  └─ usermode::self_test():
       1. alloc frame Fc; copy the PIC blob bytes into it; map Fc -> UVA_CODE
          (0x4000_0000) as USER|PRESENT   (executable: no NX set)
       2. alloc frame Fs; map Fs -> UVA_STACK as USER|WRITABLE|PRESENT
       3. build the iretq frame on the current kernel stack, high->low:
             SS     = 0x23  (u-data | RPL 3)
             RSP    = UVA_STACK_TOP   (16-aligned)
             RFLAGS = 0x202 (IF=1, reserved bit1) — or 0x2 to enter masked
             CS     = 0x1b  (u-code | RPL 3)
             RIP    = UVA_CODE
          then `iretq`  ── CPU drops to CPL 3, loads SS:RSP, jumps to UVA_CODE ──▶
       ─────────────────────────── ring 3 ───────────────────────────
       user blob: mov rax, SYS_PRINT ; mov rdi, 'Z' ; int 0x80
       ── CPU: CPL 3→0, loads RSP from TSS.RSP0, pushes SS/RSP/RFLAGS/CS/RIP,
          vectors to isr_stub_0x80 → isr_common → interrupt_dispatch ─────────▶
       kernel: match 0x80 => syscall(ctx): rax=SYS_PRINT, rdi='Z'
               assert ctx.cs & 3 == 3   (proof: serviced a CPL-3 caller)
               print the char ; ctx.rax = 0 (return value)
       ── isr_common restores regs ; iretq ── CPU: CPL 0→3, back to ring 3 ───▶
       user blob: SYS_EXIT  (int 0x80, rax=SYS_EXIT) → kernel does NOT iretq back;
               it records success and returns from self_test in ring 0.
  └─ "M13: userspace online"

FAULT PATH (the enforcement test), second blob:
       ring 3: cli    (or  mov rax,[0xb8000])
       ── CPU: privileged/ U/S violation → #GP (13) / #PF (14),
          CPL 3→0 on TSS.RSP0, standard isr_common frame ──────────────────▶
       interrupt_dispatch: vector 13/14, ctx.cs & 3 == 3
          print "[m13] blocked ring-3 violation: #GP/#PF … CS=0x1b (CPL=3)"
          do NOT resume the blob; unwind the excursion, continue kernel init.
```

The two handoff boundaries to get exactly right: **kernel→ring3** (the `iretq`
frame contents + alignment) and **ring3→kernel** (the CPU's automatic RSP0 stack
switch, which is invisible in the code and only correct if the TSS is right).

### 3.4 Edge cases

- **No/So bad RSP0 → double/triple fault.** The headline failure (Q5). The
  ring3→ring0 transition *needs* TSS.RSP0; get it wrong and there's no output.
  Mitigation: install and `ltr` the TSS before any `iretq`; verify RSP0 in GDB.
- **The guard-page-less stacks (task.rs note, reused here).** Both the RSP0
  kernel stack and the user stack are 16 KiB with **no guard page** — an overflow
  silently corrupts adjacent memory instead of faulting. The syscall/fault paths
  are shallow (one `InterruptContext` + a small dispatch chain), so a single
  excursion fits, but note it: a real per-process RSP0 with a guard page is a
  later refinement, same as the scheduler stacks.
- **Intermediate tables missing U/S=1** (§3.1d) → user #PFs on first fetch.
  `map_user_page` must set USER on every table it creates in the user subtree.
- **A fault from ring 3 must land on RSP0 (or an IST), never the user stack.**
  The CPU guarantees this *given a valid TSS*; the whole point of the TSS. If we
  later add `syscall`/`sysret`, that guarantee disappears and we'd have to switch
  stacks by hand.
- **IF / SFMASK on entry.** The `0x80` gate is an *interrupt* gate (type 0xE), so
  it clears IF on entry — the syscall handler is not itself preemptible, which is
  what we want. (If we ever used a *trap* gate (0xF), IF would stay as the user
  left it — a re-entrancy hazard.) With `syscall`, SFMASK would have to clear IF
  explicitly; not our path this cut.
- **`iretq` frame alignment.** RSP in the frame (the user stack top) should be
  16-byte aligned; a misaligned or wrong-order frame is a prime triple-fault
  cause (same lesson as `task::new`'s fabricated frame). Build it field-by-field
  with a known layout.
- **User can't read kernel pages — the enforcement itself.** Because the kernel
  identity map is U/S=0, `mov rax,[0xb8000]` from ring 3 is a #PF with the U bit
  (bit 2) set in the error code. This is not an edge case to *handle* so much as
  the behavior to *prove* (§3.5).
- **Position-independence of the blob.** The blob is copied to a relocated VA, so
  it must use no absolute addresses — just immediate-into-register + `int 0x80`.
  A blob that references a kernel symbol's absolute address would fetch-fault (or
  #PF on the data access) for the wrong reason.
- **Data segment registers at iretq.** In long mode DS/ES/FS/GS are treated flat
  and their selectors largely ignored; leaving them 0 (null) is generally fine,
  but note it as a thing to confirm in GDB rather than assume.

### 3.5 Verification plan (working vs. accidentally working)

Deterministic, over serial, no keyboard — asserted in CI like every prior
milestone.

1. **The syscall round-trip, from a proven CPL 3.** The `SYS_PRINT` handler
   asserts `ctx.cs & 3 == 3` and prints the char + CS. Success serial:
   `[m13] syscall from ring 3: SYS_PRINT 'Z' (CS=0x1b, CPL=3)`. The CS check is
   what distinguishes "ran in ring 3" from "accidentally still ring 0" (Q4).
2. **The enforcement gate (the headline).** The violation blob runs `cli` (→ #GP)
   *and*, in a second pass, reads `0xb8000` (→ #PF, error-code U bit set). Both
   land in `interrupt_dispatch` with `ctx.cs & 3 == 3`; the handler prints
   `[m13] blocked ring-3 violation: #GP at RIP=0x40000005 CS=0x1b (CPL=3)`. If
   *either* the CPU let `cli` through (no #GP) or the CS shows CPL 0, the boundary
   isn't real — FAIL. This is the office-hours Q4 gate.
3. **Clean return to ring 0.** After the excursion, `kernel_main` continues its
   normal init (heap/scheduler/shell come up) — proof the round-trip left the
   kernel's GDT/IDT/CR3/stack intact and didn't wedge on the RSP0 stack.
4. **Marker** `M13: userspace online`; bump the Makefile `MILESTONE_MARKER` from
   M12.

**GDB plan (`make debug`), tied to Q5:**
- Break just after the launch `iretq`: `info registers` → **CS must read 0x1b**,
  SS 0x23, RIP = UVA_CODE. (If CS is 0x08, the frame/DPL wiring is wrong.)
- Examine the **TSS**: dump its bytes, confirm **RSP0** points at the top of the
  dedicated kernel stack and is 16-aligned; confirm `tr` (task register) = 0x28.
- Dump the **GDT descriptors** and check the DPL nibble: u-code/u-data must be
  DPL 3, the TSS descriptor type 0x9 present.
- On the violation, read the **fault frame's CS** on the RSP0 stack — CPL 3.
- QEMU `-d int,cpu_reset` if it triple-faults on entry, to see #GP→#DF→reset
  before the reboot swallows the evidence.

**Honest unknowns (to resolve in GDB during the build):**
- **Does #DF (and maybe #GP/#PF) need an IST before this is safe?** Today all
  gates are IST=0, so a fault on the ring3→ring0 transition reuses whatever stack
  the CPU picked. If RSP0 is ever momentarily bad, that fault can't get a stack →
  #DF → triple fault. Leaning toward giving **#DF an IST1** stack (a second static
  16 KiB region in the TSS) as a safety net before flipping the `0x80` gate to
  DPL 3 — but will confirm whether it's *necessary* by testing the happy path
  first and watching for a #DF in `-d int`.
- **Whether intermediate tables truly need U/S=1** (§3.1d) — 99% yes per the SDM;
  the failing case (blob #PFs on first fetch) will confirm immediately if wrong.
- **DS/ES/FS/GS at `iretq`** — confirm null is fine in long mode, else load 0x23.
- **RFLAGS on entry** — enter with IF=0 (0x2) first to keep the excursion
  non-preemptible while debugging; flip to IF=1 (0x202) only once the round-trip
  is solid, and note that a ring-3 timer tick has no save/restore path yet
  (that's the scheduler-integration deferral).

---

## 4. Build order (each its own small commit, QEMU-tested)

1. **GDT + TSS in Rust** (`src/gdt.rs`). Build the five-entry GDT, `lgdt`, reload
   CS via `retfq`, load data selectors, define the TSS, set RSP0 to a dedicated
   kernel stack, `ltr(0x28)`. No ring-3 yet — assert in GDB that `tr`=0x28 and the
   DPLs are right, and that the kernel still boots through every existing
   self-test unchanged (proves the GDT swap didn't break ring 0).
2. **`USER` paging flag + `map_user_page`.** Add `USER`; thread it into created
   intermediates for the user subtree. Extend `paging::self_test` (or a small new
   assertion) to map a USER page above 1 GiB and confirm the walk sets U/S at
   every level.
3. **DPL-3 `0x80` gate + syscall dispatch.** Parameterize `IdtEntry::set_handler`
   (or add `set_user_handler`) so vector `0x80` gets `type_attr` 0xEE; add the
   `0x80` arm in `interrupt_dispatch` (`SYS_PRINT`, `SYS_EXIT`), with the
   `ctx.cs & 3 == 3` assert. Not reachable from ring 3 yet.
4. **`src/usermode.rs` — the ring-3 excursion.** The PIC blob, the code/stack
   mapping, the `iretq` launch (a tiny asm helper or inline `asm!`), and
   `self_test`: the SYS_PRINT round-trip. Emit `M13: userspace online`; bump the
   Makefile marker. This is the milestone's "done."
5. **The enforcement test.** Second blob (`cli`, then a kernel-page read); confirm
   #GP/#PF caught with CPL=3. This is the Q4 gate and the M15 preview.
6. **Docs + web + loop close.** `docs/concepts/privilege.md` (rings, the iretq
   frame, the TSS/RSP0, U/S); the milestone blog post (copy
   `docs/MILESTONE_CHECKLIST.md`); a `web/` step (predict: "what happens when
   ring 3 runs `cli`?" → observe the #GP → explain). Then `/kernel-review`
   (focus: the iretq frame, RSP0, the U/S-at-every-level walk, the `unsafe` in the
   launch) → `/retro` → `/document-milestone`. **`/red-team` now applies** — a real
   privilege boundary exists — run it, or defer to M15 with a one-line note (the
   deliberate-violation test is already the minimal red-team pass).

---

## 5. How M13 sets up M15 (the security milestone)

M13 *is* the boundary M15 attacks (PLAN §8, "Privilege boundary (M15)"). This cut
already ships the minimal version of M15's exercise — the deliberate `cli` /
kernel-page-read caught with CPL=3. M15 turns that into a **flag capture**: plant
a secret in a kernel-only (U/S=0) page and have a ring-3 blob try, in earnest, to
read it, execute a privileged instruction, or **abuse the syscall boundary** —
the "confused deputy" surface that appears the moment `SYS_PRINT` takes a *user
pointer* instead of a register char (the deferral in §2). So the two natural M13→
M15 seams to leave clean and documented:
- The syscall handler's argument path — register-only in M13; the first user
  *pointer* it accepts is where copy-from-user validation (range-check against the
  kernel region, confirm mapped, bounded length) must live, and where M15's
  fuzzing (out-of-range pointers, huge lengths, unmapped addresses) will probe.
- The U/S=0 identity map — already the thing that stops a ring-3 kernel read;
  M15 plants the flag behind it and proves the page permission holds (or finds
  where it doesn't).

Record in STATUS/PLAN that reaching M13 opens the security track; M14
(networking stub) and M15/M16 (security) can then proceed in any order.

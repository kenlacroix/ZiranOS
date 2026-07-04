# Milestone 15 — Break the privilege boundary (flag capture): eng-plan

*Think phase done: `milestone-office-hours` (six forcing questions answered
below) + `scope-guard` (**HOLD**; the `CR4.UMIP` demo **Reduced** to a
flip-and-revert learning function, never a permanent mitigation).*

## The one-sentence learning goal (office-hours Q1)

The CPU's ring/page boundary protects against **unauthorized access** but not
against **authorized misuse**: the instant the kernel dereferences a
user-supplied pointer with ring-0 privilege, the hardware boundary is silent,
and a software `copy_from_user` check is the *only* thing between a syscall and a
full kernel-memory read primitive. The confused deputy is a boundary the CPU
**cannot** enforce for you.

M13 already proved the *hardware* half (a ring-3 direct read of a kernel page
#PFs — red-team probe 8). M15 is about the *software* half the CPU leaves to you.

## Done, as observable serial behavior (Q2/Q4)

One boot, four asserted outcomes, then a `M15:` marker CI greps for:

1. **Direct read HELD.** Ring-3 `mov rax,[SECRET_VA]` → `#PF err=0x5` (U/S set),
   caught at CPL 3. The flag is never read. *(Reuses the M13 `BLOB_READ_KERNEL`
   harness, pointed at `SECRET_VA`.)*
2. **Confused deputy LEAKS → flag captured.** Ring-3 `SYS_WRITE_UNCHECKED(SECRET_VA, len)`
   makes the kernel read the kernel-only page on the caller's behalf and print
   it. The self-test asserts the recorded bytes **equal the planted `SECRET`
   constant** — a real capture, not incidental garbage.
3. **Validated deputy HELD.** The same call as `SYS_WRITE(SECRET_VA, len)`
   returns `-1` and prints nothing; the validator rejected `SECRET_VA`
   *specifically because its page is U/S=0* (proven by a page-walk assert, not by
   a blanket denial).
4. **Validated deputy still WORKS on a legit pointer.** `SYS_WRITE(user_buf, len)`
   over a genuine U/S=1 user buffer prints the bytes and returns `len`. This is
   the anti-"accidentally working" guard: a validator that rejected everything
   would pass test 3 while being useless.

Optional 5th (scope-Reduced): `sgdt` from ring 3 → NO FAULT (leak); set
`CR4.UMIP`; `sgdt` → `#GP`; **clear `CR4.UMIP` again** (kernel stays unhardened
per the non-goal). Purely "watch the mitigation land," cut entirely if it risks
the core.

## Minimal cut (Q3)

The confused-deputy pair (tests 2 vs 3) is irreducible — that's the lesson the
CPU can't teach. Test 1 is nearly free (existing harness). Test 4 is required for
rigor. `SYS_WRITE` handles a **flat byte buffer only** — no iovec, no
strings-from-user, no writeback. UMIP is out of the minimal cut.

## Approach

- **The secret.** `const SECRET: &[u8] = b"FLAG{ring3-cannot-read-kernel-memory}";`
  planted on a dedicated **U/S=0** page at `SECRET_VA = 0x6000_0000` (1.5 GiB):
  `frame = frame_allocator::alloc()`, `paging::map_page(SECRET_VA, frame, WRITABLE)`
  (`map_page`, **not** `map_user_page`, guarantees the leaf is U/S=0), then the
  kernel writes `SECRET` through `SECRET_VA`. A fixed VA (vs. a Rust `static`'s
  link-time address) lets the ring-3 blob hardcode `0x6000_0000` as an `imm32`,
  keeping it a static position-independent blob exactly like M13's.
  - *Why 1.5 GiB works:* it's above the heap (`0x4000_0000`..`0x4010_0000`) and
    above the 1 GiB huge-page identity window (so `map_page` won't hit
    `HugePage`). It shares `PML4[0]` and `PDPT[1]` with the M13 user pages —
    which `map_user_page` permanently loosened to U/S=1 — but the secret's own
    **PD entry, PT entry, and leaf are created by `map_page` as U/S=0**, and the
    hardware ANDs U/S down the walk, so the page stays kernel-only. A boot assert
    (`!paging::user_range_ok(SECRET_VA, 1)`) proves it.

- **The validator (new `pub` in `src/paging.rs`).**
  `pub fn user_range_ok(uaddr: u64, len: u64) -> bool` — the copy-from-user
  contract. Returns `true` iff **every** 4 KiB page in `[uaddr, uaddr+len)` is
  `PRESENT | USER` per a page-table walk that **never dereferences the memory**:
  - `len == 0` → `true` (nothing to read).
  - `end = uaddr.checked_add(len)` — overflow → `false` (rejects the huge-length
    / wraparound attack).
  - For each page, an internal `page_user_accessible(virt)` walks
    PML4→PDPT→PD→PT, requiring `PRESENT & USER` at each level, terminating at a
    `HUGE` entry (1 GiB at PDPT / 2 MiB at PD) as the governing leaf. Uses the
    existing private `read_cr3`/`read_entry`/`table_index`/`ADDR_MASK` — which is
    why it must live in `paging.rs`.
  - **Never touches the page** to test it: this is the footgun the office-hours
    named — a validator that dereferences the pointer *is* the confused deputy.

- **The syscalls (new arms in `src/interrupts.rs::syscall`).** ABI: number in
  `rax`, `rdi = user ptr`, `rsi = len`; returns bytes-written in `rax`, or
  `u64::MAX` (-1) on rejection.
  - `SYS_WRITE = 3` — the correct deputy. Reject if `len > MAX_WRITE` (256) or
    `!user_range_ok(rdi, rsi)`. Otherwise copy `len` bytes into a bounded kernel
    bounce buffer (validated ⇒ the copy is infallible), print to VGA+serial,
    record into `LAST_WRITE` for the test assert, return `len`.
  - `SYS_WRITE_UNCHECKED = 4` — the confused deputy *with the check omitted*, the
    deliberate teaching device (labeled like M11's loose extent check; must never
    exist in a hardened kernel). Same as above but **skips `user_range_ok`** —
    dereferences whatever `rdi` points at with ring-0 power, so `SECRET_VA` leaks.
    A bounded `len` still caps the read.

- **The blobs + harness (`src/usermode.rs`, `boot/usermode.asm`).** Three new
  static blobs (imm32 moves + `int 0x80`, position-independent):
  - `BLOB_READ_SECRET` — `mov rax,[SECRET_VA]; jmp $` → #PF (reuses `run_violation`,
    expect vector 14).
  - `BLOB_LEAK` — `mov eax,4; mov edi,SECRET_VA; mov esi,len; int 0x80; mov eax,2; int 0x80`.
  - `BLOB_WRITE` — same with `mov eax,3`, run twice: once with `edi=SECRET_VA`
    (rejected), once with `edi=USER_BUF_VA` (accepted). The kernel pre-writes a
    known string into the user page at `USER_BUF_VA` before the excursion.
  - Generalize M13's hardcoded `self_test` into `run_user_blob(blob) -> exit` so
    the clean-exit excursion (map → `save_and_disable` → `usermode_enter` →
    `restore` → assert `LAST_VIOLATION==0`) is reused, not copy-pasted.

## Preconditions (machine state)

- Runs late in `kernel_main`, **after** `gdt::init` / `interrupts::init` /
  `paging::init` / heap — same point as the M13 excursions (`src/lib.rs` ~233).
- Long mode, kernel CR3 active, IDT loaded with the DPL-3 `int 0x80` gate.
- Each excursion runs inside `interrupts::save_and_disable()` … `restore()`
  (IF=0): ring 3 has no scheduler save/restore yet, so the timer must not fire
  mid-excursion. The syscall (interrupt gate) also runs IF=0.
- `user_range_ok` assumes the walked tables are the live CR3 tree and
  identity-mapped (they are — `read_entry` reads through the identity map).

## Data flow

```
ring3 blob:  int 0x80  (rax=3, rdi=ptr, rsi=len)
  -> isr stub (boot/isr.asm) saves InterruptContext, IF=0
  -> interrupt_dispatch -> syscall(ctx)              [src/interrupts.rs]
       SYS_WRITE arm:
         len>MAX or !paging::user_range_ok(ptr,len)  --> ctx.rax = -1, return
             |                                             (no dereference)
         else copy ptr..ptr+len -> bounce buf        --> print + record + rax=len
  -> iretq back to ring3 (or, on the leak, the flag is already on the console)
```

## Edge cases

- `ptr + len` overflow / wraparound → `checked_add` → reject.
- `len == 0` → accept, no-op (write nothing).
- `len > MAX_WRITE` → reject (bounded read; the "huge length" red-team attack).
- Range straddles a page boundary where page A is U/S=1 but page B is U/S=0 →
  per-page walk rejects (the classic partial-validity confused-deputy bug).
- `SECRET_VA` sits under U/S=1 `PML4[0]`/`PDPT[1]` but a U/S=0 PD/leaf → the walk
  must check **every** level, not just the leaf, or it would wrongly accept.
- Re-entrancy: validate fully **before** copying one byte, so the copy can't fault
  on a page that passed validation (infallible-by-construction).

## Verification plan (proving, not observing)

- **Serial asserts** for all four outcomes above; the capture test asserts
  `LAST_WRITE == SECRET` (byte-equality — the "working vs accidentally working"
  gate from Q4). The block test asserts `rax == -1` **and** `LAST_WRITE` unchanged
  **and** `!user_range_ok(SECRET_VA,1)` with a U/S=0 walk.
- **`M15:` marker** asserted by CI (`Makefile` `run-headless` grep), same pattern
  as M11–M13.
- **GDB spot-check** (if anything is subtle): break in `syscall`, `x/4xb SECRET_VA`
  to confirm the flag bytes, and dump the PD entry for `0x6000_0000` to confirm
  `USER` (bit 2) is clear.
- **Named stall risks (Q5):** (a) a `copy_from_user` that dereferences to test →
  *is* the deputy — mitigated by walking tables only; (b) a fault mid-copy
  corrupting the IF=0 excursion frame — mitigated by validate-then-infallible-copy.
  Reach for `/investigate` + `make debug` if a walk faults.

## Red-team note

M15 *is* a red-team milestone. Per the M13 retro ("write the attack first"), the
blobs above are the acceptance tests the implementation is built against, aimed
at the two seams the M13 red-team named: the user-pointer/copy-from-user path
(tests 2–4) and the `sgdt`/`sidt` leak (optional test 5). A full `/red-team` pass
runs after the build lands.

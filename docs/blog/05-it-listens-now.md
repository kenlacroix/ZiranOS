# It listens now

*Milestone 5 — PS/2 keyboard input. The first time the machine reacts to the
outside world instead of just talking at it.*

> New to this? Read [docs/concepts/keyboard-and-pic.md](../concepts/keyboard-and-pic.md)
> alongside — it explains the PIC, IRQs, and scancodes from scratch.

Milestone 4 handled *exceptions* — faults the running code causes itself. This
milestone handles the other kind: a *hardware interrupt*, fired by a device
that has no relationship to whatever instruction the CPU happens to be on. It's
also the milestone where we enable interrupts (`sti`) for the very first time.

### Milestone 5 — keyboard — checklist

Think
- [x] office-hours — below
- [x] scope-guard — below

Plan
- [x] eng-plan — below

Build
- [x] Working, demoable state; `make` clean; header check passes
- [x] No new warnings; new `unsafe`/asm justified

Review
- [x] kernel-review over the diff

Security
- [x] No new attack surface (still ring 0, no data boundary) — /red-team N/A

Reflect
- [x] document-milestone (STATUS/README/this post)
- [ ] CI green (build + headless QEMU boot) — runs on push

## Office hours

1. **The one thing I'll understand:** how a hardware device (keyboard) gets the
   CPU's attention through the interrupt controller (PIC), versus the CPU-caused
   exceptions of M4.
2. **"Done" as observable behavior:** typing in the QEMU window echoes the
   characters to the screen, live.
3. **Smallest version that teaches it:** unmask only the keyboard IRQ (mask the
   timer and everything else), US layout, echo printable keys + Enter/Backspace.
   No key repeat handling, no full keymap.
4. **Working vs. accidentally working:** the echo must be *interrupt-driven* —
   the CPU sits in a halt loop and only wakes to a keystroke. If I were polling
   the port in a loop I'd "see" keys too, but that wouldn't be the lesson. The
   halt loop makes it honest: nothing happens until an interrupt fires.
5. **What could stall this for days:** forgetting to send the PIC an
   end-of-interrupt (EOI) — after which it never delivers another IRQ and the
   keyboard "dies" after exactly one key. Also: not reading port 0x60, which
   leaves the keyboard controller unable to send the next byte.
6. **Scope check:** below.

## Scope-guard

Verdict: **Hold**, with one **Reduce**. Keyboard input is core (the shell needs
it). The reduction: unshifted + shift only, US QWERTY, common keys — no Caps
Lock LED, no alternate layouts, no key-repeat tuning. Those are polish, not
lesson, and can come with the shell if ever wanted.

## Eng-plan

**Approach.** Three small modules:
- `src/pic.rs` — remap the two 8259 PICs so their IRQs land at vectors
  0x20–0x2F instead of colliding with the CPU exception vectors 0–31, then mask
  everything except the keyboard line (IRQ1), and expose `send_eoi`.
- `src/keyboard.rs` — read a scancode from port 0x60, track shift state, and
  translate scancode-set-1 make codes to characters.
- `src/port.rs` — shared `inb`/`outb` (factored out of `serial.rs`, which now
  reuses them).

Then wire vector 0x21 (0x20 + IRQ1) in `interrupt_dispatch` to the keyboard
handler + EOI, and in `kernel_main` call `pic::init()` and finally `sti`.

**Machine-state preconditions.** The IDT from M4 is loaded (its 256 stubs
already cover 0x21). Interrupts are enabled *only after* the PIC is remapped and
masked — enabling them earlier, while IRQs still point at exception vectors,
would misfire a stray IRQ into a fault handler.

**Edge cases.** Break codes (key release, make | 0x80) must be ignored except
for shift; the shift make/break codes (0x2A/0x36, 0xAA/0xB6) update state; keys
with no printable mapping return nothing; EOI must be sent on every keyboard IRQ
or delivery stops; interrupt gates keep IF cleared during the handler so no
nested keyboard IRQ can reenter the writer lock.

**Verification.** (1) assembles/compiles/links clean, header intact. (2) In
QEMU: the halt loop idles, and typing echoes characters, Enter starts a new
line, Backspace erases. (2) exercised by CI's boot (which at minimum proves the
`sti` path doesn't immediately fault).

## What got built

- `src/port.rs`, `src/pic.rs`, `src/keyboard.rs` as planned; `serial.rs`
  refactored onto the shared port helpers.
- `interrupts.rs` gains a keyboard arm (read scancode → echo → `send_eoi`); the
  fatal catch-all still guards every other unexpected vector.
- `vga_buffer.rs` gains backspace handling so erasing looks right.
- `kernel_main` enables interrupts and drops into the halt loop, now genuinely
  waiting on the world.

## Verification status (honest)

Assembles, compiles, and links clean. The live typing is exercised in QEMU / by
CI, not yet on real hardware; when a real keystroke first echoes (or doesn't),
the debugging story lands here.

## Takeaway for the next person

An exception is the CPU tapping *itself* on the shoulder; a hardware interrupt is
someone *else* doing it, at a moment you don't control. The PIC is the
receptionist deciding which taps get through and in what order — and the EOI is
you telling the receptionist "okay, send me the next one." Forget that one
courtesy and everything goes quiet after a single keypress.

# The keyboard, the PIC, and hardware interrupts

*Companion to Milestone 5. Builds directly on
[interrupts.md](interrupts.md) — read that first if "IDT" or "vector" is new.*

---

## Two kinds of interrupt

Milestone 4 handled **exceptions**: the CPU interrupting *itself* because the
current instruction went wrong (divide by zero, bad memory access). This
milestone handles the other kind: a **hardware interrupt** (IRQ, "interrupt
request"), raised by a *device* — the keyboard — that has nothing to do with
whatever instruction the CPU is running. It can arrive at literally any moment.

That "any moment" is why hardware interrupts were kept *off* until now. We
enable them, for the first time, with a single instruction: `sti` (set interrupt
flag). Before we dare do that, two things must be true, and getting them wrong is
the whole difficulty of this milestone.

## The PIC: a receptionist for interrupts

Devices don't wire straight to the CPU. They raise a line on the **PIC**
(Programmable Interrupt Controller, the classic Intel 8259). A PC has two of
them chained — a "master" and a "slave" — giving 15 numbered lines called IRQs.
The keyboard is **IRQ1**. The timer is IRQ0. The slave hangs off the master's
IRQ2.

The PIC's job is to take these device signals, decide priority, and hand the CPU
one interrupt at a time along with a **vector number** — the same 0–255 index
into the IDT that exceptions use.

### Problem 1: the vectors collide, so we *remap*

Out of reset, the PIC delivers IRQ0–7 as vectors **0x08–0x0F**. But the CPU has
already claimed vectors **0–31** for exceptions! Vector 0x08 is the *double
fault*. So a timer tick (IRQ0) would arrive looking exactly like a double
fault, and the keyboard (IRQ1, vector 0x09) would look like a coprocessor
error. Chaos.

The fix is to **remap** the PIC: reprogram it so its IRQs come in at vectors
**0x20–0x2F** instead — safely above the 0–31 exception range. IRQ1 (keyboard)
then arrives as vector `0x20 + 1 = 0x21`, and our IDT entry 0x21 (one of the 256
stubs from Milestone 4, already in place) catches it.

Remapping is done by writing four "Initialisation Command Words" (ICW1–4) to the
PIC in a fixed sequence — that's the dance in `src/pic.rs::init`. On real
hardware the chip can need a beat between writes, so we insert a tiny `io_wait`
delay; QEMU doesn't care, but real silicon sometimes does.

### Problem 2: don't open every door at once, so we *mask*

Each PIC line can be individually **masked** (disabled). If we unmasked
everything and hit `sti`, the timer would immediately start firing dozens of
times a second into a handler we haven't written. So we mask everything except
IRQ1. The keyboard is the only device allowed to speak this milestone; the timer
comes online in a later one.

## The EOI: "okay, send me the next one"

Here's the courtesy that trips everyone up. When the PIC delivers an interrupt,
it marks that line "in service" and **will not deliver another until you tell it
you're done**. That acknowledgement is the **End Of Interrupt (EOI)** — a
specific byte written to the PIC's command port at the end of the handler.

Forget the EOI, and the symptom is unmistakable: the keyboard works for *exactly
one keypress*, then goes silent forever. The PIC is still waiting for you to
acknowledge the first one. (For an IRQ handled by the slave PIC you must EOI
both chips; the keyboard is on the master, so one EOI suffices.)

## Scancodes: what the keyboard actually sends

The keyboard doesn't send letters. It sends **scancodes** — small numbers
identifying *physical keys*, made available at I/O port **0x60**. The default
encoding is "scancode set 1":

- Press a key → a **make code** (e.g. `0x1E` for the A key).
- Release it → a **break code**, the make code with the high bit set (`0x9E`).

So a single "type an A" is two interrupts: press (0x1E) and release (0x9E). We
act on presses and ignore releases — *except* for Shift, whose press and release
we track so we can choose 'a' vs 'A'. Turning a scancode into a character is a
lookup table (`src/keyboard.rs::translate`), one per layout; ours is US QWERTY.

Two things you must always do when the keyboard interrupt fires, in order:

1. **Read port 0x60.** This both gives you the scancode *and* frees the keyboard
   controller to send the next byte. Skip it and the controller jams.
2. **Send the EOI** to the PIC (see above).

## Putting it together: the life of one keystroke

```
you press 'k'
  -> keyboard controller puts 0x25 at port 0x60, raises IRQ1
  -> PIC (IRQ1 unmasked) delivers vector 0x21 to the CPU
  -> CPU looks up IDT[0x21] -> the M4 assembly stub -> interrupt_dispatch
  -> dispatch: keyboard::handle_interrupt() reads 0x60, returns 'k'
             : print!("k") echoes it to the screen
             : pic::send_eoi(1) acknowledges, so the next key can come
  -> iretq resumes exactly where the CPU was (our halt loop)
  -> CPU halts again, waiting for the next interrupt
```

That halt loop is the honest proof this is interrupt-*driven*: the CPU does
nothing between keystrokes. It isn't spinning asking "any key yet? any key yet?"
— it's asleep until the hardware wakes it. That's how a real OS idles.

## Why no lock deadlock (this time)

The handler prints, which takes the VGA writer's spinlock — the reentrancy
hazard flagged back in Milestone 4. It's safe here because our IDT entries are
*interrupt gates*, which clear the interrupt flag on entry: a keyboard interrupt
cannot interrupt its own handler. The next keystroke waits until `iretq`
restores the flag, so the lock is always free again by then. (If the main loop
ever printed while interrupts were on, the hazard would return — which is why a
lock-free emergency writer is still on the eventual to-do list.)

## Where this goes next

- **Milestone 6+** unmask the timer (IRQ0), and a periodic tick becomes the
  heartbeat a scheduler needs.
- The characters we're echoing will feed a line buffer and become the **shell**
  (Milestone 10) — at which point "it listens" turns into "it obeys."

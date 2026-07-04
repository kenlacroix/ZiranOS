//! Networking stub — Milestone 14, "hello over a socket."
//!
//! The whole roadmap named networking a **non-goal** — no stack, no protocols,
//! no real hardware — with exactly one carve-out: a *stub* that shows what a
//! socket is underneath the abstraction. This module is that stub, and nothing
//! more. There is no IP, no TCP/UDP, no ports, no checksums, no ARP, no routing,
//! no device driver, and no wire; QEMU is booted with no network at all. See
//! `docs/planning/milestone-14-eng-plan.md` for the scope-guard reasoning.
//!
//! The one lesson: **a socket is a named endpoint with a receive queue, reached
//! by *address*, decoupled from whoever is on the other end.** Three layers that
//! a direct function call collapses but a network keeps separate:
//!
//! ```text
//!   socket layer:   sock.send(to, bytes)  -- just queues a frame on the device
//!   loopback device: net::poll()          -- moves TX frames to the dst's RX queue
//!   socket layer:   sock.recv()           -- pops a frame off *its* RX queue
//! ```
//!
//! The two queues — the device's transmit queue and each socket's receive queue —
//! are the point. A `send` does not touch the receiver; the *device*, when polled,
//! routes the frame by its destination address. So `recv` returns `None` until
//! `poll` runs (the frame is in flight), delivery reaches only the socket bound to
//! `dst` (not a broadcast), and a frame for an unbound address is silently dropped
//! (a real NIC doesn't know the peer is gone) — the behaviours that separate
//! "modeled addressed delivery" from "echoed a buffer."
//!
//! "Loopback" means the device's transmit path *is* its receive path: what you
//! send comes back in on the same interface, routed to whichever local socket owns
//! the destination address. That is the one interface where "the network" is
//! unambiguously just a buffer — no peer required to demonstrate the abstraction.
//!
//! Re-entrancy: `NET` is a plain `spin::Mutex` (not IF-guarded like `SCHED`)
//! because nothing here is ever called from an interrupt handler — only from tasks
//! and the boot self-test. A future "deliver on a timer IRQ" change must revisit
//! that.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;
use spin::Mutex;

/// Maximum payload a single frame may carry, in bytes. The *only* nod to real
/// framing — it teaches "a frame has a bounded size" and caps a stray allocation,
/// without being a protocol. (1500 is Ethernet's classic MTU; the value is a wink,
/// not a standard we implement.)
pub const MTU: usize = 1500;

/// One frame on the wire-that-isn't: a source and destination address (bare
/// `u16`s — no IP, no ports, just "who from / who to") and an opaque payload. The
/// frame carries `src` so the receiver learns who sent it, exactly as delivery
/// preserves addressing on a real link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub src: u16,
    pub dst: u16,
    pub payload: Vec<u8>,
}

/// Why a socket call failed. A no-route (delivering to an unbound address) is
/// deliberately **not** here: like a real NIC, `send` succeeds and the frame is
/// silently dropped at delivery — surfaced only as [`dropped`]'s counter, not as a
/// sender-visible error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetError {
    /// [`Socket::bind`] was given an address already bound.
    AddrInUse,
    /// [`Socket::send`] was given a payload larger than [`MTU`].
    TooLong,
}

/// The loopback interface and its socket table — the whole "network," behind one
/// lock. `tx` is the device's transmit queue (frames a socket has sent but the
/// device has not yet delivered); `sockets` maps each bound address to its receive
/// queue; `dropped` counts frames delivered to no one.
///
/// No queue-depth bound: [`MTU`] caps a single frame, but `tx` and each RX queue
/// grow unbounded if a caller `send`s without `poll`ing or lets `recv` lag — a stub
/// with disciplined callers (the demo always polls and drains immediately) rather
/// than a NIC with real backpressure. Adding a ring-buffer cap would be the first
/// step toward a real driver, deliberately out of scope here.
struct Net {
    tx: VecDeque<Frame>,
    sockets: BTreeMap<u16, VecDeque<Frame>>,
    dropped: u64,
}

impl Net {
    const fn new() -> Self {
        Net {
            tx: VecDeque::new(),
            sockets: BTreeMap::new(),
            dropped: 0,
        }
    }
}

/// The one loopback interface. `None` until [`init`] installs it (after the heap is
/// up), mirroring the `SCHED` house pattern. A plain `spin::Mutex` — see the module
/// note on why no IF-guard is needed.
static NET: Mutex<Option<Net>> = Mutex::new(None);

/// Install the loopback interface. Call once, after the heap is initialized (the
/// socket queues allocate).
pub fn init() {
    *NET.lock() = Some(Net::new());
}

/// A bound endpoint: just its address. The receive queue lives in [`NET`]'s table,
/// so a `Socket` is a lightweight handle, not an owner — no `Arc`/`RefCell` needed.
///
/// **Handles must be explicitly [`close`](Socket::close)d.** There is deliberately
/// no `Drop` impl: a `Drop` that unbinds would have to lock `NET`, and dropping a
/// handle while `NET` is already held (a plausible future mistake) would deadlock a
/// non-reentrant spin lock — a worse footgun than the leak it prevents. So dropping
/// a `Socket` without closing it leaks its binding (the address stays `AddrInUse`
/// and its RX queue is retained). Every caller here closes on all paths; a future
/// caller using `?` between `bind` and `close` must close in the error path too.
#[derive(Debug)]
pub struct Socket(u16);

impl Socket {
    /// Bind `addr` and return a handle to it, or [`NetError::AddrInUse`] if some
    /// other socket already holds it. Registers an empty receive queue.
    pub fn bind(addr: u16) -> Result<Socket, NetError> {
        let mut guard = NET.lock();
        let net = guard.as_mut().expect("net: init() not called");
        if net.sockets.contains_key(&addr) {
            return Err(NetError::AddrInUse);
        }
        net.sockets.insert(addr, VecDeque::new());
        Ok(Socket(addr))
    }

    /// Send `bytes` to address `to`. Builds a [`Frame`] and hands it to the
    /// device's transmit queue — it is **not** delivered until [`poll`] runs.
    /// Fails only with [`NetError::TooLong`] (`> MTU`); an unroutable `to` is not a
    /// send-time error (it is dropped at delivery, like a real link).
    pub fn send(&self, to: u16, bytes: &[u8]) -> Result<(), NetError> {
        if bytes.len() > MTU {
            return Err(NetError::TooLong);
        }
        let frame = Frame {
            src: self.0,
            dst: to,
            payload: bytes.to_vec(),
        };
        let mut guard = NET.lock();
        guard.as_mut().expect("net: init() not called").tx.push_back(frame);
        Ok(())
    }

    /// Pop the next frame off this socket's receive queue, or `None` if nothing has
    /// been delivered yet (a would-block — the queue is real, not a latched value).
    pub fn recv(&self) -> Option<Frame> {
        let mut guard = NET.lock();
        let net = guard.as_mut().expect("net: init() not called");
        net.sockets.get_mut(&self.0)?.pop_front()
    }

    /// Unbind this socket, discarding any undelivered frames on its receive queue.
    /// Consumes the handle so the address can't be used after release.
    pub fn close(self) {
        let mut guard = NET.lock();
        if let Some(net) = guard.as_mut() {
            net.sockets.remove(&self.0);
        }
    }
}

/// Run the loopback device one step: drain the transmit queue, routing each frame
/// to the receive queue of the socket bound to its `dst`, or dropping it (bumping
/// the drop counter) if no socket owns that address. Returns the number delivered.
/// This is where "loopback" happens — transmit becomes receive, routed by address.
pub fn poll() -> usize {
    let mut guard = NET.lock();
    let net = guard.as_mut().expect("net: init() not called");
    let mut delivered = 0;
    while let Some(frame) = net.tx.pop_front() {
        match net.sockets.get_mut(&frame.dst) {
            Some(rx) => {
                rx.push_back(frame);
                delivered += 1;
            }
            None => net.dropped += 1, // no socket owns dst — silently dropped, as on a real link
        }
    }
    delivered
}

/// Total frames the device has dropped for want of a bound destination — the
/// no-route counter. Read by the self-test and the `net` shell command.
pub fn dropped() -> u64 {
    NET.lock().as_ref().map(|n| n.dropped).unwrap_or(0)
}

/// Prove the loopback stub over serial, deterministically: send one frame from
/// address 1 to address 2 and get the exact bytes back, then assert the four
/// properties that make this *addressed delivery* rather than an echo — the frame
/// is in flight until the device polls, it reaches only the socket bound to the
/// destination, an unbound destination drops cleanly, and a drained queue blocks.
pub fn self_test() {
    let msg = b"hello from ziran";

    let rx = Socket::bind(2).expect("net: bind 2");
    let tx = Socket::bind(1).expect("net: bind 1");
    let other = Socket::bind(3).expect("net: bind 3");

    // Rebinding a live address is refused, not silently clobbered.
    assert_eq!(Socket::bind(1).err(), Some(NetError::AddrInUse), "net: double bind");

    tx.send(2, msg).expect("net: send 1->2");

    // The frame is on the device's TX queue, not yet delivered: recv blocks.
    assert!(rx.recv().is_none(), "net: frame delivered before poll -- the layers aren't separate");

    let delivered = poll();
    assert_eq!(delivered, 1, "net: poll should deliver exactly one frame");

    // Delivered to the destination, byte-exact, with addressing preserved.
    let frame = rx.recv().expect("net: nothing on addr 2 after poll");
    assert_eq!(frame.payload, msg, "net: payload corrupted in loopback");
    assert_eq!((frame.src, frame.dst), (1, 2), "net: addressing not preserved");

    // Delivery is BY ADDRESS: the socket bound to 3 saw nothing (not a broadcast).
    assert!(other.recv().is_none(), "net: frame leaked to the wrong address");

    // The queue is real: a second recv on the now-drained socket blocks.
    assert!(rx.recv().is_none(), "net: recv on a drained queue should block");

    // An unbound destination drops cleanly at poll -- no panic, counted as no-route.
    let before = dropped();
    tx.send(9, b"nobody home").expect("net: send 1->9");
    assert_eq!(poll(), 0, "net: nothing should be delivered to an unbound address");
    assert_eq!(dropped(), before + 1, "net: the unrouted frame should be counted dropped");

    // Oversized payloads are refused by the frame's MTU.
    assert_eq!(tx.send(2, &[0u8; MTU + 1]).err(), Some(NetError::TooLong), "net: MTU");

    // Leave NET clean for the `net` shell command.
    rx.close();
    tx.close();
    other.close();

    crate::serial_println!(
        "[ok] net: loopback delivered {:?} from addr 1 to addr 2 byte-exact; \
         wrong-address and unbound-destination frames did not arrive (1 dropped, no-route)",
        core::str::from_utf8(msg).unwrap_or("<non-utf8>")
    );
    crate::serial_println!("M14: loopback online");
}

/// Run one live loopback round-trip for the shell's `net` command: bind two
/// addresses, send a greeting, poll the device, receive it, and report. Cleans up
/// after itself so it can be run repeatedly.
pub fn demo() {
    const A: u16 = 1;
    const B: u16 = 2;
    let a = match Socket::bind(A) {
        Ok(s) => s,
        Err(_) => {
            crate::println!("net: loopback busy, try again");
            return;
        }
    };
    let b = match Socket::bind(B) {
        Ok(s) => s,
        Err(_) => {
            a.close();
            crate::println!("net: loopback busy, try again");
            return;
        }
    };

    let msg = b"hello from ziran";
    let _ = a.send(B, msg);
    let delivered = poll();
    match b.recv() {
        Some(frame) => crate::println!(
            "lo: {} -> {} {:?} ({} bytes, {} delivered)",
            frame.src,
            frame.dst,
            core::str::from_utf8(&frame.payload).unwrap_or("<binary>"),
            frame.payload.len(),
            delivered,
        ),
        None => crate::println!("net: nothing received (unexpected)"),
    }

    a.close();
    b.close();
}

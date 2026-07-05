# A CTF against my own kernel

*I already broke this filesystem in private. Tier 1 is me building a glass box
around the break so anyone can throw the switch themselves — paste a forged disk
image over a serial console in a browser tab and watch the kernel hand you a
secret it was built to hide.*

Back in Milestone 16 I proved something small and unsettling about my own
filesystem: a `FLAG{...}` sitting in the RAM disk with **no directory entry**
could still be read right out of it, if you handed the kernel a disk image you'd
crafted for the purpose. `ls` couldn't list the flag. `cat` couldn't name it.
And a twenty-five-byte lie in a superblock field walked it out the front door
anyway.

That capture lived in a self-test. It ran once, on my machine, and printed
`CAPTURED` to a serial log nobody else would ever see. This post is about the
next move: turning that private, one-shot proof into something you can *play* —
[a browser CTF at ziranos.pages.dev/ctf](https://ziranos.pages.dev/ctf), where
the whole exploit is yours to build, submit, and pull the flag out of a live
kernel booting in WebAssembly.

> **The lesson, up front: validating against a value the caller controls is not
> validation.** It's a receipt the attacker wrote themselves, stamped by a
> checker too polite to ask who filled in the number.

The controlling image for the whole thing is a **glass box**. Tier 1 is a firing
range enclosed in glass: the target is a real kernel, the shot is a real
exploit, and you can see everything — the ammunition, the mechanism, even the
flag itself if you press your face to the wall. That transparency isn't a
failure of the range. It *is* the range. You come to learn the shot, not to be
kept from the target.

## The bug, in the open

Tier 1 is white-box on purpose, so I can just tell you how it works.

ZranFS — the toy filesystem — reads files by extent: a directory entry says
"this file's bytes start at `offset` and run for `length`," and the reader hands
back that slice of the image. Milestone 11 already bounds-checked this. It made
sure every extent stayed *inside the image*. That sounds airtight until you say
it slowly: inside the image includes the region past all the real files, which
is exactly where I hid the flag.

Milestone 16's fix added one field to the superblock — `data_end`, the offset
where legitimate file data stops. The strict reader confines every extent to
`<= data_end`. The flag lives in `[data_end, total)`, past the ceiling, and an
extent that reaches for it fails with `ExtentEscapesData`. Real files still read
byte-exact. It's a good fix. It holds against navigation, against `ls`, against
`cat`, against every honest path to the bytes.

And it has a residual I documented rather than closed, because closing it is a
different project: **`data_end` is a field inside the image, and in this CTF you
supply the whole image.** So you forge it. You write a `data_end` that sits
*below* the flag, then craft a file whose extent aliases the hidden bytes, and
the strict reader — checking your extent against the very number you just chose —
nods it through. The ceiling is real. It's just painted on the floor by the
person walking in.

That's the transferable primitive, and it's why the flag capture is worth
playing rather than reading about: the checker isn't buggy in the usual sense. It
does precisely what it says. It compares one number against another. The whole
lesson lives in *where the second number came from*.

## Getting a disk image into a running kernel

Here's where the delivery engineering starts, and where the actual war story is,
because "let the user submit a crafted image" turned out to be three problems
wearing one coat.

**Problem one: the shell can't hold an image.** My first, naive plan was the
obvious one — type (well, paste) the base64 of your crafted image at the shell
prompt and let a command decode it. I wired up a `load` command, pasted a preset
image in, and got a mount error that made no sense: the decoder was choking on
base64 that looked, in the textarea, completely valid.

It took me the better part of an evening of squinting at the wrong layer — I was
sure the bug was in my hand-rolled decoder — before I noticed the input was
*truncated*. The shell's line editor is a fixed 128-byte buffer. A disk image is
kilobytes. The prompt had been silently eating everything past character 128 and
handing my decoder a base64 string sliced off mid-quantum. The decoder wasn't
wrong. It was being fed a fragment and asked to explain itself. The bug was one
layer below where I was looking, which is the oldest story in this whole
project.

The fix reframes what `load` *is*. It isn't a command that reads an argument
from the line editor — it can't be, the line editor is too small by two orders
of magnitude. Instead `load` **takes over the read loop**: once invoked, it
stops being a shell command and becomes a streaming sink, pulling bytes straight
off the serial console one at a time and appending them to the image buffer,
until it sees a lone `.` on its own line. The sentinel ends the paste. The line
editor never touches the payload; it only sees the word `load` and, later, the
dot.

That also solved a delivery quirk I hadn't anticipated: you can't dump the whole
base64 blob into the emulated serial port in one burst — the guest's input path
wants bytes fed to it promptly, not slammed in as a single wall. So the browser
side paces the stream, types `load\r`, waits a beat for `cmd_load` to reach its
read loop, feeds the base64, then sends `.\r`. It reads less like a file upload
and more like teaching someone to whistle a long tune one note at a time.

**Problem two: no base64 decoder exists.** This kernel is `no_std` on
`x86_64-unknown-none`. There is no `base64` crate, no `alloc`-friendly convenience,
no anything. So the decoder is hand-written, and — like every other byte-parsing
path in this filesystem — it is **total and panic-free**. It cannot be otherwise:
the entire point of the exercise is feeding a kernel hostile input, and a decoder
that panics on a malformed paste would just be a second, dumber vulnerability
sitting next to the one I'm trying to teach. It validates the alphabet, handles
partial quanta and padding without ever indexing out of bounds, and returns a
clean error instead of faulting. It's maybe forty lines and it is the least
glamorous, most load-bearing code in the whole demo.

**Problem three: knowing when you've won.** The flag never gets echoed as
plaintext — you paste *base64*, so the readable string never crosses the wire on
the way in. It only appears when the kernel prints it back out after a
successful leak. So the page watches the serial stream for the flag's byte
pattern, and when it matches, a **"🚩 Captured!"** banner fires. The capture
re-arms itself after each load, so you can try the honest image (which should be
*rejected* — `ExtentEscapesData`, the fix doing its job), then a forged one, and
watch the banner stay dark, then light up. That contrast is the entire point:
the range shows you both the shot that misses and the shot that lands.

## Why the flag is trivially findable, and why that's correct

Now the honest part, because "honest" is the value this whole project keeps
circling back to.

The Tier 1 flag is not secret. It lives in your own browser tab's memory. You
could open DevTools and find it in seconds. You could run `strings` on the kernel
blob. You do not need my exploit at all to read the string `FLAG{...}` — you need
it only to make the *kernel* read it, through the extent-aliasing path, the way a
real attacker would against a real boundary.

This is the glass box, and it's deliberate. A practice range where the target is
also hidden behind a wall isn't teaching you to shoot; it's teaching you to
resent the range. Tier 1 optimizes for one thing: that you understand the
*primitive* — forge the bound, alias the extent, walk the secret out — with
nothing between you and the mechanism. The flag being visible through the glass
is the cost of being able to see the mechanism clearly, and it's a cost worth
paying, because the mechanism is the whole lesson and the specific flag string is
worth nothing.

If you want the version where the flag is genuinely out of reach — where it lives
only in a server's RAM, injected fresh per session, and no amount of DevTools on
your own tab will show it to you — that's **Tier 2**, the remote CTF at
[ziranos.pages.dev/ctf-remote](https://ziranos.pages.dev/ctf-remote). It rests on
a different, subtler bug than Tier 1's forged ceiling — a length-confusion in the
reader, where the number describing how much to return and the number describing
how much is *allowed* drift apart. I'm going to say exactly that much and no
more. Tier 2 is a black box on purpose, and spelling out the crafted-image recipe
here would unlock a door I hung specifically so people could pick it themselves.
The concept is fair game; the combination stays in my pocket.

That split — a glass box to learn the primitive, a black box to test whether you
actually can — is the shape I'd want if I were on the other side of it.

## What I deferred, and what I haven't verified

In the spirit of not pretending a toy is hardened:

- **This is one vulnerability class, and a *logical* one.** The bug is a bounds
  check that trusts an attacker-supplied length — a classic out-of-bounds read
  ([CWE-125](https://cwe.mitre.org/data/definitions/125.html)) — but not the
  memory-unsafe kind: the kernel slices its backing buffer in safe Rust, so every
  read stays inside the allocation. It's information disclosure past a *logical*
  fence, not corruption or a smashed stack. One deliberately-planted path, defended
  against by the strict reader in the very same file — not a model of a hardened
  kernel's real attack surface, and modern kernels layer memory-safe languages,
  fuzzing, and mitigations precisely to keep this class out. For the field this toy
  only gestures at: [LangSec](http://langsec.org/) (parsing untrusted input as *the*
  boundary), [Project Zero](https://googleprojectzero.blogspot.com/) (the class in
  real production exploits), [pwn.college](https://pwn.college/) (hands-on, well past
  this one primitive).
- **I didn't close the forged-`data_end` residual, and I'm not going to.** In
  Tier 1 it's the *feature* — the whole challenge is that you control the field
  the check trusts. A filesystem that genuinely resisted this would need the
  bound to live somewhere the caller can't author (a trusted superblock, a
  signature, an out-of-band manifest), and that's a real filesystem's problem,
  squarely on the far side of this project's non-goals.
- **The pacing between the browser and the emulated serial port is tuned, not
  proven.** The stream waits a fixed beat for `cmd_load` to reach its read loop
  before feeding bytes. It's worked on every browser I've tried, but it's timing
  against an emulator, and timing against an emulator is a promise, not a
  guarantee. A slow enough machine could in principle race the sentinel. I
  haven't built a handshake that would make it deterministic; I've built one that
  works and left a note.
- **I've verified the honest image is rejected and forged images are captured,
  by playing it — not by a test matrix.** The kernel-side proof is Milestone
  16's self-test, which still runs and still asserts byte-equality with the
  planted constant. The *browser* delivery layer on top of it is checked the way
  a demo gets checked: by doing it, repeatedly, and watching the banner. That's
  weaker than the self-test underneath it, and I'd rather say so than imply the
  paste path has the same rigor as the parser it feeds.

## Play it

- **Tier 1 (this post, white-box practice):**
  [ziranos.pages.dev/ctf](https://ziranos.pages.dev/ctf) — start from the honest
  preset, confirm the fix *rejects* it, then forge a `data_end` and alias the
  flag's extent. The banner will tell you when the kernel hands it over.
- **Tier 2 (the real thing, black-box):**
  [ziranos.pages.dev/ctf-remote](https://ziranos.pages.dev/ctf-remote) — same
  family of bug, a flag that actually hides, no white-box crutches. If Tier 1
  taught you the primitive, Tier 2 asks whether you can aim it.

The build is openly AI-assisted — this runs on Claude Code the way the rest of
the project does — but the story here isn't the assistant. It's that a bounds
check I was quietly proud of turns out to be a receipt the attacker writes
themselves, and the most useful thing I could do with that fact was stop
narrating it and let people press the switch. A private `CAPTURED` in a log
teaches me one thing. A glass box on the public internet, with the mechanism lit
up and the flag sitting right there behind the glass, teaches the shot to
whoever wants it.

The wall I built to hide the flag was honest work. The most honest thing I could
do next was invite everyone to walk through it.

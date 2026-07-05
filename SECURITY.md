# Security policy

Ziran OS is a **hobbyist, broken-to-learn** operating system. The kernel is
**deliberately not hardened** — several "vulnerabilities" in it are the whole point
(they're teaching devices with a milestone and a blog post each). Please read this
before reporting anything.

## The kernel's insecurity is intentional — don't report it as a bug

These are **features**, not findings, and writeups about *exploiting* them are
welcome (see [`SOLVERS.md`](SOLVERS.md)):

- The Tier-1 / Tier-2 filesystem exploits (`Fs::mount_loose`, `Fs::mount_tier2`, the
  `data_end` / trusted-length confusion) — the CTF *is* landing these.
- The ring-3 confused-deputy syscall (`SYS_WRITE_UNCHECKED`), the planted `FLAG{…}`
  pages, `sgdt`/`sidt` info-leak (UMIP deliberately off), and any other in-kernel
  boundary that a milestone documents as broken.

No POSIX, no real security model, no multi-user, no real-hardware support — those are
[explicit non-goals](PLAN.md). "The OS has no ASLR / no W^X / trusts its own disk" is
by design. **Do not run this on real hardware or anything you care about.**

## The CTF: authorized, go for it

The live **Tier-2 remote capture** ([`ctf.kennethlacroix.me`](https://ziranos.pages.dev/ctf-remote.html))
is *meant* to be attacked. You have full ring-0 control of the guest kernel — that's
the challenge, not a breach. Landing the filesystem exploit to capture your session
flag is **explicitly in scope and encouraged**. Then
[open a writeup issue](https://github.com/kenlacroix/ZiranOS/issues/new?template=ctf-writeup.md).

## Please report these PRIVATELY (not a public issue)

The kernel is a toy; the **hosting around it is not part of the game**. If you find a
way to affect anything *outside* the intended challenge, please disclose it privately:

- **Escaping the QEMU guest** to the host VM (a real QEMU/hypervisor break).
- **Reaching the LAN** or any host beyond the CTF VM from inside a session.
- Compromising the **bridge** (`ctf-server/bridge.mjs`) — RCE, the Turnstile/limits
  bypass letting you exhaust the host, reading another session's flag, etc.
- Anything touching the **Cloudflare tunnel**, the deploy pipeline, or other users.

**How:** email **contact@kennethlacroix.me** with steps to reproduce and, if you can,
the impact. This is a solo hobby project, so there's **no bug bounty** and response is
best-effort — but good-faith reports are genuinely appreciated and I'll credit you if
you'd like.

## Good-faith safe harbor

Testing the **CTF challenge itself** against the live target is authorized. If you
stay within it — no DoS/flooding beyond what one honest session does, no attempts to
reach the host or other people's sessions, no data destruction — I won't consider it
hostile. If you accidentally cross a line, stop and email me; telling me is the
opposite of a problem.

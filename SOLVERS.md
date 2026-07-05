# Solvers — Ziran OS Filesystem CTF 🚩

A hall of fame for people who broke the boundary and wrote up how.

The challenge: a real from-scratch 64-bit kernel boots [in your browser](https://ziranos.pages.dev/ctf),
and its RAM disk hides a `FLAG{…}` in bytes with **no directory entry** — `ls` won't
list it, `cat` can't name it. You make the filesystem hand it to you anyway by
crafting a disk image whose own header lies about where its data ends. (See
[`docs/planning/ctf-eng-plan.md`](docs/planning/ctf-eng-plan.md) for the full design,
and [`docs/concepts/filesystem.md`](docs/concepts/filesystem.md) for the format.)

Two tiers: **[Tier 1](https://ziranos.pages.dev/ctf)** (in-browser, white-box practice)
and **[Tier 2](https://ziranos.pages.dev/ctf-remote.html)** (remote capture — see below).

## How to get listed

Solve it, then **[open a writeup issue](https://github.com/kenlacroix/ZiranOS/issues/new?template=ctf-writeup.md)**
describing how you did it. Once it's reviewed, your name lands here.

A writeup is what earns the spot — not the flag string. Tier 1 is a **white-box
sandbox**: the flag lives in your own browser tab, so `strings`/DevTools would find
it in seconds. That's fine and expected; the point is understanding and reproducing
the *exploit primitive* (a bounds check that trusts an attacker-controlled length),
not keeping a secret.

The uncheatable, server-side version is **[Tier 2](https://ziranos.pages.dev/ctf-remote.html)** —
now **live**. The kernel runs on a server; a unique `FLAG{ziran-tier2-…}` is minted per
session and lives **only in that instance's RAM** — never in the page, the repo, or any
bytes you're given. You develop the exploit against Tier 1, then land it here over the
wire. It's hardened against shortcuts: the flag's offset is **randomized per session**
(the kernel prints it on `load`, so a copied writeup misses — you must aim at your own
instance), submissions are **verified server-side by hash**, and a **honeytoken** decoy
tripwire flags anyone who submits a scraped flag instead of capturing one.

## Roll

| # | Solver | Tier | Writeup | Date |
|---|--------|------|---------|------|
| — | *be the first* | | | |

<!-- Add rows above this line, most recent first:
| 1 | [name](https://link) | 1 | #issue | YYYY-MM-DD |
-->

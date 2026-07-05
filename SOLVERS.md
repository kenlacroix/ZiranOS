# Solvers — Ziran OS Filesystem CTF 🚩

A hall of fame for people who broke the boundary and wrote up how.

The challenge: a real from-scratch 64-bit kernel boots [in your browser](https://ziranos.pages.dev/ctf),
and its RAM disk hides a `FLAG{…}` in bytes with **no directory entry** — `ls` won't
list it, `cat` can't name it. You make the filesystem hand it to you anyway by
crafting a disk image whose own header lies about where its data ends. (See
[`docs/planning/ctf-eng-plan.md`](docs/planning/ctf-eng-plan.md) for the full design,
and [`docs/concepts/filesystem.md`](docs/concepts/filesystem.md) for the format.)

## How to get listed

Solve it, then **[open a writeup issue](https://github.com/kenlacroix/ZiranOS/issues/new?template=ctf-writeup.md)**
describing how you did it. Once it's reviewed, your name lands here.

A writeup is what earns the spot — not the flag string. Tier 1 is a **white-box
sandbox**: the flag lives in your own browser tab, so `strings`/DevTools would find
it in seconds. That's fine and expected; the point is understanding and reproducing
the *exploit primitive* (a bounds check that trusts an attacker-controlled length),
not keeping a secret. The uncheatable, server-side version is **Tier 2** (planned).

## Roll

| # | Solver | Tier | Writeup | Date |
|---|--------|------|---------|------|
| — | *be the first* | | | |

<!-- Add rows above this line, most recent first:
| 1 | [name](https://link) | 1 | #issue | YYYY-MM-DD |
-->

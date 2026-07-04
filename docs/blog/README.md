# Ziran OS — build log

One post per milestone. The rule (from [PLAN.md §8](../../PLAN.md)): each post
says what broke, how long it took to find, and the actual fix. The debugging
story is the honest and interesting part — not "it works now."

| # | Post | Milestone | State |
|---|------|-----------|-------|
| 00 | [Setting up to build nothing](00-setting-up-to-build-nothing.md) | M0 toolchain | drafted |
| 01 | The first 512 bytes | M1 bootloader | outline |
| 02 | Handing off to Rust | M2 long mode | outline |
| 03 | Making it talk back | M3 VGA text | outline |
| 04 | [Teaching it to fail gracefully](04-teaching-it-to-fail-gracefully.md) | M4 interrupts | drafted |
| 05 | [It listens now](05-it-listens-now.md) | M5 keyboard | drafted |
| 06 | [It knows what it has now](06-it-knows-what-it-has.md) | M6 physical memory | drafted |
| 07 | [The illusion of memory](07-the-illusion-of-memory.md) | M7 paging | drafted |
| 08 | [Room to grow](08-room-to-grow.md) | M8 heap | drafted |
| 09 | [Multiple things at once, sort of](09-multiple-things-at-once.md) | M9 timer + scheduling | drafted |
| 10 | [It has a prompt now](10-it-has-a-prompt-now.md) | M10 simple shell | drafted |
| 11 | [Files are just convincing lies about disk layout](11-convincing-lies-about-disk-layout.md) | M11 filesystem (read) | drafted |
| 12 | [The whole point, arrived at](12-the-whole-point-arrived-at.md) | M12 file manager | drafted |
| 13 | [The first wall](13-the-first-wall.md) | M13 userspace / ring 3 / syscalls | drafted |
| 15 | [The confused deputy](15-the-confused-deputy.md) | M15 break the privilege boundary (security) | drafted |
| 16 | [Attacking the lies about disk layout](16-attacking-the-lies-about-disk-layout.md) | M16 break the filesystem boundary (security) | drafted |
| 14 | [Hello over a socket that isn't](14-hello-over-a-socket-that-isnt.md) | M14 networking stub (loopback) | drafted |
| ✦ | [What I actually learned building an OS with AI](a-what-i-learned-building-an-os-with-ai.md) | capstone reflection (M0–12) | drafted |

Posts are employer-agnostic and people-agnostic, matching the rest of the blog.

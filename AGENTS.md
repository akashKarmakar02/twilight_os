# AGENTS.md — Twilight OS Engineering Standard for AI Coding Agents

This document is the **binding engineering standard** for every AI coding agent
(human reviewers: it applies to you too in spirit) working in this repository.
It is long on purpose. Read all of it before writing a single line of code here,
and re-read §Golden Rules before every commit. When this file and your own
defaults disagree, **this file wins**.

Companion documents you are also expected to honor:

- [`docs/kernel-locking.md`](docs/kernel-locking.md) — the IRQ-shared lock audit.
  If you touch a lock, you touch this document.
- [`docs/timekeeping.md`](docs/timekeeping.md) — clocksource/clockevent
  architecture. If you touch time, you touch this document.
- [`docs/repository-layout.md`](docs/repository-layout.md),
  [`docs/development.md`](docs/development.md) — human-written context.

---

## 1. What this project is

Twilight OS is a **Unix-like hobby OS written in Rust** (`no_std` kernel),
currently targeting x86_64 via the Limine bootloader (UEFI + BIOS). It aims to
be genuinely usable, not just bootable: ELF dynamic linking, a self-hosting C
compiler (`tcc`), a custom filesystem (TwilightFS/TFS), a from-scratch Wayland
compositor (`twland`), a custom network stack (smoltcp-based TCP/UDP/DHCP/DNS),
full POSIX signals, runit-style init, and a Linux-style timekeeping subsystem.

The design north star: **do it the way mature Unix kernels do it — and FreeBSD
in particular is the permanent reference implementation** (see §6).

Current maturity snapshot (keep this in mind when choosing how bold to be):

| Area            | State |
|-----------------|-------|
| Paging, VFS/TFS, timekeeping, dynamic linking, networking, IPC, signals | Done |
| Preemptive userspace scheduling | Beta (kernel preemption implemented but **off by default**; SMP detected, APs parked — effectively single-CPU) |
| Wayland compositor | Experimental |
| Package mgmt (`.ipk`/opkg) | Beta |

---

## 2. Golden Rules

These are non-negotiable. Each is expanded in its own section below.

1. **Readable first.** Code is read 10× more than it is written. Optimize for
   the next reader — name things for what they mean, comment *why*, keep
   functions small. (§7.1)
2. **Clean, not clever.** No dead code, no commented-out cruft, no magic
   numbers, no surprise side effects. Explicit beats terse. (§7.2)
3. **Performance is a feature, not an afterthought.** Never allocate in
   interrupt context, never do O(n) work in a hot path when O(1)/O(log n) is
   available, never hold a lock across slow work. Measure before and after.
   (§7.3)
4. **One feature, one module.** Every kernel feature lives in its own properly
   placed submodule under the right subsystem directory. Nothing grows by
   accretion into `main.rs` or an unrelated file. New file ⇒ new `mod.rs` entry.
   (§5)
5. **FreeBSD is the reference. Forever.** Before designing any kernel
   mechanism, look up how FreeBSD solves it and follow that design unless there
   is a written reason not to. (§6)
6. **Reuse before you write.** Always grep for an existing implementation
   (kernel crates, `utils/`, `kernel_utils/`, userspace apps, docs) before
   writing anything new. Extend; do not duplicate. (§8)
7. **See a bug, own the bug.** If anything looks wrong while you scan or work —
   verify it, then fix it (in scope) or report it (out of scope). Never notice
   and walk past. Never paper over a bug you found. (§9)
8. **Docs are part of the change.** A change to a subsystem without a change to
   its `docs/` page is an incomplete change. (§11)
9. **Verify before you claim done.** Kernel builds, relevant regression
   harness passes, and you say exactly what you ran. No "should work." (§4.2)
10. **No co-author/attribution trailers** on commits or PR descriptions — the
    maintainer's standing preference. Conventional-commit format, see §10.

---

## 3. Repository Layout (where things live)

```
GNUmakefile                  Top-level build/run/test orchestration
kernel/
  twilight_kernel/           The kernel crate (no_std, x86_64)
    src/
      main.rs                Entry point only — keep it thin
      arch/x86_64/           GDT, IDT, IRQ, syscall entry/exit, port I/O, power
      compat/freebsd_kpi/    FreeBSD KPI shim: bus_space, bus_dma, callout,
                             taskqueue, mtx, intr, device/driver model
      driver/
        cpu/  disk/  keyboard/  mouse/  nic/  time/  timer/  uart/  usb/
        disk/ata/ ata_dma.rs virtioblkdev.rs     usb/uhci/ usb/xhci/ hid msc ...
      kernel_utils/          Cross-cutting kernel helpers
      sys/                   THE core kernel subsystems (see §5)
        console/ framebuffer/ fs/ memory/ net/ pci/ proc/ syscall/ time/ timer/
        acpi.rs clk.rs kmsg.rs preempt.rs rng.rs
      task/  utils/           Task/scheduler context, sync primitives (Mutex,
                             WaitQueue), shared bitmaps
  twilight_common/           Shared kernel⇄userspace types (syscall ABI lives here)
  twilight_proc/             Proc macros used by the workspace
userspace/
  apps/<name>/               One directory per app — auto-built by build.rs
                             (detects Cargo.toml / build.zig / Makefile / C)
  libs/                      Cross-built third-party libs (libffi, wayland, twigl)
  protocols/                 Wayland protocol definitions
  build.rs                   The userspace builder; stages binaries into rootfs/bin
rootfs/                      Base root filesystem content (packed into system.tfs);
                             rootfs/bin & sbin are GENERATED — never hand-edit
tools/
  mktfs/                     Builds the TwilightFS system image (system.tfs)
  time-regression/           Host-side headless QEMU timing regression harness
docs/                        Subsystem documentation — read before touching a
                             subsystem, update after changing it
limine.conf                  Bootloader config
limine/ ovmf/                Downloaded boot assets (gitignored)
initramfs.cpio system.tfs twilight-os.iso hdd.img   Build outputs (gitignored)
qemu.log                     QEMU trace log — CAN BE 100 MB+. grep it, never
                             read it whole.
```

Git submodules (upstream/ported projects — special handling, §10.2):

```
userspace/apps/tcc    (fork: akashKarmakar02/tinycc, musl port)
userspace/vi          (fork: akashKarmakar02/viless)
userspace/oksh        (upstream: ibara/oksh)
```

---

## 4. Building, Running, Verifying

### 4.1 Requirements

Nightly Rust (pinned ~`nightly-2025-06-01`; the Makefile force-picks the glibc
host toolchain on musl hosts), `llvm-tools-preview`, `nasm`, binutils `ld`,
`xorriso`, `qemu-system-x86_64`, `musl-gcc`/musl-tools, `cpio`, `sgdisk`,
`mformat`/`mcopy` (mtools). First `make` auto-downloads Limine and OVMF.

### 4.2 Command cheat sheet

| Command | What it does |
|---|---|
| `make` / `make all` | Full ISO build: kernel + userspace + initramfs + system.tfs |
| `make kernel` | Kernel only (`cargo build --target x86_64-unknown-none --release`) |
| `make userspace` | All userspace apps → `rootfs/bin/` (stable toolchain) |
| `make run` | Default QEMU run (IDE hdd + ISO cdrom, `-serial stdio`) |
| `make run-x86_64-uefi` | UEFI/OVMF boot |
| `make run-bios` | q35 BIOS boot |
| `make run-blk-bios` | virtio-blk + qemu-xhci (USB storage testing) |
| `make run-hdd` / `make all-hdd` | GPT HDD image instead of ISO |
| `make test-time` | **Headless non-destructive timing regression matrix** — no disk writes |
| `make clean` / `make distclean` | Remove outputs (+ downloaded boot assets) |

Guest exit from `-serial stdio` QEMU: `Ctrl-A x`.

### 4.3 Verification loop — what "done" means

Before saying a task is finished you must have actually run, at minimum:

1. `make kernel` — the kernel compiles clean (it is `no_std`; treat every new
   warning as a bug in your change, not noise).
2. `make userspace` — if you touched anything under `userspace/`.
3. The regression harness matching your change's area:
   - Timing/scheduler/timer/poll/anything with deadlines → **`make test-time`**
     must pass.
   - Disk/FS → `iotest`, `diskbench` inside the guest.
   - IPC → the `rootfs/bin/*_test` guest binaries (`pipe_ipc_test`,
     `unix_bind_test`, `fdpass_*_test`, `memfd_*_test`, `poll_test`).
   - Wayland → `twland_test_client`, `wayland_shm_foundation_test`.
4. **Pure logic gets host-side unit tests.** Established pattern: extract pure
   helpers into a file the host can test (see `pacing.h` in smolnes,
   `tools/time-regression`). Prefer this over only-in-guest testing when logic
   is separable from hardware.
5. Report exactly what you ran and what passed. If you could not run something,
   say so — a skipped verification reported honestly beats a fake green tick.

First-boot note: a fresh `hdd.img` has no filesystem; run `install` in the guest
shell, then reboot, to get persistent TwilightFS.

---

## 5. Module Organization — "one feature, one submodule"

The kernel's layout is the contract. Every new feature lands as its own
properly-placed module; nothing gets appended to an unrelated module because it
was convenient.

**Where new code goes:**

| You're adding... | It goes in... |
|---|---|
| A new syscall | `sys/syscall/<domain>.rs` (e.g. `time.rs`, `poll.rs`, `fs_attr.rs`); ABI numbers/types in `kernel/twilight_common/src/syscall/`; register/unregister in `sys/syscall/mod.rs` |
| A new character/block device driver | `driver/<device-class>/<driver_name>/` or `<driver_name>.rs` under the class dir |
| A new filesystem backend | `sys/fs/<fs_name>/` implementing the VFS interface in `sys/fs/vfs.rs` |
| A new socket family/option | `sys/net/socket/` (+ `bind_map.rs` for bind state) |
| A new scheduler behavior | `sys/proc/` (+ `sys/preempt.rs`, `task/`) |
| New memory management behavior | `sys/memory/` (bitmap/phys/slab/heap/vmalloc already exist — reuse, don't wrap) |
| Timer/deadline machinery | `sys/timer/` (queue) and `driver/time/` (clocksource/clockevent split — §7.4) |
| Console/tty behavior | `sys/console/` |
| Shared kernel⇄userspace ABI types | `kernel/twilight_common` — **never** duplicated into the kernel crate |
| Reusable generic helpers | `utils/` (bitmaps, sync) or `kernel_utils/` |
| A userspace app | `userspace/apps/<app>/` with whatever build system; `build.rs` auto-detects it |
| FreeBSD-KPI surface area | `compat/freebsd_kpi/` — mirroring the named FreeBSD primitive |

Rules for module hygiene:

- Every module gets a `//!` header comment: what it owns, what it deliberately
  does **not** own, and its key invariants. The existing docs (timekeeping,
  locking) show the house style: ownership boundaries called out explicitly.
- Add the module to its parent `mod.rs`. If a subsystem dir has no `mod.rs`
  entries listing purpose, add one-line comments.
- Public items get `///` doc comments. Non-obvious unsafe gets a `// SAFETY:`
  comment stating the invariant relied on.
- A module file that grows past several hundred lines of *mixed* concerns is a
  smell: split by feature exactly like the tree above splits.
- The `compat/freebsd_kpi/` layer is how FreeBSD-shaped drivers compile here;
  when a driver's natural expression is "like FreeBSD", extend the shim rather
  than hand-rolling parallel infrastructure.

---

## 6. FreeBSD Is The Reference — Forever

This is a standing, permanent policy of the project, not a suggestion.

**When implementing or changing any kernel mechanism — timers, polling, sockets,
signals, driver models, locking, VM, VFS behavior, IPC, device probing — first
consult FreeBSD's implementation of that mechanism and follow its structure,
semantics, and naming unless the repo documents a deliberate divergence.**

Why: FreeBSD is a coherent, single-tree, BSD-licensed Unix with a famously
clean implementation of exactly the primitives a hobby Unix needs, and this
repo already encodes that decision twice — the timekeeping docs explicitly say
"as Linux and FreeBSD do," and the kernel ships a `compat/freebsd_kpi/` shim so
FreeBSD-style drivers can compile here. Stay on that course.

### 6.1 How to consult it

Keep a local copy — it's the fastest way to answer "how does FreeBSD handle X":

```bash
git clone --depth 1 https://git.freebsd.org/src.git ~/refs/freebsd-src
```

(Or browse `https://cgit.freebsd.org/src/tree/`.) Paths below are FreeBSD-head
layout.

### 6.2 Mapping table — Twilight feature ⇄ FreeBSD reference

| Twilight code | FreeBSD reference |
|---|---|
| `compat/freebsd_kpi/callout.rs` | `sys/kern/kern_timeout.c`, `sys/sys/_callout.h` |
| `compat/freebsd_kpi/taskqueue.rs` | `sys/kern/kern_taskqueue.c` |
| `compat/freebsd_kpi/bus_space.rs`, `dma.rs` | `sys/sys/bus.h`, `sys/sys/bus_dma.h`, `sys/kern/subr_bus.c` |
| `compat/freebsd_kpi/mutex.rs` | `sys/sys/mutex.h`, `sys/kern/kern_mutex.c` |
| `driver/time/` clocksource/clockevent split | `sys/kern/kern_clock.c`, `sys/sys/timecounter.h`, `sys/x86/x86/tsc.c`, `sys/dev/acpica/acpi_hpet.c` |
| `sys/timer/` absolute-deadline queue | `sys/kern/kern_timeout.c` (callout wheel semantics) |
| `sys/syscall/poll.rs` | `sys/kern/sys_generic.c` (`poll`, `select`) |
| `sys/syscall/time.rs` nanosleep/clock_nanosleep | `sys/kern/kern_time.c` |
| `sys/net/socket/` | `sys/kern/uipc_sock.c`, `sys/kern/uipc_socket.c`, `sys/kern/uipc_usrreq.c` (unp) |
| Signal delivery (`rt_sigaction` etc.) | `sys/kern/kern_sig.c` |
| `sys/proc/` fork/exec/exit semantics | `sys/kern/kern_fork.c`, `sys/kern/kern_exec.c`, `sys/kern/kern_exit.c` |
| `sys/fs/vfs.rs` VFS interface | `sys/sys/vnode.h`, `sys/kern/vfs_vnops.c` |
| TwilightFS internals | `sys/ufs/ufs/` (design reference for inode/extents decisions) |
| `driver/disk/ata/`, `ata_dma.rs` | `sys/dev/ata/` |
| `driver/nic/` RTL8139 / PCNET | `sys/dev/re/if_re.c`, `sys/dev/pcn/if_pcn.c` |
| `driver/usb/` (uhci/xhci/hid/msc) | `sys/dev/usb/` (FreeBSD's controller-stack separation maps closely) |
| PS/2 input | `sys/dev/atkbdc/` |

When you add a new mapping row in spirit — i.e., you designed something after a
FreeBSD file — say so in the PR description (and, where useful, a comment):
"modeled on FreeBSD `kern_timeout.c`". Future readers get the trail.

### 6.3 Licensing & copying rules

Twilight OS is BSD-3-Clause; FreeBSD is BSD-2-Clause. Consulting designs and
algorithms is unambiguously fine and encouraged. If you copy actual code lines
(rare; usually the Rust↔C rewrite makes this moot): preserve the original
copyright notice and attribution in a header comment, and say so in the PR.
Never copy code from GPL sources into this kernel. If a design decision is
forced by *our* constraints (no_std, single-CPU, Rust) rather than FreeBSD's,
write the divergence down in the relevant `docs/` page.

---

## 7. Code Standards

### 7.1 Human-readable code

- **Names carry the design.** `block_current_until_deadline`, not `wait2`.
  Names say what a thing *is* in domain language, using the terms the docs use.
- **Comment *why*, not what.** The code already says what it does; the comment
  explains the invariant, the hardware quirk, the ordering constraint, or the
  historical bug ("this ordering prevents the lost-wakeup from #66").
- Invariant comments are mandatory at: lock-ordering sites, memory-ordering /
  `unsafe` blocks, IRQ-state transitions, and any code whose correctness depends
  on another module's behavior. Cite the issue number when a comment explains a
  past-bug guard — the repo already does this and it's valuable archaeology.
- Functions stay small enough to state their contract in the `///` line. Deep
  nesting ⇒ early returns. Boolean params at call sites are unreadable — if you
  can't avoid one, name it (`/* dry_run */ true`) or take an enum.
- Prefer boring, flat control flow over macros/generics gymnastics. The reader
  is a new contributor at 1 AM.
- Match the surrounding code's idiom — this codebase favors plain explicit
  Rust, table-driven registration, and prose docs. Do not import a shiny
  dependency to avoid writing 30 clear lines.

### 7.2 Clean code

- Dead code, commented-out code, `dbg!`/`println!` leftovers, and stale TODOs
  do not get committed. Convert genuine TODOs into issues instead.
- Magic numbers are named constants, especially timeouts, sizes, and IRQ
  numbers, with a comment citing the spec/source of the value.
- Errors are handled explicitly and propagated with context; no silent
  fallthrough. Panic only on unrecoverable invariant violations — with a
  message naming the violated invariant.
- One responsibility per function. If your doc comment needs "and" three times,
  split it.
- Formatting must be rustfmt-clean for touched files. Keep diffs minimal:
  reformat a file only as its own dedicated commit, never mixed into a feature
  diff.
- Tests are part of clean code: pure logic extracted testable on the host
  (§4.3), guest-visible behavior covered by a harness or a guest test binary.

### 7.3 Performance

Performance is a first-class goal in this repo — its history includes ATA DMA,
TFS hot-file caching, extent-based block mapping, allocation-free wait queues,
and the one-shot LAPIC clockevent conversion specifically for performance and
latency. Treat it accordingly:

**Never (hard rules):**

- **Never allocate in interrupt context.** Interrupt handlers and anything they
  call run with the allocator off-limits. Use pre-allocated slabs/pools (see
  `WaitQueue`'s allocation-free design, `sys/memory/slab.rs`). This is enforced
  socially here — an allocation in an ISR is a bug.
- **Never hold a spinlock across I/O, memory allocation-heavy work, or long
  loops.** Critical sections are as short as the invariant allows.
- **Never add polling loops, per-tick wake-all storms, or O(n) reschedules in
  hot paths** when an ordered/heap/indexed structure gives O(log n) or O(1) —
  the deadline queue (#66), per-fd wakeup (#69), and clockevent arming (#68)
  replaced exactly these patterns.
- **Never block the single CPU gratuitously** — busy-wait polling is reserved
  for documented device-controller handshakes with timeout bounds.

**Always (default disciplines):**

- Know which paths are hot: interrupt handlers, `dispatch_irq`, timer expiry,
  scheduler switch, fast-path syscalls (`read`/`write` on pipes/tty), VFS
  lookup, packet RX. Design those first for constant factors.
- Batch I/O where the hardware allows it (DMA over PIO, multi-sector transfers).
- Cache deliberately — follow TFS's existing hot-file cache pattern; extend
  caches rather than stacking new ad-hoc ones.
- Prefer zero-copy / borrow over clone when lifetimes allow.
- Measure before and after: `make test-time` (wake latency, clock drift),
  `iotest`/`diskbench`, `clockcheck`. A performance claim in a PR without a
  number is an opinion.
- Prefer existing allocators (slab, bitmap, heap) sized right over embedding
  new ad-hoc structures.

### 7.4 Subsystem-specific correctness rules (learn these before touching)

- **Locking** ([`docs/kernel-locking.md`](docs/kernel-locking.md)): a lock ever
  taken from an IRQ handler must be taken with `lock_irq()` at **every** site —
  one `lock()` site is a deadlock. Syscalls currently run with IF cleared
  (`IA32_FMASK = 0x300`); `PROCESS_TABLE` safety analysis depends on this. If
  you enable interrupts anywhere in syscall bodies you **must re-audit
  PROCESS_TABLE** and update the lock inventory doc.
- **Timekeeping**: `CLOCK_MONOTONIC` is *never* derived from delivered timer
  interrupts — only from the validated free-running clocksource (TSC/HPET).
  Clock events drive wakeups, not the timeline. Don't break the
  clocksource/clockevent split.
- **Scheduler**: deferred-reschedule kernel — explicit `cond_resched()` safe
  points; `enable_preemption()`/`disable_preemption()` nest. Wakeups go through
  the token-safe deadline queue; absolute deadlines only.
- **Userspace apps** are freestanding by default; check whether an app builds
  against `twigl`/wayland libs before inventing new platform shims.

---

## 8. Reuse-First — check for existing implementations before writing anything

Before writing any new function, type, table, or app, **search for it first**.
This repo is deep enough that it often already has 80% of what you need.

Search order:

1. `kernel/twilight_kernel/src/utils/` and `kernel_utils/` — generic helpers.
2. `kernel/twilight_common/` — shared ABI types.
3. The owning subsystem dir for the feature (§5 table).
4. `userspace/apps/` and `userspace/libs/` — does an app/lib already do this?
5. `grep` the whole tree for the concept's likely names (plural/singular, `_`
   and CamelCase variants, FreeBSD names — the codebase borrows FreeBSD's
   vocabulary deliberately, e.g. `callout`, `taskqueue`, `bus_space`).
6. `git branch -a` and closed PR history — someone may have built it already on
   a branch (`feat/*`, `fix/*`, `port/*`).

If it exists: extend it. If it exists but doesn't fit, first consider whether
generalizing it is cleaner than duplicating it — and if you truly must fork it,
say why in a comment. Duplicating an existing capability silently is treated as
a review defect.

The flip side: don't delete or rewrite a working implementation to be "nicer"
without a functional reason — churn costs review time and risks regressions.

---

## 9. Bug Protocol — see something, say something, fix something

While scanning, reading, or working anywhere in this repo, **if something looks
like a bug, you are required to act**. Ignoring a spotted bug is the one way to
fail this project's review culture outright.

Procedure:

1. **Verify before claiming.** Re-read the code path; check the docs'
   stated invariants; find the exact conditions that trigger it. Most false
   alarms come from missing a locking or IRQ-context subtlety — the docs exist
   precisely so you don't have to guess.
2. **Fix it if it's in scope** for your current work, small, and clear: write
   the fix, add a regression test where feasible, and report it in the PR
   description as a distinct bullet ("also fixes: …").
3. **Report it if it's out of scope, large, or risky:** open a GitHub issue
   (use `gh`) with: repro steps or triggering conditions, expected vs actual,
   affected file/function, suspected root cause, and severity. Link the issue
   in your PR if related.
4. **Never paper over it.** Swallowing an error, widening a timeout, or
   loosening an assertion to make a symptom disappear is forbidden. If you must
   temporarily work around a bug to proceed, leave a `// FIXME(#NNN):` comment
   referencing the filed issue.
5. Update the relevant `docs/` invariant tables when a bug changes them (a lock
   gains/loses an IRQ-site, a subsystem changes an ownership boundary).

Suspect-list to keep in the back of your mind while scanning (this repo's
historical bug classes): lost wakeups around sleep/wait races, IRQ-shared locks
acquired with plain `lock()`, timeline math deriving time from interrupt
counts, integer truncation on byte-offset register addresses, EINTR/remainder
semantics, fd lifetime across SCM_RIGHTS passes, and lock-ordering cycles.

---

## 10. Git & PR Workflow

### 10.1 Branches, commits, PRs

- Branch per issue: `feat/<slug>-<issue#>` or `fix/<slug>-<issue#>` from
  `master`.
- For substantial issue-scoped work, write the plan to
  `.claude/plans/<slug>-<issue#>.md` first (this is established practice —
  issues #62–#72 all have plan docs). Plans are gitignored local scaffolding.
- Conventional commits, scope in parens, present tense, issue ref in the body
  or title per repo habit:
  `feat(kernel): ...`, `fix(time): ...`, `test(sched): ...`, `docs(fs): ...`
- One logical change per commit; `fixup` commits squashed before merge.
- PRs target `master`; CodeRabbit reviews them — address its findings or argue
  them in-thread; "address review findings" commits then a review-fix commit
  is the house pattern.
- **No `Co-Authored-By`/attribution trailers on commits, no "generated with"
  footers on PR bodies.** Standing maintainer preference; do not add them.
- Don't commit build artifacts (`iso`, `hdd.img`, `system.tfs`,
  `initramfs.cpio`, `qemu.log`, `rootfs/bin|sbin`) — they're gitignored.
- **Never commit third-party copyrighted assets** (ROMs like `.nes`, `.wad`,
  firmware blobs, game data) even if lying around in `rootfs/` untracked.

### 10.2 Submodule conduct

`userspace/apps/tcc`, `userspace/vi`, `userspace/oksh` are git submodules. Never
edit inside them casually: commit submodule work in the submodule's repo first
(tcc and vi are the project's own forks), then bump the superproject's pointer
in a separate commit. Note `.gitmodules` uses the upstream URL for oksh — don't
push changes meant for the fork there. When in doubt, treat submodule contents
as read-only dependencies and surface the need as an issue.

---

## 11. Documentation Duty

Every subsystem has a `docs/` page; the docs are how invariants survive. Rules:

- Touch a subsystem ⇒ read its doc first ⇒ update it in the same PR if
  behavior, interfaces, or invariants changed. Lock inventory table, timekeeping
  diagram, syscall lists — these are living documents.
- New subsystem ⇒ new doc page with an architecture overview, ASCII diagram
  where it clarifies structure (house style), ownership boundaries, and known
  limitations.
- Prose is precise and dense: exact rule, reason, exception. Match the tone of
  `docs/kernel-locking.md` / `docs/timekeeping.md`.
- Doc-only changes are commit type `docs(scope): ...`.

---

## 12. Quick self-check before you push

- [ ] Read the relevant `docs/` pages and honored their invariants?
- [ ] Checked for an existing implementation before writing (§8)?
- [ ] Followed FreeBSD reference for design (§6)?
- [ ] New code in its own module, registered in `mod.rs`, doc-commented (§5, §7.1)?
- [ ] No allocation added in interrupt context, no new hot-path O(n) (§7.3)?
- [ ] IRQ-shared locking rules respected, lock inventory updated if needed (§7.4)?
- [ ] Spotted bugs fixed or filed, none papered over (§9)?
- [ ] Built (`make kernel`, `make userspace`) and ran the matching harness (§4.3)?
- [ ] Docs updated (§11)?
- [ ] Conventional commit, right branch, no attribution trailers, no artifacts
      staged, no copyrighted assets (§10)?

If any box is unchecked, you're not done — either finish it or say plainly which
one and why not.

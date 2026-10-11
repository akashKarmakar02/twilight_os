# CLAUDE.md — Claude Code operating notes for Twilight OS

**The full engineering standard lives in [@AGENTS.md](AGENTS.md) — read it and
follow it.** It is binding for all work in this repo: readable/clean code,
performance discipline, one-feature-one-module placement, FreeBSD as the
permanent design reference, reuse-first search before writing anything, the
see-a-bug-fix-or-file-it protocol, build/verify loop, git/PR conventions, and
doc-update duty. Highlights you will be held to (details in AGENTS.md):

- **FreeBSD is the reference forever** (§6): consult FreeBSD source before
  designing any kernel mechanism; follow its structure/naming unless divergence
  is documented. `compat/freebsd_kpi/` is the in-repo expression of this.
- **Reuse first** (§8): grep `utils/`, `kernel_utils/`, `twilight_common/`,
  subsystem dirs, userspace apps, and branches for an existing implementation
  before writing anything new.
- **One feature, one submodule** (§5): new code lands in its own module in the
  right subsystem dir, registered in `mod.rs`, doc-commented.
- **Bug protocol** (§9): if anything looks buggy while scanning or working,
  verify it, then fix it in-scope or file an issue out-of-scope. Never walk
  past one, never paper over one.
- **Verification before "done"** (§4.3): `make kernel` (+ `make userspace` when
  relevant) + the matching harness (`make test-time`, guest `iotest`/
  `diskbench`, the `rootfs/bin/*_test` binaries). Report exactly what ran.
- **No `Co-Authored-By` trailers on commits and no attribution footers on PR
  bodies** — standing maintainer preference (§10.1).
- Touch a subsystem ⇒ update its `docs/` page in the same change (§11).

## Claude-specific operating notes

### Planning
For substantial issue-scoped work, write a plan to
`.claude/plans/<slug>-<issue#>.md` first (established practice — issues #62–#72
each have one), get it approved, then implement. Plans are gitignored local
scaffolding, not deliverables.

### Repo-specific gotchas
- The project directory is `twilight_os` (underscore), not `twilight-os`.
- `qemu.log` regularly exceeds 100 MB — grep it, never read it whole.
- `rootfs/bin/` and `rootfs/sbin/` are **generated** by `make userspace`; never
  hand-edit staged binaries there. Sources live under `userspace/apps/`.
- Git submodules: `userspace/apps/tcc`, `userspace/vi`, `userspace/oksh` —
  treat as read-only dependencies; submodule changes go through their own repos
  first (AGENTS.md §10.2).
- Guest serial console exit is `Ctrl-A x` when running QEMU with `-serial stdio`.
- Build outputs (`twilight-os.iso`, `system.tfs`, `initramfs.cpio`, `hdd.img`,
  `qemu.log`) are gitignored artifacts — don't stage them; also never stage
  copyrighted ROM/game assets that may be lying around `rootfs/`.

### Build & test quick reference
| Command | Use |
|---|---|
| `make kernel` | Compile kernel only (treat every new warning as a bug) |
| `make userspace` | Build all apps → `rootfs/bin/` |
| `make run` / `run-x86_64-uefi` / `run-bios` / `run-blk-bios` | QEMU runs |
| `make test-time` | Headless timing regression matrix (no disk writes) |

First boot on a fresh `hdd.img`: run `install` in the guest shell, then reboot,
for persistent TwilightFS.

### Key subsystem docs (read before touching)
- [`docs/kernel-locking.md`](docs/kernel-locking.md) — IRQ-shared lock audit;
  any lock touched ⇒ this doc touched.
- [`docs/timekeeping.md`](docs/timekeeping.md) — clocksource/clockevent split;
  never derive monotonic time from interrupt counts.
- `docs/filesystem/`, `docs/usb/`, `docs/system/`, `docs/disk/` — per-subsystem
  internals.

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

### Creating Rust userspace apps (including twland-style services)

Follow [AGENTS.md §7.5](AGENTS.md#75-rust-userspace-applications--creation-and-coding-rules)
and the [tested app starter](docs/userspace.md#creating-a-rust-application).
These are requirements for new Rust apps, not permission to rewrite existing
applications without a functional reason.

- Create `userspace/apps/<app>/` with stable Rust/edition 2024, a declared
  minimum Rust version, `Cargo.toml`, committed `Cargo.lock`, `src/main.rs`,
  and the existing Makefile integration pattern. Build the musl guest target
  explicitly, keep static/non-PIE flags, propagate errors, and expose the
  binary at the app root for staging. Ignore generated output. Do not rely
  on the incomplete Cargo-only builder path ([#94](https://github.com/akashKarmakar02/twilight_os/issues/94)).
- Rust userspace normally uses `std`; kernel `no_std`, panic handlers,
  toolchain settings, and linker scripts do not belong in these apps. Verify
  required syscalls/devices and dependency compatibility with Twilight.
- Keep `main.rs` for startup/run/error reporting. Place transport, protocol
  state, platform I/O, input, rendering, and feature policy in cohesive modules.
  Reuse `twland`'s separated modules and existing shared interfaces; do not
  copy its large `main.rs` as an architectural template. Document module
  invariants and expose the smallest necessary API.
- Use descriptive Rust naming, meaningful types/enums, references/slices,
  explicit resource ownership, and contextual `Result` errors. Malformed
  input must not panic or crash an entire service. Avoid hidden fallbacks,
  unnecessary dependencies, blanket lint allowances, and hot-loop logging.
- Own fds/mappings with RAII. Isolate FFI/unsafe, verify guest ABI layouts and
  bounds, and write safety justifications. Validate all protocol lengths,
  conversions, object lifetimes, and received descriptors; close resources
  on failure/disconnect. Wayland fds travel through SCM_RIGHTS, and registry
  discovery must not assume a fixed number of globals.
- Use nonblocking readiness and bounded event-loop work, backpressure,
  monotonic deadlines, and reusable buffers. Do not busy-spin or block all
  clients on one operation. Make shutdown and buffer presentation explicit.
- Run rustfmt, Clippy, and host tests with guest-only flags cleared and an
  explicit host target, then verify the staged ELF and behavior in the guest
  after the required builds. Host tests cover pure logic; guest checks cover
  ABI/devices/IPC/rendering. Update the app/subsystem docs and deliberately
  configure twinit only if the app is meant to run as a service.

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
- Git submodules are inventoried in `.gitmodules` and [docs/ports.md](docs/ports.md).
  Commit and push source changes in their own repositories before updating
  OS gitlinks; upstream-only remotes need a project fork for local changes
  (AGENTS.md §10.2).
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
- [`docs/userspace.md`](docs/userspace.md) — app creation, build/staging rules,
  and host/guest verification; Rust apps must also follow AGENTS.md §7.5.
- [`docs/kernel-locking.md`](docs/kernel-locking.md) — IRQ-shared lock audit;
  any lock touched ⇒ this doc touched.
- [`docs/timekeeping.md`](docs/timekeeping.md) — clocksource/clockevent split;
  never derive monotonic time from interrupt counts.
- `docs/filesystem/`, `docs/usb/`, `docs/system/`, `docs/disk/` — per-subsystem
  internals.

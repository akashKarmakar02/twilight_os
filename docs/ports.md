# Port sources and Git submodules

Third-party application ports and library sources are pinned by gitlinks in
the OS repository. Native Twilight applications and twigl remain ordinary
source directories. Build wrappers and sysroot integration belong to the OS;
upstream code and platform frontends belong to the source repositories.

| Source path | Repository | Integration |
|---|---|---|
| `userspace/apps/tcc` | [akashKarmakar02/tinycc](https://github.com/akashKarmakar02/tinycc) | Existing musl compiler port |
| `userspace/vi` | [akashKarmakar02/viless](https://github.com/akashKarmakar02/viless) | Standalone vi builder |
| `userspace/oksh` | [ibara/oksh](https://github.com/ibara/oksh) | Standalone shell builder |
| `userspace/doomgeneric` | [akashKarmakar02/doomgeneric](https://github.com/akashKarmakar02/doomgeneric) | `apps/fbdoom/Makefile` builds the framebuffer port |
| `userspace/apps/smolnes` | [akashKarmakar02/smolnes](https://github.com/akashKarmakar02/smolnes) | Normal app discovery builds the framebuffer port |
| `userspace/snes9x-fb` | [akashKarmakar02/snes9x-fb](https://github.com/akashKarmakar02/snes9x-fb) | `apps/snes9x/Makefile` builds `twilight/` in Alpine |
| `userspace/libs/libffi/upstream` | [libffi/libffi](https://github.com/libffi/libffi) | Wrapper bootstraps Autotools and cross-builds for musl |
| `userspace/libs/wayland/upstream` | [wayland/wayland](https://gitlab.freedesktop.org/wayland/wayland) | Wrapper cross-builds libraries with Meson and a native scanner |

Doom, Smolnes, and Snes9x use the forks' `twilight` branches. The OS checks out
the exact committed gitlink, regardless of a remote's default branch. Library
pins initially match libffi **v3.5.2** and Wayland **1.25.0**, the releases used
before migration. Extraction preserves emulator, input, and pacing code and
retains upstream licenses and history. Smolnes's unused SDL submodule is
removed on the Twilight branch; the framebuffer port does not need it.
Snes9x likewise omits nested shader, cubeb, and Windows-library submodules
unused by its standalone Twilight frontend.

## Clone and initialize

```sh
git clone --recurse-submodules https://github.com/akashKarmakar02/twilight_os.git
cd twilight_os
```

For an existing checkout, or after switching to a commit with different pins:

```sh
git submodule sync --recursive
git submodule update --init --recursive
git submodule status --recursive
```

Each status line starts with a space when the checkout matches its pin, `-`
when uninitialized, or `+` when checked out elsewhere. Preserve local work
before updating. Do not use `--force`, or `update --remote` across all
dependencies, as initialization steps. The latter selects moving branch
heads rather than the recorded pins.

## Build ownership

`make userspace` keeps its existing app entry points. Initialize submodules
first; builds do not fetch or update source revisions. Snes9x no longer needs
a sibling checkout but still requires its [Alpine distrobox](../userspace/apps/snes9x/README.md).
`SNES_DIR` selects another development checkout; `SNES_BOX` selects the container.

Library wrappers use `upstream/` as source and ignored `build/<commit>/`
directories for objects and staging. A changed pin selects a fresh build
configuration. Incremental builds still invoke Make or Ninja to detect source
edits. Libffi's `autogen.sh` generates ignored files inside its source checkout
and needs Autoconf, Automake, and Libtool; guest documentation is disabled.
Wayland needs Meson, Ninja, pkg-config, a host C compiler, and Expat development
headers (`libexpat1-dev` on Debian/Ubuntu, `expat-devel` on Fedora). The wrapper
builds and stages a native scanner from the same source pin before cross-building
the libraries. A Meson native file selects that scanner without adding native
libraries to the musl dependency search path. `HOST_CC` selects the native C
compiler (default `cc`); it is independent of the guest compiler. The native
clients share `userspace/libs/wayland/scanner.mk` to select this same scanner
for protocol bindings, with an optional `WAYLAND_SCANNER` override. `make clean` in a library wrapper removes
build outputs while retaining its source submodule.

## Change a port or update a pin

1. Create a branch in the owning source repository before editing a detached
   checkout. Preserve upstream notices and exclude outputs and game data.
   For upstream remotes (oksh, libffi, Wayland), prepare a project fork if
   Twilight-specific changes are needed.
2. Commit and push source changes there first. For upstream updates, fetch and
   explicitly choose a release or commit, reconciling port changes in the fork.
   The OS must reference a commit available remotely.
3. Stage the source path in the OS with `git add <submodule-path>`. Review
   `git diff --cached --submodule=log` and update relevant docs and toolchain
   requirements. Do not hand-edit the gitlink hash.
4. Build with `make kernel` and `make userspace`, run affected tests, and verify
   a fresh recursive checkout can fetch the new pin before publishing the OS
   commit. Smolnes host tests use `make -C userspace/apps/smolnes test`;
   library updates also need guest Wayland validation.

The existing Rust `twland_test_client` currently stops after five registry
globals and rejects the compositor's sixth advertisement; see
[#91](https://github.com/akashKarmakar02/twilight_os/issues/91). This predates
the migration. `wayland_shm_foundation_test` and the dynamically linked
`twland_libwayland_client` provide additional checks; report the probe failure
explicitly rather than treating it as a passing regression run.

Existing OS history still contains the former vendored sources; migration
does not rewrite it. ROMs, WADs, firmware, generated executables, and build
caches must stay out of both the OS and port repositories.

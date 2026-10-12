# Userspace

Userspace apps live in `userspace/apps/`. The `userspace` crate contains a build script that discovers each app directory, builds it, and copies the resulting executables into `rootfs/bin`.

## Build behavior
The build script first builds libraries in `userspace/libs/`, then detects apps
in this order:

1. Makefile (`Makefile` or `makefile`)
2. Rust (`Cargo.toml`)
3. Zig (`build.zig` or `zig.toml`)
4. Plain C/C++ files in the directory

Executables are copied into `rootfs/bin` and marked executable.
Standalone oksh and vi submodules are built afterward. Service-manager and
logging binaries also receive generated `/sbin` copies. Most programs link
statically; Wayland clients and Snes9x can use shared sysroot libraries.

## Environment variables
- `CC`: overrides the C compiler used for Makefile and C builds (defaults to `musl-gcc`).
- `TWILIGHT_NO_PIE`: set to `0` to allow PIE; by default, PIE is disabled for static userland binaries.
- `SNES_DIR`, `SNES_BOX`: Snes9x source directory override and Alpine distrobox name.

## Bundled apps
Highlights include `tcc`, `oksh`, `vi`, core command-line utilities, `curl`,
`httpd`, `twinit`, `twinitctl`, `twilogd`, `twilogctl`, `twland`, `twclock`,
`smolnes`, `chip8`, and `fbdoom`. Guest probes cover timing, storage, IPC,
shared memory, polling, networking, and Wayland. Browse `userspace/apps/` for
the full list; oksh and vi also have standalone submodule build paths.

## Snes9x port

`userspace/apps/snes9x/Makefile` delegates to the pinned
`userspace/snes9x-fb/twilight` source submodule. It requires an Alpine distrobox
with musl-native C++ tools; the wrapper is discovered by the ordinary userspace
build. Initialize submodules and follow the
[wrapper README](../userspace/apps/snes9x/README.md) before building.

## Smolnes

The `userspace/apps/smolnes` source submodule uses accumulated absolute monotonic deadlines
for frame pacing. Run `make -C userspace/apps/smolnes test` for its host pacing
tests. Current `master` uses keyboard input; the generic gamepad work is
described separately in [usb/gamepads.md](usb/gamepads.md).

ROMs and Doom WADs are user-supplied local assets and must not be committed.

## Known build limitations

The plain-C path currently stages only executable paths absent before the
build. Rebuilding a binary already present in its app directory can therefore
leave `rootfs/bin` stale; this is tracked in
[#89](https://github.com/akashKarmakar02/twilight_os/issues/89). A successful
compiler invocation alone does not prove the staged copy was refreshed.

The Cargo-only fallback ignores Cargo's failing exit status and searches the
app root rather than Cargo's target directory for executables
([#94](https://github.com/akashKarmakar02/twilight_os/issues/94)). Until that
path is repaired, Rust apps must use the Makefile integration below, as
`twland` and `twinit` already do.

## Doom

The Doom port source is pinned as the `userspace/doomgeneric` submodule.
The Twilight framebuffer and input port is built through
`userspace/apps/fbdoom/Makefile`, so the normal userspace build installs
`fbdoom` into `rootfs/bin`:

```bash
make userspace
```

After booting Twilight OS, run:

```sh
fbdoom -iwad /doom.wad
```

See `userspace/doomgeneric/TWILIGHT.md` for upstream provenance, port details,
and standalone build instructions.

## Third-party sources

All application ports and downloaded library sources are pinned submodules;
see [ports.md](ports.md) for the inventory and update workflow. The libffi and
Wayland wrappers build their `upstream/` submodules rather than downloading
tarballs. Their objects and staging trees use `build/<commit>/`, so a changed
pin selects a fresh configuration. Autoconf, Automake, and Libtool bootstrap
libffi; the cross-build disables guest documentation. Twilight's own apps and
twigl remain ordinary source directories.

## Adding a new app
1. Create a new directory under `userspace/apps/<name>`.
2. Add a `Makefile` (required for Rust apps until #94 is fixed), or a supported
   build description for another language.
3. Build with `make userspace` or `make` at the repo root.
4. Verify the resulting binary was refreshed in `rootfs/bin`.

## Creating a Rust application

Follow [AGENTS.md §7.5](../AGENTS.md#75-rust-userspace-applications--creation-and-coding-rules)
for coding, resource safety, protocol, and event-loop requirements. Native
Rust apps use `std` on the guest musl target; they are independent packages,
not members of the bare-metal kernel workspace. Start with this layout:

```text
userspace/apps/my_app/
  Cargo.toml
  Cargo.lock
  Makefile
  .gitignore
  src/main.rs
```

`Cargo.toml` (the minimum Rust version here covers this starter; raise it when
the app uses newer features):

```toml
[package]
name = "my_app"
version = "0.1.0"
edition = "2024"
rust-version = "1.85"

[dependencies]

[lints.rust]
unsafe_code = "warn"
unsafe_op_in_unsafe_fn = "deny"

[lints.clippy]
all = "warn"
pedantic = "warn"

[profile.release]
panic = "abort"
```

`src/main.rs`:

```rust
//! Application entry point; feature logic belongs in its owning modules.

fn main() {
    println!("my_app: ready");
}
```

`Makefile` (recipe lines start with a tab):

```makefile
BIN := my_app
RUST_TARGET ?= x86_64-unknown-linux-musl
RUSTFLAGS ?= -C relocation-model=static -C link-arg=-no-pie
RUST_SOURCES := $(shell find src -type f -name '*.rs')

.PHONY: all clean
all: $(BIN)

$(BIN): Cargo.toml Cargo.lock $(RUST_SOURCES)
	env -u CARGO_ENCODED_RUSTFLAGS RUSTFLAGS="$(RUSTFLAGS)" \
		cargo +stable build --locked --release --target $(RUST_TARGET)
	cp target/$(RUST_TARGET)/release/$(BIN) ./$(BIN)

clean:
	cargo +stable clean
	rm -f $(BIN)
```

`.gitignore`:

```gitignore
/target/
/my_app
```

Run `cargo +stable generate-lockfile` from the app directory and commit the
lockfile. Keep `BIN`, the package's binary target, and the directory name in
agreement; document and implement an explicit mapping when they differ.
The userspace builder selects the Makefile, stages the app-root executable,
and removes that temporary executable afterward. Do not write to `rootfs/bin`
from the app's build recipe.

Install the musl target with `rustup target add --toolchain stable
x86_64-unknown-linux-musl`. The explicit target prevents an accidental host
build; the flags match existing Rust app integration. Cargo reads configuration
from the working directory and its ancestors, so check those files when
diagnosing unexpected flags. See the [Cargo configuration reference](https://doc.rust-lang.org/cargo/reference/config.html)
and [rustc relocation options](https://doc.rust-lang.org/rustc/codegen-options/index.html#relocation-model).

For checks, run from the app directory, selecting the host explicitly and
clearing guest flags so the host test harness can link normally:

```sh
cargo +stable fmt --check
RUST_HOST_TARGET=$(rustc +stable -vV | sed -n 's/^host: //p')
env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS \
    cargo +stable clippy --locked --all-targets --target "$RUST_HOST_TARGET" -- -D warnings
env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS \
    cargo +stable test --locked --target "$RUST_HOST_TARGET"
```

Keep pure logic tests independent of framebuffer/input devices and live guest
sockets. `twland`'s separated wire, input, framebuffer, and layer-shell modules
illustrate ownership boundaries; keep new entry points small rather than
copying its large `main.rs`.

From the repository root, run `make kernel` and `make userspace`. Inspect
`readelf -h rootfs/bin/my_app` and `readelf -l rootfs/bin/my_app`: this starter
must be an x86-64 `EXEC` with no `INTERP` segment. If shared linking is a
deliberate requirement, document the guest loader/library dependencies and
verify they are staged. Rebuild the image and exercise the app in Twilight;
host tests cannot validate the guest's syscall support or device ABI. Add
app documentation covering build/run commands, required interfaces, resource
ownership, and limitations. Services need deliberate twinit configuration and
shutdown behavior; a new app must not become a boot service by accident.

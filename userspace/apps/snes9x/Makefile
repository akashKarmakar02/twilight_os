# Snes9x — Twilight OS framebuffer port.
#
# Thin wrapper that delegates to the snes9x-fb source tree, exactly like the
# fbdoom app delegates to userspace/doomgeneric. The pinned source submodule
# is userspace/snes9x-fb; its twilight/ subfolder holds the real Makefile and
# the /dev/fb0 frontend.
#
# The Snes9x core is C++. Fedora's g++ targets glibc and produces binaries that
# won't run against the musl/libstdc++ in the Twilight rootfs, so the build is
# driven through the Alpine distrobox, whose g++ is musl-native. This keeps
# `make userspace` working unchanged from the Fedora host: this Makefile shells
# out to distrobox rather than calling g++ directly.
#
# No static linking — the binary uses /lib/ld-musl-x86_64.so.1 + libstdc++.so,
# both present in the rootfs.

# Resolve to an absolute path so distrobox can find the in-tree submodule.
# SNES_DIR may still point to another checkout for port development.
SNES_DIR ?= $(abspath ../../snes9x-fb/twilight)

# The Alpine distrobox name. Override with SNES_BOX=... if yours differs.
SNES_BOX ?= alpine

.PHONY: all clean

all:
	@test -f "$(SNES_DIR)/Makefile" || { \
		echo "Snes9x source missing: run git submodule update --init --recursive (see README.md)" >&2; \
		exit 1; \
	}
	# Force Alpine's native gcc/g++ (both musl-linked). build.rs exports
	# CC=musl-gcc, which propagates through distrobox but doesn't exist in
	# Alpine — override it here so the inner Makefile picks up the right tools.
	distrobox enter "$(SNES_BOX)" -- env CC=gcc CXX=g++ make -C "$(SNES_DIR)"
	cp "$(SNES_DIR)/snes9x" .

clean:
	distrobox enter "$(SNES_BOX)" -- env CC=gcc CXX=g++ make -C "$(SNES_DIR)" clean
	rm -f snes9x

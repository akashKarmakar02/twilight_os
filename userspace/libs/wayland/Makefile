# libwayland-client (+ server/cursor/egl) — cross-compiled for musl with meson.
#
# This gives twilight_os the official libwayland-client .so and generated
# protocol headers, so GUI apps can #include <wayland-client.h> and link
# -lwayland-client instead of hand-rolling the Wayland wire protocol.
#
# Build strategy:
#   - meson cross-build with musl-gcc (see musl-cross.ini).
#   - -Dscanner=false: do not cross-compile a scanner. meson then resolves the
#     *host* wayland-scanner as a native build dependency to generate the
#     protocol C/headers. The host scanner (1.25.0) matches this wayland
#     release, keeping the protocol XML in sync.
#   - The cross pkg-config wrapper (musl-pkg-config) points meson at the musl
#     libffi installed into the rootfs sysroot by ../libffi/.
#
# Driven by build_libraries() in userspace/build.rs via
# `make install ROOTFS=../../../rootfs`. Downloads the pinned release tarball on
# demand (sha256-verified) into a gitignored build/ dir.

VERSION ?= 1.25.0
TARBALL := wayland-$(VERSION).tar.gz
URL     := https://gitlab.freedesktop.org/wayland/wayland/-/archive/$(VERSION)/$(TARBALL)
SHA256  := 95413723f6cd0462770eda207a1c78d142d0a1bba9b0572a4ed1da6a9b27059c

ROOTFS ?= ../../../rootfs
PREFIX   := $(ROOTFS)/usr/x86_64-linux-musl
INCLUDEDIR := $(PREFIX)/include
LIBDIR   := $(PREFIX)/lib64

BUILD := build
SRC   := $(BUILD)/wayland-$(VERSION)
BUILDDIR := $(SRC)/_build

# meson resolves cross-file binary paths relative to the current working
# directory, not the cross-file's location, so a relative ./musl-pkg-config is
# unreliable. Generate the cross-file from a template with an absolute path to
# the wrapper (which lives alongside this Makefile).
CROSSFILE := $(CURDIR)/musl-cross.ini

MESON ?= meson

# scanner=false  -> use the host wayland-scanner (native) to generate protocol
#                   sources/headers; don't cross-compile one.
# libraries=true -> build libwayland-{client,server,cursor,egl}.
# tests/documentation/dtd_validation=false -> trim host-only build deps.
MESON_OPTS := \
	-Dscanner=false \
	-Dlibraries=true \
	-Dtests=false \
	-Ddocumentation=false \
	-Ddtd_validation=false \
	--prefix=/usr/x86_64-linux-musl \
	--libdir=lib64

.PHONY: all install clean distclean

all: $(BUILDDIR)/.built

$(CROSSFILE): musl-cross.ini.in
	sed -e 's|@CROSSFILE_DIR@|$(CURDIR)|g' $< > $@

$(SRC)/.extracted: $(BUILD)/$(TARBALL)
	cd $(BUILD) && sha256sum -c $(CURDIR)/$(BUILD)/sha256.txt
	cd $(BUILD) && tar xzf $(TARBALL)
	touch $@

$(BUILDDIR)/.setup: $(SRC)/.extracted $(CROSSFILE)
	$(MESON) setup --cross-file $(CROSSFILE) $(MESON_OPTS) $(BUILDDIR) $(SRC)
	touch $@

$(BUILDDIR)/.built: $(BUILDDIR)/.setup
	$(MESON) compile -C $(BUILDDIR)
	touch $@

install: all
	$(MESON) install -C $(BUILDDIR) --destdir $(CURDIR)/$(BUILD)/stage
	mkdir -p $(LIBDIR) $(INCLUDEDIR)
	# Shared libraries + soname/dev symlinks.
	cp -a $(BUILD)/stage/usr/x86_64-linux-musl/lib64/libwayland-*.so* $(LIBDIR)/
	# Public headers (wayland-client.h, generated *-protocol.h, wayland-util.h, ...).
	cp -a $(BUILD)/stage/usr/x86_64-linux-musl/include/wayland-*.h $(INCLUDEDIR)/

clean:
	rm -rf $(BUILDDIR) $(BUILD)/stage

distclean: clean
	rm -rf $(BUILD)

# Download + verify the tarball on demand.
$(BUILD)/$(TARBALL): $(BUILD)/sha256.txt | $(BUILD)
	curl -fsSL -o $@ $(URL)

$(BUILD)/sha256.txt: | $(BUILD)
	printf '%s  %s\n' '$(SHA256)' '$(TARBALL)' > $@

$(BUILD):
	mkdir -p $(BUILD)

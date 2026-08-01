# libffi — cross-compiled for the musl sysroot with musl-gcc.
#
# libwayland-client has a hard runtime dependency on libffi (ffi_call, used for
# event dispatch). The host ships only a glibc libffi, so we build libffi from
# source against musl and install the .so + headers into the rootfs sysroot
# where wayland's meson build finds it via the cross pkg-config wrapper.
#
# This Makefile is driven by build_libraries() in userspace/build.rs, which
# runs `make install ROOTFS=../../../rootfs`. It downloads the pinned release
# tarball on demand (with sha256 verification) into a gitignored build/ dir, so
# the upstream source is never committed to the tree.

VERSION ?= 3.5.2
TARBALL := libffi-$(VERSION).tar.gz
URL     := https://github.com/libffi/libffi/releases/download/v$(VERSION)/$(TARBALL)
SHA256  := f3a3082a23b37c293a4fcd1053147b371f2ff91fa7ea1b2a52e335676bac82dc

ROOTFS ?= ../../../rootfs
PREFIX   := $(ROOTFS)/usr/x86_64-linux-musl
INCLUDEDIR := $(PREFIX)/include
LIBDIR   := $(PREFIX)/lib64

BUILD := build
SRC   := $(BUILD)/libffi-$(VERSION)

# musl's sysroot has no linux/ UAPI headers, and libffi's static-exec
# trampoline code (src/tramp.c) includes <linux/limits.h>. Static trampolines
# are an optional optimization; closures (what wayland uses) work without them,
# so disable them rather than vendoring UAPI headers.
CONFIGURE_FLAGS := \
	--host=x86_64-linux-musl \
	--prefix=/usr/x86_64-linux-musl \
	--libdir=/usr/x86_64-linux-musl/lib64 \
	--includedir=/usr/x86_64-linux-musl/include \
	--enable-shared \
	--disable-static \
	--disable-exec-static-tramp

.PHONY: all install clean distclean

all: $(SRC)/.built

$(SRC)/.extracted: $(BUILD)/$(TARBALL)
	cd $(BUILD) && sha256sum -c $(CURDIR)/$(BUILD)/sha256.txt
	cd $(BUILD) && tar xzf $(TARBALL)
	touch $@

$(SRC)/.configured: $(SRC)/.extracted
	cd $(SRC) && CC=musl-gcc ./configure $(CONFIGURE_FLAGS)
	touch $@

$(SRC)/.built: $(SRC)/.configured
	$(MAKE) -C $(SRC) -j$$(nproc)
	touch $@

install: all
	$(MAKE) -C $(SRC) install DESTDIR=$(CURDIR)/$(BUILD)/stage
	mkdir -p $(LIBDIR) $(INCLUDEDIR) $(LIBDIR)/pkgconfig
	cp -a $(BUILD)/stage/usr/x86_64-linux-musl/lib64/libffi.so* $(LIBDIR)/
	cp -a $(BUILD)/stage/usr/x86_64-linux-musl/lib64/pkgconfig/libffi.pc $(LIBDIR)/pkgconfig/
	cp -a $(BUILD)/stage/usr/x86_64-linux-musl/include/ffi.h $(INCLUDEDIR)/
	cp -a $(BUILD)/stage/usr/x86_64-linux-musl/include/ffitarget.h $(INCLUDEDIR)/

clean:
	rm -rf $(SRC) $(BUILD)/stage

distclean: clean
	rm -rf $(BUILD)

# Download + verify the tarball on demand. The sha256 is pinned in this file and
# materialized into build/sha256.txt so `sha256sum -c` can verify it.
$(BUILD)/$(TARBALL): $(BUILD)/sha256.txt | $(BUILD)
	curl -fsSL -o $@ $(URL)

$(BUILD)/sha256.txt: | $(BUILD)
	printf '%s  %s\n' '$(SHA256)' '$(TARBALL)' > $@

$(BUILD):
	mkdir -p $(BUILD)

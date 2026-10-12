# libffi — musl cross-build from the pinned upstream Git submodule.
# build_libraries() runs `make install ROOTFS=../../../rootfs` here.
# Autotools generates ignored files in upstream/; objects and staging stay in
# build/<revision>/ so changing the gitlink selects a fresh configuration.

ROOTFS ?= ../../../rootfs
PREFIX := $(ROOTFS)/usr/x86_64-linux-musl
INCLUDEDIR := $(PREFIX)/include
LIBDIR := $(PREFIX)/lib64

SRC := $(CURDIR)/upstream
REVISION := $(shell git -C "$(SRC)" rev-parse --verify HEAD 2>/dev/null)
BUILD := $(CURDIR)/build/$(REVISION)
AUTOTOOLS_INPUTS := $(wildcard $(SRC)/configure.ac $(SRC)/Makefile.am $(SRC)/m4/*.m4)

# Musl lacks linux/limits.h, required only for optional static trampolines.
# Closures used by Wayland work without that optimization. Documentation is
# not installed into the guest, so the cross-build does not require texinfo.
CONFIGURE_FLAGS := \
	--host=x86_64-linux-musl \
	--prefix=/usr/x86_64-linux-musl \
	--libdir=/usr/x86_64-linux-musl/lib64 \
	--includedir=/usr/x86_64-linux-musl/include \
	--enable-shared \
	--disable-static \
	--disable-exec-static-tramp \
	--disable-docs

.PHONY: all install clean distclean check-source

all: $(BUILD)/Makefile
	$(MAKE) -C "$(BUILD)" -j$$(nproc)

check-source:
	@test -f "$(SRC)/configure.ac" || { \
		echo "libffi source missing: run git submodule update --init --recursive" >&2; \
		exit 1; \
	}

$(BUILD)/.bootstrap: $(AUTOTOOLS_INPUTS) | check-source
	mkdir -p "$(BUILD)"
	cd "$(SRC)" && ./autogen.sh
	touch "$@"

$(BUILD)/Makefile: $(BUILD)/.bootstrap
	cd "$(BUILD)" && CC=musl-gcc "$(SRC)/configure" $(CONFIGURE_FLAGS)

install: all
	$(MAKE) -C "$(BUILD)" install DESTDIR="$(BUILD)/stage"
	mkdir -p $(LIBDIR) $(INCLUDEDIR) $(LIBDIR)/pkgconfig
	cp -a $(BUILD)/stage/usr/x86_64-linux-musl/lib64/libffi.so* $(LIBDIR)/
	cp -a $(BUILD)/stage/usr/x86_64-linux-musl/lib64/pkgconfig/libffi.pc $(LIBDIR)/pkgconfig/
	cp -a $(BUILD)/stage/usr/x86_64-linux-musl/include/ffi.h $(INCLUDEDIR)/
	cp -a $(BUILD)/stage/usr/x86_64-linux-musl/include/ffitarget.h $(INCLUDEDIR)/

clean:
	rm -rf build

distclean: clean

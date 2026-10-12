# Wayland — musl cross-build from the pinned upstream Git submodule.
# A native scanner built from the same pin generates protocol code; libffi
# is cross-built by ../libffi first. Outputs live in build/<revision>/ so a new
# gitlink cannot reuse the preceding source revision's Meson configuration.

ROOTFS ?= ../../../rootfs
PREFIX := $(ROOTFS)/usr/x86_64-linux-musl
INCLUDEDIR := $(PREFIX)/include
LIBDIR := $(PREFIX)/lib64

SRC := $(CURDIR)/upstream
REVISION := $(shell git -C "$(SRC)" rev-parse --verify HEAD 2>/dev/null)
BUILD := $(CURDIR)/build/$(REVISION)
CROSSFILE := $(CURDIR)/build/musl-cross.ini
NATIVEFILE := $(BUILD)/native.ini
SCANNER_BUILD := $(BUILD)/scanner
SCANNER_PREFIX := $(BUILD)/host
HOST_CC ?= cc
MESON ?= meson

# Keep the scanner native; only the libraries target the guest musl sysroot.
MESON_OPTS := \
	-Dscanner=false \
	-Dlibraries=true \
	-Dtests=false \
	-Ddocumentation=false \
	-Ddtd_validation=false \
	--prefix=/usr/x86_64-linux-musl \
	--libdir=lib64

.PHONY: all install clean distclean check-source native-scanner

all: $(BUILD)/.setup
	$(MESON) compile -C "$(BUILD)"

check-source:
	@test -f "$(SRC)/meson.build" || { \
		echo "Wayland source missing: run git submodule update --init --recursive" >&2; \
		exit 1; \
	}

$(CROSSFILE): musl-cross.ini.in Makefile
	mkdir -p "$(@D)"
	sed -e 's|@CROSSFILE_DIR@|$(CURDIR)|g' $< > "$@"

# scanner=false in the cross-build still requires a native scanner with an
# exact version match. Build that tool separately rather than depending on the
# host distribution's Wayland version. Its Expat dependency remains native.
$(SCANNER_BUILD)/build.ninja: | check-source
	CC="$(HOST_CC)" $(MESON) setup -Dlibraries=false -Dscanner=true \
		-Dtests=false -Ddocumentation=false -Ddtd_validation=false \
		--prefix="$(SCANNER_PREFIX)" --libdir=lib "$(SCANNER_BUILD)" "$(SRC)"

native-scanner: $(SCANNER_BUILD)/build.ninja
	$(MESON) compile -C "$(SCANNER_BUILD)"
	$(MESON) install -C "$(SCANNER_BUILD)"

$(NATIVEFILE): Makefile
	mkdir -p "$(@D)"
	printf '%s\n' '[built-in options]' \
		"pkg_config_path = ['$(SCANNER_PREFIX)/lib/pkgconfig']" > "$@"

$(BUILD)/.setup: $(CROSSFILE) $(NATIVEFILE) | native-scanner
	$(MESON) setup --reconfigure --native-file "$(NATIVEFILE)" \
		--cross-file "$(CROSSFILE)" $(MESON_OPTS) "$(BUILD)" "$(SRC)"
	touch "$@"

install: all
	$(MESON) install -C "$(BUILD)" --destdir "$(BUILD)/stage"
	mkdir -p $(LIBDIR) $(INCLUDEDIR)
	cp -a $(BUILD)/stage/usr/x86_64-linux-musl/lib64/libwayland-*.so* $(LIBDIR)/
	cp -a $(BUILD)/stage/usr/x86_64-linux-musl/include/wayland-*.h $(INCLUDEDIR)/

clean:
	rm -rf build

distclean: clean

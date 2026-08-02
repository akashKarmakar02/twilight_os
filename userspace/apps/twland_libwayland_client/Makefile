# twland_libwayland_client - libwayland-client demo against twland.
#
# Links the official libwayland-client.so (cross-built for musl by issue #57's
# userspace/libs/wayland) and generates the xdg-shell bindings from the vendored
# protocol XML files with the host wayland-scanner.

CC := musl-gcc
CFLAGS ?= -O2 -Wall -Wextra -std=c11
CPPFLAGS ?= -I. -I../../../rootfs/usr/x86_64-linux-musl/include
LDFLAGS ?= -L../../../rootfs/usr/x86_64-linux-musl/lib64 -Wl,-rpath,/usr/x86_64-linux-musl/lib64
LDLIBS ?= -lwayland-client

# Generated xdg-shell bindings from the vendored protocol XML. The client header
# has typed static-inline request wrappers; the public-code C source defines the
# interface structs (xdg_wm_base_interface, ...) the wrappers reference.
WAYLAND_SCANNER ?= wayland-scanner
XDG_XML := ../../protocols/xdg-shell.xml
XDG_HEADER := xdg-shell-client-protocol.h
XDG_SRC := xdg-shell-protocol.c
XDG_OBJ := xdg-shell-protocol.o
LAYER_XML := ../../protocols/wlr-layer-shell-unstable-v1.xml
LAYER_HEADER := wlr-layer-shell-unstable-v1-client-protocol.h
LAYER_SRC := wlr-layer-shell-unstable-v1-protocol.c
LAYER_OBJ := wlr-layer-shell-unstable-v1-protocol.o

BIN := twland_libwayland_client
SRCS := main.c buffer.c registry.c
OBJS := $(SRCS:.c=.o) $(XDG_OBJ) $(LAYER_OBJ)

.PHONY: all clean

all: $(BIN)

$(XDG_HEADER): $(XDG_XML)
	$(WAYLAND_SCANNER) client-header $< $@

$(XDG_SRC): $(XDG_XML)
	$(WAYLAND_SCANNER) public-code $< $@

$(LAYER_HEADER): $(LAYER_XML)
	$(WAYLAND_SCANNER) client-header $< $@

$(LAYER_SRC): $(LAYER_XML)
	$(WAYLAND_SCANNER) public-code $< $@

# App objects depend on both generated shell protocol headers.
$(SRCS:.c=.o): $(XDG_HEADER) $(LAYER_HEADER)

%.o: %.c
	$(CC) $(CPPFLAGS) $(CFLAGS) -c $< -o $@

$(XDG_OBJ): $(XDG_SRC) $(XDG_HEADER)
	$(CC) $(CPPFLAGS) $(CFLAGS) -c $(XDG_SRC) -o $@

$(LAYER_OBJ): $(LAYER_SRC) $(LAYER_HEADER)
	$(CC) $(CPPFLAGS) $(CFLAGS) -c $(LAYER_SRC) -o $@

$(BIN): $(OBJS)
	$(CC) $(CFLAGS) -o $@ $^ $(LDFLAGS) $(LDLIBS)

clean:
	rm -f $(BIN) $(OBJS) $(XDG_HEADER) $(XDG_SRC) $(LAYER_HEADER) $(LAYER_SRC)

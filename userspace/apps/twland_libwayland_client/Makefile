# twland_libwayland_client - libwayland-client demo against twland.
#
# Links the official libwayland-client.so (cross-built for musl by issue #57's
# userspace/libs/wayland) and generates the xdg-shell bindings from the vendored
# protocol XML with the host wayland-scanner.

CC := musl-gcc
CFLAGS ?= -O2 -Wall -Wextra -std=c11
CPPFLAGS ?= -I. -I../../../rootfs/usr/x86_64-linux-musl/include
LDFLAGS ?= -L../../../rootfs/usr/x86_64-linux-musl/lib64 -Wl,-rpath,/usr/x86_64-linux-musl/lib64
LDLIBS ?= -lwayland-client

# Generated xdg-shell bindings from the vendored protocol XML. The client header
# has typed static-inline request wrappers; the public-code C source defines the
# interface structs (xdg_wm_base_interface, ...) the wrappers reference.
WAYLAND_SCANNER ?= wayland-scanner
PROTOCOL_DIR := protocol
XDG_XML := $(PROTOCOL_DIR)/xdg-shell.xml
XDG_HEADER := xdg-shell-client-protocol.h
XDG_SRC := xdg-shell-protocol.c
XDG_OBJ := xdg-shell-protocol.o

BIN := twland_libwayland_client
SRC_DIR := .
SRCS := $(wildcard $(SRC_DIR)/*.c)
OBJS := $(SRCS:.c=.o) $(XDG_OBJ)

.PHONY: all clean

all: $(BIN)

$(XDG_HEADER): $(XDG_XML)
	$(WAYLAND_SCANNER) client-header $< $@

$(XDG_SRC): $(XDG_XML)
	$(WAYLAND_SCANNER) public-code $< $@

# App objects depend on the generated xdg-shell header.
$(SRCS:.c=.o): $(XDG_HEADER)

%.o: %.c
	$(CC) $(CPPFLAGS) $(CFLAGS) -c $< -o $@

$(XDG_OBJ): $(XDG_SRC) $(XDG_HEADER)
	$(CC) $(CPPFLAGS) $(CFLAGS) -c $(XDG_SRC) -o $@

$(BIN): $(OBJS)
	$(CC) $(CFLAGS) -o $@ $^ $(LDFLAGS) $(LDLIBS)

clean:
	rm -f $(BIN) $(OBJS) $(XDG_HEADER) $(XDG_SRC)

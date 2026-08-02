# twclock - a small native layer-shell clock for Twilight OS.

ifeq ($(origin CC), default)
CC := musl-gcc
endif

CFLAGS ?= -O2 -Wall -Wextra -Werror -std=c11
CPPFLAGS ?= -I. -I../../../rootfs/usr/x86_64-linux-musl/include
LDFLAGS ?= -L../../../rootfs/usr/x86_64-linux-musl/lib64 -Wl,-rpath,/usr/x86_64-linux-musl/lib64
LDLIBS ?= -lwayland-client

WAYLAND_SCANNER ?= wayland-scanner
LAYER_XML := ../../protocols/wlr-layer-shell-unstable-v1.xml
LAYER_HEADER := wlr-layer-shell-unstable-v1-client-protocol.h
LAYER_SOURCE := wlr-layer-shell-unstable-v1-protocol.c
LAYER_OBJECT := wlr-layer-shell-unstable-v1-protocol.o
XDG_XML := ../../protocols/xdg-shell.xml
XDG_SOURCE := xdg-shell-protocol.c
XDG_OBJECT := xdg-shell-protocol.o

BIN := twclock
SOURCES := main.c wayland_app.c pointer_input.c shm_buffer.c clock_face.c
OBJECTS := $(SOURCES:.c=.o) $(LAYER_OBJECT) $(XDG_OBJECT)

.PHONY: all clean

all: $(BIN)

$(LAYER_HEADER): $(LAYER_XML)
	$(WAYLAND_SCANNER) client-header $< $@

$(LAYER_SOURCE): $(LAYER_XML)
	$(WAYLAND_SCANNER) public-code $< $@

$(XDG_SOURCE): $(XDG_XML)
	$(WAYLAND_SCANNER) public-code $< $@

$(SOURCES:.c=.o): $(LAYER_HEADER)

%.o: %.c
	$(CC) $(CPPFLAGS) $(CFLAGS) -c $< -o $@

$(LAYER_OBJECT): $(LAYER_SOURCE) $(LAYER_HEADER)
	$(CC) $(CPPFLAGS) $(CFLAGS) -c $(LAYER_SOURCE) -o $@

$(XDG_OBJECT): $(XDG_SOURCE)
	$(CC) $(CPPFLAGS) $(CFLAGS) -c $(XDG_SOURCE) -o $@

$(BIN): $(OBJECTS)
	$(CC) $(CFLAGS) -o $@ $^ $(LDFLAGS) $(LDLIBS)

clean:
	rm -f $(BIN) $(OBJECTS) $(LAYER_HEADER) $(LAYER_SOURCE) $(XDG_SOURCE)

CC := musl-gcc
CFLAGS ?= -O2 -fPIC -Wall -Wextra -std=c11
LDFLAGS ?= -shared -Wl,-soname,libtwigl.so.0

ROOTFS ?= ../../../rootfs
INCLUDEDIR := $(ROOTFS)/usr/x86_64-linux-musl/include
LIBDIR := $(ROOTFS)/usr/x86_64-linux-musl/lib64

LIB := libtwigl.so
SONAME := libtwigl.so.0
SRC_DIR := src
SRCS := $(wildcard $(SRC_DIR)/*.c)
OBJS := $(SRCS:.c=.o)

.PHONY: all install clean

all: $(LIB)

$(SRC_DIR)/%.o: $(SRC_DIR)/%.c src/twigl.h
	$(CC) $(CFLAGS) -c $< -o $@

$(LIB): $(OBJS)
	$(CC) $(LDFLAGS) -o $@ $^

install: all
	mkdir -p $(INCLUDEDIR) $(LIBDIR)
	cp src/twigl.h $(INCLUDEDIR)/twigl.h
	cp $(LIB) $(LIBDIR)/$(SONAME)
	cp $(LIB) $(LIBDIR)/$(LIB)

clean:
	rm -f $(LIB) $(OBJS)

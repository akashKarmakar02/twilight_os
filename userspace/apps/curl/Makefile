CC := musl-gcc
CFLAGS ?= -O2 -Wall -Wextra
CPPFLAGS ?= -I../../../rootfs/usr/x86_64-linux-musl/include -I/usr/x86_64-linux-musl/include -I/usr/local/include -I/usr/include
LDFLAGS ?= -L../../../rootfs/usr/x86_64-linux-musl/lib64 -Wl,-rpath,/usr/x86_64-linux-musl/lib64
LDLIBS ?= -l:libwolfssl.so.42

BIN := curl
SRC_DIR := src
SRCS := $(wildcard $(SRC_DIR)/*.c)
OBJS := $(SRCS:.c=.o)

.PHONY: all clean

all: $(BIN)

$(SRC_DIR)/%.o: $(SRC_DIR)/%.c
	$(CC) $(CPPFLAGS) $(CFLAGS) -c $< -o $@

$(BIN): $(OBJS)
	$(CC) $(CFLAGS) -o $@ $^ $(LDFLAGS) $(LDLIBS)

clean:
	rm -f $(BIN) $(OBJS)

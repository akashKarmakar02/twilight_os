CC ?= musl-gcc
CFLAGS ?= -O2 -static -Wall -Wextra
CCACHE_DISABLE ?= 1

BIN := mkdir
SRC_DIR := src
SRCS := $(wildcard $(SRC_DIR)/*.c)
OBJS := $(SRCS:.c=.o)

.PHONY: all clean

all: $(BIN)

$(SRC_DIR)/%.o: $(SRC_DIR)/%.c
	CCACHE_DISABLE=$(CCACHE_DISABLE) $(CC) $(CFLAGS) -c $< -o $@

$(BIN): $(OBJS)
	CCACHE_DISABLE=$(CCACHE_DISABLE) $(CC) $(CFLAGS) -o $@ $^

clean:
	rm -f $(BIN) $(OBJS)

CC := musl-gcc
CFLAGS ?= -O2 -Wall -Wextra -std=c11
LDLIBS ?= -ljpeg

BIN := imgview
SRC_DIR := src
SRCS := $(wildcard $(SRC_DIR)/*.c)
OBJS := $(SRCS:.c=.o)

.PHONY: all clean

all: $(BIN)

$(SRC_DIR)/%.o: $(SRC_DIR)/%.c
	$(CC) $(CFLAGS) -c $< -o $@

$(BIN): $(OBJS)
	$(CC) $(CFLAGS) -o $@ $^ $(LDLIBS)

clean:
	rm -f $(BIN) $(OBJS)

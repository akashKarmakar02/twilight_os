CC ?= musl-gcc
CFLAGS ?= -O2 -static -Wall -Wextra -std=c11

BIN := chip8
SRC_DIR := src
SRCS := $(wildcard $(SRC_DIR)/*.c)
OBJS := $(SRCS:.c=.o)

.PHONY: all clean

all: $(BIN)

$(SRC_DIR)/%.o: $(SRC_DIR)/%.c
	$(CC) $(CFLAGS) -c $< -o $@

$(BIN): $(OBJS)
	$(CC) $(CFLAGS) -o $@ $^

clean:
	rm -f $(BIN) $(OBJS)

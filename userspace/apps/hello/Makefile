CC ?= musl-gcc
CFLAGS ?= -O2 -static -Wall -Wextra

BIN := hello
SRC_DIR := src
SRCS := $(wildcard $(SRC_DIR)/*.c)
OBJS := $(SRCS:.c=.o)

.PHONY: all clean install

all: $(BIN)

# explicit pattern rule to create src/foo.o from src/foo.c
$(SRC_DIR)/%.o: $(SRC_DIR)/%.c
	$(CC) $(CFLAGS) -c $< -o $@

# link
$(BIN): $(OBJS)
	$(CC) $(CFLAGS) -o $@ $^

clean:
	rm -f $(BIN) $(OBJS)
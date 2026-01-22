CC ?= musl-gcc
CFLAGS ?= -O2 -static
CCACHE_DISABLE ?= 1
BIN = tsh

SRC_DIR := src
SRCS := $(wildcard $(SRC_DIR)/*.c)
OBJ_DIR := build
OBJS := $(patsubst $(SRC_DIR)/%.c,$(OBJ_DIR)/%.o,$(SRCS))


all: $(BIN)

# explicit pattern rule to create build/foo.o from src/foo.c
$(OBJ_DIR)/%.o: $(SRC_DIR)/%.c
	@mkdir -p $(OBJ_DIR)
	CCACHE_DISABLE=$(CCACHE_DISABLE) $(CC) $(CFLAGS) -c $< -o $@

# link
$(BIN): $(OBJS)
	CCACHE_DISABLE=$(CCACHE_DISABLE) $(CC) $(CFLAGS) -o $@ $^

clean:
	rm -f $(BIN) $(OBJS)
	rm -rf $(OBJ_DIR)

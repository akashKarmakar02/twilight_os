CC ?= musl-gcc
CFLAGS ?= -O2 -static
BIN = tsh

SRC = $(wildcard src/*.c)
OBJ = $(SRC:.c=.o)

all: $(BIN)

$(BIN): $(OBJ)
	$(CC) $(CFLAGS) -o $@ $^

clean:
	rm -f $(BIN) $(OBJ)

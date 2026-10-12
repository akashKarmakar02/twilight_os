CC ?= musl-gcc
CFLAGS ?= -O2 -static -Wall -std=c99 \
	-Wno-parentheses \
	-Wno-misleading-indentation \
	-Wno-bool-operation \
	-Wno-discarded-qualifiers \
	-Wno-incompatible-pointer-types-discards-qualifiers \
	-Wno-unknown-warning-option \
	-Wno-switch-outside-range \
	-Wno-unused-value \
	-Wno-char-subscripts \
	-Wno-switch

# Host C compiler for the pacing-arithmetic unit tests (not the target build).
HOST_CC ?= cc

BIN := smolnes
SRC := deobfuscated.c
PACING_HDR := pacing.h
TEST_BIN := test_pacing
TEST_SRC := test_pacing.c

.PHONY: all clean test

all: $(BIN)

$(BIN): $(SRC) $(PACING_HDR)
	$(CC) $(CFLAGS) -o $@ $<

# Host unit tests for the pure pacing helpers in pacing.h. Built and run with
# the host compiler; no framebuffer or kernel required.
test: $(TEST_BIN)
	./$(TEST_BIN)

$(TEST_BIN): $(TEST_SRC) $(PACING_HDR)
	$(HOST_CC) -O2 -Wall -std=c99 -o $@ $(TEST_SRC)

clean:
	rm -f $(BIN) $(TEST_BIN)

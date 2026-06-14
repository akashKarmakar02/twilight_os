BIN := twinit
RUST_TARGET ?= x86_64-unknown-linux-musl

.PHONY: all clean

all: $(BIN)

$(BIN):
	cargo build --release --target $(RUST_TARGET)
	cp target/$(RUST_TARGET)/release/$(BIN) ./$(BIN)

clean:
	cargo clean
	rm -f $(BIN)

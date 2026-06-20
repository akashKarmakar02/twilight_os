BIN := twinitctl
RUST_TARGET ?= x86_64-unknown-linux-musl
RUSTFLAGS ?= -C relocation-model=static -C link-arg=-no-pie

.PHONY: all clean

all: $(BIN)

$(BIN):
	env -u CARGO_ENCODED_RUSTFLAGS RUSTFLAGS="$(RUSTFLAGS)" \
		cargo build --release --target $(RUST_TARGET)
	cp target/$(RUST_TARGET)/release/$(BIN) ./$(BIN)

clean:
	cargo clean
	rm -f $(BIN)

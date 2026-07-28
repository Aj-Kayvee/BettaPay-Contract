SORBORN ?= soroban

.PHONY: build
build:
	@echo "Building all contracts..."
	cargo build --target wasm32-unknown-unknown --release

.PHONY: optimize
optimize: build
	@mkdir -p target/optimized
	@for contract in $(shell find . -path "*/target/wasm32-unknown-unknown/release/*.wasm" -type f); do \
		output=$$(basename $$contract .wasm)_opt.wasm; \
		$(SORBORN) contract optimize --wasm $$contract --optimized-wasm target/optimized/$$output; \
	done

.PHONY: clean
clean:
	cargo clean
	@rm -rf target/optimized

.PHONY: test check clippy all

test:
	cargo test --workspace

check:
	cargo check --workspace

clippy:
	cargo clippy --workspace -- -D warnings

all: fmt check clippy test test_optimized wasm_size

.PHONY: setup check test lint fmt fmt-check web-check web-build build run

RUST_PACKAGES = -p nipa-core -p nipa-download -p nipa-match -p nipa-providers -p nipa-scanner -p nipa-server -p nipa-stream

setup:
	git submodule update --init --recursive
	cd webui/app && npm ci

test:
	cargo test --workspace --all-targets --locked

lint:
	cargo clippy --workspace --all-targets -- -D warnings

fmt:
	cargo fmt $(RUST_PACKAGES)

fmt-check:
	cargo fmt $(RUST_PACKAGES) -- --check

web-check:
	cd webui/app && npm run check

check: fmt-check test lint web-check

web-build:
	cd webui/app && npm run build

build: web-build
	cargo build --release --locked -p nipa-server

run:
	cargo run -p nipa-server

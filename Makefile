.PHONY: setup check test lint fmt fmt-check agent-test agent-lint web-check web-build build run

RUST_PACKAGES = -p nipa-core -p nipa-download -p nipa-match -p nipa-providers -p nipa-scanner -p nipa-server -p nipa-stream
AGENT_MANIFEST = nipa-agent/Cargo.toml

setup:
	git submodule update --init --recursive
	cd webui/app && npm ci

test:
	cargo test --workspace --all-targets --locked
	cargo test --manifest-path $(AGENT_MANIFEST) --all-targets

lint:
	cargo clippy --workspace --all-targets -- -D warnings
	cargo clippy --manifest-path $(AGENT_MANIFEST) --all-targets -- -D warnings

fmt:
	cargo fmt $(RUST_PACKAGES)
	cargo fmt --manifest-path $(AGENT_MANIFEST)

fmt-check:
	cargo fmt $(RUST_PACKAGES) -- --check
	cargo fmt --manifest-path $(AGENT_MANIFEST) -- --check

agent-test:
	cargo test --manifest-path $(AGENT_MANIFEST) --all-targets

agent-lint:
	cargo clippy --manifest-path $(AGENT_MANIFEST) --all-targets -- -D warnings

web-check:
	cd webui/app && npm run check

check: fmt-check test lint web-check

web-build:
	cd webui/app && npm run build

build: web-build
	cargo build --release --locked -p nipa-server

run:
	cargo run -p nipa-server

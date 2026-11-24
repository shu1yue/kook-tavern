.PHONY: check fmt clippy test build local

check: fmt clippy test

fmt:
	cargo fmt --all -- --check

clippy:
	cargo clippy --workspace --all-targets -- -D warnings

test:
	cargo test --workspace

build:
	cargo build --release -p kook-tavern-dm

local:
	cargo run -p kook-tavern-dm -- local --mock-llm

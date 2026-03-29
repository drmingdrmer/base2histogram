all: check

test:
	cargo test

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

lint:
	cargo clippy --all-targets --all-features -- -D warnings

doc:
	RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items

check: fmt-check lint test doc

clean:
	cargo clean

.PHONY: all test fmt fmt-check lint doc check clean

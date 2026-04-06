all: check

test:
	cargo test

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

clippy-fix:
	cargo clippy --fix --allow-dirty --allow-staged

lint: fmt clippy-fix
	cargo clippy --all-targets --all-features -- -D warnings

doc:
	RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items

check: lint fmt-check test doc

coverage:
	cargo llvm-cov --html
	@echo "Coverage report: target/llvm-cov/html/index.html"

clean:
	cargo clean

.PHONY: all test fmt fmt-check lint doc check coverage clean

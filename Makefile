.PHONY: format test lint check tags format-docs
.DEFAULT_GOAL := check

start:
	docker compose run --build bot

format:
	cargo fmt --all

test:
	cargo test

lint:
	cargo clippy --all-targets --all-features -- -D warnings

check: lint test

tags:
	ctags -R --languages=Rust --exclude=target --exclude=.git --fields=+niazS --extras=+q --output-format=e-ctags

format-docs:
	@echo "Formatting Markdown documents..."
	@npx -y prettier --list-different --write $(shell find . -name "*.md")

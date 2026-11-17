.PHONY: start shell format test lint check tags format-docs
.DEFAULT_GOAL := check

# Removes the one-off container and any orphaned containers when the bot exits, so a stopped run does
# not linger in `docker compose ps`. Volumes are kept.
start:
	trap 'docker compose down --remove-orphans' EXIT; \
	docker compose run --build --rm bot

shell:
	docker compose run bot bash

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

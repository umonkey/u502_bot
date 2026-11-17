# u502_bot

## Project overview

u502_bot is a Rust Telegram bot for a local car community.

The current scope is CLI scaffolding: `u502 start` logs `Starting the Telegram bot.` and exits. The Telegram bot loop and the echo behavior described in `task.md` are not implemented yet.

## Prerequisites

- Rust stable (edition 2024): install through rustup; `rust-toolchain.toml` pins the stable channel with the `rustfmt` and `clippy` components.

- GNU Make: runs the canonical commands below.

- universal-ctags (optional): required only for `make tags`.

- `uvx` (optional): required only for `make format-docs`.

- Docker: host only; images are published by CI, not built in this development container.

## Commands

Run these from the repository root. They are the canonical verification commands for the project.

```bash
make format       # cargo fmt --all
make test         # cargo test
make lint         # cargo clippy --all-targets --all-features -- -D warnings
make check        # lint + test
make tags         # ctags index (requires universal-ctags)
make format-docs  # mdformat on docs/ (requires uvx)
cargo run -- start
```

Running `make` without a target executes `check`.

## Layout

- `src/main.rs`: entry point; calls `cli::run()`.

- `src/cli/mod.rs`: defines the clap `Command` enum, parses arguments, and routes each variant.

- `src/cli/<command>.rs`: one module per subcommand, such as `src/cli/start.rs`.

Add every new subcommand as a variant of the `Command` enum in `src/cli/mod.rs`, route it from `run()`, and implement it in `src/cli/<command>.rs`.

## Code style

- Target edition 2024 on the stable toolchain.

- Clippy must pass with `-D warnings`; `make lint` enforces this.

- Run `cargo fmt` (or `make format`) before committing.

- Avoid `unwrap()` in runtime paths; return or handle errors explicitly.

## Docker

The image uses a two-stage build: `rust:1-slim-trixie` compiles the release binary, and `debian:trixie-slim` runs it with `supervisor` and `cron`.

`supervisord` is PID 1. It supervises `cron -f` as root and the bot as the unprivileged `u502` user.

Images are published by CI, not built in development containers. The development container has no Docker daemon, so `docker build` and `docker run` fail here. On a host with Docker:

```bash
docker build -t u502_bot .
docker run --rm u502_bot
```

`compose.yaml` defines a single `bot` service. It builds from this checkout when given `--build`, and otherwise pulls the published image. `RUST_LOG` defaults to `info` and can be overridden through the environment.

```bash
docker compose up --build      # build locally and run in the foreground
docker compose pull            # fetch the published image instead
docker compose up -d           # start detached
```

## Architecture decisions

Architecture decisions live in `docs/adr/`, one Markdown file per decision, named `NNNN-slug.md`.

ADR 0001 records the decision to use Rust.

Do not add `docs/adr/README.md` until a second ADR exists.

Run `make format-docs` after editing anything under `docs/`.

## Continuous integration

Pull requests run `make check` plus a formatting check with `cargo fmt --all -- --check`.

Pushes to `master` publish `ghcr.io/umonkey/u502_bot:latest` to GHCR. The publish workflow does not re-run tests; it relies on the required PR checks.

## Secrets

`.env` is git-ignored and docker-ignored. Never commit it or copy it into an image.

Never bake tokens into the image or source. The future bot token is read from environment variables at runtime.

## Out of scope

- Project level skills live in `.agents/skills`.

# u502_bot

## Project overview

u502_bot is a Rust Telegram bot for a local car community.

`u502 start` connects to Telegram with long polling through `teloxide` and logs every incoming message. The token comes from `TELEGRAM_API_TOKEN`, or from the file named by `TELEGRAM_API_TOKEN_FILE` when that is set.

## Prerequisites

- Rust stable (edition 2024): install through rustup; `rust-toolchain.toml` pins the stable channel with the `rustfmt` and `clippy` components.

- GNU Make: runs the canonical commands below.

- A Telegram bot token from BotFather: required to run `cargo run -- start`; not needed for any `make` target.

- universal-ctags (optional): required only for `make tags`.

- Node and `npx` (optional): required only for `make format-docs`.

- Docker: host only; images are published by CI, not built in this development container.

## Commands

Run these from the repository root. They are the canonical verification commands for the project.

```bash
make format       # cargo fmt --all
make test         # cargo test
make lint         # cargo clippy --all-targets --all-features -- -D warnings
make check        # lint + test
make tags         # ctags index (requires universal-ctags)
make format-docs  # prettier on all Markdown (requires Node/npx)
TELEGRAM_API_TOKEN=... cargo run -- start
```

Running `make` without a target executes `check`.

## Layout

- `src/main.rs`: entry point; calls `cli::run()`.

- `src/cli/mod.rs`: defines the clap `Command` enum, parses arguments, and routes each variant.

- `src/cli/<command>.rs`: one module per subcommand, such as `src/cli/start.rs`.

- `src/infra.rs`: declares the infra modules.

- `src/infra/config.rs`: `get_secret` and `SecretError`, with `<NAME>_FILE` precedence.

- `src/infra/telegram_bot.rs`: `run` and `handle_message`, the long-polling loop.

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

Compose reads the token from `.secrets/telegram_api_token`, so `docker compose up` fails immediately until that file exists; see Secrets.

```bash
docker compose up --build      # build locally and run in the foreground
docker compose pull            # fetch the published image instead
docker compose up -d           # start detached
```

## Architecture decisions

Architecture decisions live in `docs/adr/`, one Markdown file per decision, named `NNNN-slug.md`.

ADR 0001 records the decision to use Rust.

ADR 0003 records the decision to receive updates by long polling.

Do not add `docs/adr/README.md` until a second ADR exists.

Run `make format-docs` after editing anything under `docs/`.

## Continuous integration

Pull requests run `make check` plus a formatting check with `cargo fmt --all -- --check`.

Pushes to `master` publish `ghcr.io/umonkey/u502_bot:latest` to GHCR. The publish workflow does not re-run tests; it relies on the required PR checks.

## Secrets

The bot token can be supplied in three ways:

- `TELEGRAM_API_TOKEN`: the token itself, convenient for local runs.

- `TELEGRAM_API_TOKEN_FILE`: the path of a file holding the token.

- Compose: sets `TELEGRAM_API_TOKEN_FILE=/run/secrets/telegram_api_token` and mounts `.secrets/telegram_api_token` there, so the token never enters the container environment or `docker inspect`.

`TELEGRAM_API_TOKEN_FILE` wins when both variables are set, and one trailing newline in the file is ignored. Create the file before running Compose:

```bash
mkdir -p .secrets
printf '%s' "$TELEGRAM_API_TOKEN" > .secrets/telegram_api_token
chmod 600 .secrets/telegram_api_token
```

`.secrets/` and `.env` are git-ignored and docker-ignored. Never commit them or copy them into an image.

Never bake tokens into the image or source. The token is read from the environment at runtime.

## Out of scope

- Project level skills live in `.agents/skills`.

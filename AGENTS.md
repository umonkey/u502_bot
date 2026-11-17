# u502_bot

## Project overview

u502_bot is a Rust Telegram bot for a local car community.

`u502 start` connects to Telegram with long polling through `teloxide`, logs every incoming message, and stores it in SQLite. The token comes from `TELEGRAM_API_TOKEN`, or from the file named by `TELEGRAM_API_TOKEN_FILE` when that is set.

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
TELEGRAM_API_TOKEN=... DATABASE_PATH=... cargo run -- start
```

Running `make` without a target executes `check`.

## Layout

- `src/main.rs`: entry point; calls `cli::run()`.
- `src/cli/mod.rs`: defines the clap `Command` enum, parses arguments, and routes each variant.
- `src/cli/<command>.rs`: one module per subcommand; `src/cli/start.rs` composes the clients and the repository.
- `src/domains.rs`: declares the domain modules.
- `src/domains/attachment.rs`: the attachment model and the repository that owns the `attachments` SQL.
- `src/domains/message.rs`: the message model and the repository that owns the `messages` SQL.
- `src/infra.rs`: declares the infra modules.
- `src/infra/config.rs`: `get_secret` and `SecretError`, with `<NAME>_FILE` precedence.
- `src/infra/database.rs`: `Database`, `DatabaseError`, `database_path`, and `SCHEMA`; the async SQLite client, which opens a pooled connection and applies the embedded schema without knowing any table.
- `src/infra/sentry.rs`: `SENTRY_DSN`, `init` (the `tracing` subscriber and optional Sentry client), and the `message_received` / `message_stored` metric functions.
- `src/infra/telegram_bot.rs`: `messages` and `Messages`, the Telegram client; it delivers messages and knows nothing about storage.
- `dev/schema.sql`: initial SQLite schema, embedded at build time and applied on every startup.

Add every new subcommand as a variant of the `Command` enum in `src/cli/mod.rs`, route it from `run()`, and implement it in `src/cli/<command>.rs`.

## Layering

- `src/infra/*`: clients for exactly one external system; they never call each other, hold SQL, or contain domain logic.
- `src/domains/*`: models and repositories; each repository owns the SQL for its data.
- `src/cli/*`: commands compose the clients and the repositories; the Telegram client must not know that a database exists.

## Code style

- Target edition 2024 on the stable toolchain.
- Clippy must pass with `-D warnings`; `make lint` enforces this.
- Run `cargo fmt` (or `make format`) before committing.
- Avoid `unwrap()` in runtime paths; return or handle errors explicitly.

## Docker

The image uses a two-stage build: `rust:1-slim-trixie` compiles the release binary, and `debian:trixie-slim` runs it with `supervisor` and `cron`.

The runtime image includes the `sqlite3` CLI so the database inside the container can be inspected; the schema file itself is not shipped because it is compiled into the binary.

`supervisord` is PID 1. It supervises `cron -f` as root and the bot as the unprivileged `u502` user.

Images are published by CI, not built in development containers. The development container has no Docker daemon, so `docker build` and `docker run` fail here. On a host with Docker:

```bash
docker build -t u502_bot .
docker run --rm u502_bot
```

`compose.yaml` defines a single `bot` service. It builds from this checkout when given `--build`, and otherwise pulls the published image. `RUST_LOG` defaults to `info` and can be overridden through the environment.

Compose reads the token from `.secrets/telegram_api_token` and the Sentry DSN from `.secrets/sentry_dsn`, so `docker compose up` fails immediately until both files exist; see Secrets.

```bash
docker compose up --build      # build locally and run in the foreground
docker compose pull            # fetch the published image instead
docker compose up -d           # start detached
```

## Architecture decisions

Architecture decisions live in `docs/adr/`, one Markdown file per decision, named `NNNN-slug.md`.

- ADR 0001 records the decision to use Rust.
- ADR 0002 records the decision to read secrets from the environment or from a file.
- ADR 0003 records the decision to receive updates by long polling.
- ADR 0004 records the decision to report errors and metrics to Sentry.
- ADR 0005 records the decision to store messages in a local SQLite database.
- ADR 0006 records the decision to store attachment metadata for later processing.

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

The Sentry DSN is read the same way, through `get_secret`:

- `SENTRY_DSN`: the DSN itself, convenient for local runs.
- `SENTRY_DSN_FILE`: the path of a file holding the DSN.
- Compose: sets `SENTRY_DSN_FILE=/run/secrets/sentry_dsn` and mounts `.secrets/sentry_dsn` there, so the DSN never enters the container environment or `docker inspect`.

`SENTRY_DSN_FILE` wins when both variables are set. Sentry is optional: a missing, empty, or invalid DSN logs one warning and the bot runs without it. Create the file before running Compose, or leave it empty to opt out:

```bash
mkdir -p .secrets
printf '%s' "$SENTRY_DSN" > .secrets/sentry_dsn
chmod 600 .secrets/sentry_dsn
: > .secrets/sentry_dsn      # opt out: an empty file disables Sentry
```

## Storage

Messages live in a local SQLite database. The schema is embedded at build time with `include_str!` from `dev/schema.sql` and applied on every startup, so there is no schema path or environment variable to configure. The database is opened asynchronously at startup and accessed through the message repository in `src/domains/message.rs` and the attachment repository in `src/domains/attachment.rs`. The database path comes from `DATABASE_PATH`, which is ordinary configuration read with `std::env`, not a secret and therefore not through `get_secret`; it is required, so a missing value fails startup, and Compose sets it to `/opt/data/database.sqlite`. The command stores every delivered message in `messages` and skips a redelivery by its `(chat_id, message_id)` uniqueness. Every incoming audio or video message also records one row in `attachments`, keyed by the same `(chat_id, message_id)` pair, holding the Telegram `file_id`, the MIME type, the duration, the size, and a `transcribed` flag. Nothing is downloaded yet: the row is what a later worker needs to fetch and transcribe the file, and a partial index serves the future pending scan.

## Observability

Sentry receives every `ERROR`-level `tracing` event and every panic; `RUST_LOG` controls console output only. The command layer records the application counter `telegram.messages.received` once per incoming Telegram message and `telegram.messages.stored` once per successful storage statement, so the gap between them is the number of messages that failed to persist. It records `telegram.attachments.audio` and `telegram.attachments.video` once per received attachment of that family, and `telegram.attachments.stored` once per successful attachment statement, so the gap between them is the number of attachments that failed to persist.

Message text, chat ids, file ids, MIME types, and usernames are never sent: `send_default_pii` stays off and no metric carries attributes. Without a DSN the integration is disabled and the bot logs a single warning; see Secrets.

## Out of scope

- Project level skills live in `.agents/skills`.

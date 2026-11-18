# u502_bot

## Project overview

u502_bot is a Rust Telegram bot for a local car community.

`u502 start` connects to Telegram with long polling through `teloxide`, logs every incoming message, and stores it in SQLite. Three long-running services run beside the connection: one ingests messages, one transcribes audio and video from white-listed chats through OpenRouter and queues the text, and one delivers the queued replies. The token comes from `TELEGRAM_API_TOKEN`, or from the file named by `TELEGRAM_API_TOKEN_FILE` when that is set.

## Prerequisites

- Rust stable (edition 2024): install through rustup; `rust-toolchain.toml` pins the stable channel with the `rustfmt` and `clippy` components.
- GNU Make: runs the canonical commands below.
- A Telegram bot token from BotFather: required to run `cargo run -- start`; not needed for any `make` target.
- An OpenRouter API key: required to run `cargo run -- start`, for the same reason. The numeric ids of the chats to transcribe are optional and can be collected from the logs after the bot starts.
- universal-ctags (optional): required only for `make tags`.
- Node and `npx` (optional): required only for `make format-docs`.
- ffmpeg: required to transcribe video; installed in the runtime image, and `FFMPEG_PATH` can point at a different binary. A non-UTF-8 value logs one warning and falls back to the `ffmpeg` on `PATH`. Audio is transcribed without it.
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
TELEGRAM_API_TOKEN=... OPENROUTER_API_KEY=... TRANSCRIBE_CHATS=... DATABASE_PATH=... cargo run -- start
```

Running `make` without a target executes `check`.

## Layout

- `docs/features/<feature>/README.md`: user-facing documentation per feature; `README.md` indexes them and `docs/Install.md` covers running the bot.
- `src/main.rs`: entry point; calls `cli::run()`.
- `src/cli/mod.rs`: defines the clap `Command` enum, parses arguments, and routes each variant.
- `src/cli/<command>.rs`: one module per subcommand; `src/cli/start.rs` composes the clients, the repositories, and the services.
- `src/domains.rs`: declares the domain modules.
- `src/domains/attachment.rs`: the attachment model and the repository that owns the `attachments` SQL, including the pending scan and the processed flag.
- `src/domains/message.rs`: the message model and the repository that owns the `messages` SQL.
- `src/domains/outbox.rs`: the outbox model and the repository that owns the `outbox` SQL.
- `src/infra.rs`: declares the infra modules.
- `src/infra/config.rs`: `VariableName`, the enum naming every environment variable, plus `get_optional` and `require_secret`; one resolver gives every variable the same form: `<NAME>_FILE` first, then `<NAME>`, the value trimmed, blank meaning unset, UTF-8 only.
- `src/infra/database.rs`: `Database`, `database_path`, and `SCHEMA`; the async SQLite client, which opens a pooled connection and applies the embedded schema without knowing any table.
- `src/infra/ffmpeg.rs`: `Ffmpeg`; the client for the `ffmpeg` program, which extracts a video's first audio track to Ogg/Opus under a timeout, kills the child on drop, and records its own call telemetry. The binary path comes from `VariableName::FfmpegPath`; an unusable value, whether non-UTF-8 or an unreadable `FFMPEG_PATH_FILE`, logs one warning and falls back to the `ffmpeg` on `PATH`.
- `src/infra/openrouter.rs`: `OpenRouter`, `model_from_env`, `DEFAULT_TRANSCRIBE_MODEL`, and `AudioFormat`; the Speech-to-Text client for audio transcription, which records its own call telemetry.
- `src/infra/sentry.rs`: `VariableName::SentryDsn`, `init` (the `tracing` subscriber, the optional Sentry client, an error layer, and a crate-scoped structured-log layer), and the metric functions for messages, attachments, the outbox, OpenRouter, and ffmpeg.
- `src/infra/signal.rs`: `Signal` and `wait`; the listener for `SIGINT` and, on Unix, `SIGTERM`.
- `src/infra/telegram_bot.rs`: `Telegram` and `Messages`, the Telegram client; it receives, downloads, reacts, and sends, and knows nothing about storage.
- `src/services.rs`: declares the long-running services, the shutdown flag they share, and their shared clock.
- `src/services/shutdown.rs`: `Shutdown`, the sticky cooperative flag that stops every service on one request.
- `src/services/wake.rs`: `Wake`, the one-way pulse a service sends another when it creates work, which ends the receiver's idle pause early.
- `src/services/ingestor.rs`: the message loop; it stores every delivered message and its attachment metadata, drains what the dispatcher already queued, then stops polling.
- `src/services/transcriber.rs`: the transcription loop for audio and video and the `TRANSCRIBE_CHATS` whitelist; it processes an audio batch, then a video batch, and a video is downloaded and reduced with `ffmpeg` before it joins the audio flow.
- `src/services/outbox.rs`: the delivery loop; it sends queued replies and marks them sent.
- `src/shared.rs` / `src/shared/result.rs`: the crate-wide `Error` enum, with one variant per failure mode the bot can produce, and the `Result<T>` alias every fallible function returns; any layer may use them.
- `dev/schema.sql`: initial SQLite schema, embedded at build time and applied on every startup.

Add every new subcommand as a variant of the `Command` enum in `src/cli/mod.rs`, route it from `run()`, and implement it in `src/cli/<command>.rs`.

## Layering

- `src/shared/*`: cross-cutting types every layer may use; never a client, SQL, or a loop.
- `src/infra/*`: clients for exactly one external system; they never call each other, hold SQL, or contain domain logic. Configuration lookup and the observability sink are the exceptions: both are shared services rather than peer integrations.
- `src/domains/*`: models and repositories; each repository owns the SQL for its data.
- `src/services/*`: long-running tasks a command composes; they loop over time, own no SQL and no clients of their own, and hold the clients and repositories they were given.
- `src/cli/*`: commands compose the clients, the repositories, and the services; the Telegram client must not know that a database exists.

## Code style

- Target edition 2024 on the stable toolchain.
- Clippy must pass with `-D warnings`; `make lint` enforces this.
- Run `cargo fmt` (or `make format`) before committing.
- Avoid `unwrap()` in runtime paths; return or handle errors explicitly.

## Docker

The image uses a two-stage build: `rust:1-slim-trixie` compiles the release binary, and `debian:trixie-slim` runs it with `supervisor` and `cron`.

The runtime image includes the `sqlite3` CLI so the database inside the container can be inspected, and `ffmpeg` so video can be transcribed; the schema file itself is not shipped because it is compiled into the binary.

`supervisord` is PID 1. It supervises `cron -f` as root and the bot as the unprivileged `u502` user.

Images are published by CI, not built in development containers. The development container has no Docker daemon, so `docker build` and `docker run` fail here. On a host with Docker:

```bash
docker build -t u502_bot .
docker run --rm u502_bot
```

`compose.yaml` defines a single `bot` service. It builds from this checkout when given `--build`, and otherwise pulls the published image. `RUST_LOG` defaults to `info` and can be overridden through the environment.

Compose reads the token from `.secrets/telegram_api_token`, the Sentry DSN from `.secrets/sentry_dsn`, and the OpenRouter key from `.secrets/openrouter_api_key`, so `docker compose up` fails immediately until the token and the key exist; `TRANSCRIBE_CHATS` is optional; see Secrets.

```bash
docker compose up --build      # build locally and run in the foreground
docker compose pull            # fetch the published image instead
docker compose up -d           # start detached
```

## Shutdown

`SIGINT` (Ctrl-C) and `SIGTERM` (Compose, Docker, supervisord) both request one cooperative shutdown: the signal listener sets a sticky flag, and each service checks it at a work boundary. The Telegram dispatcher closes its channel, the ingestor stores what the dispatcher had already queued and then stops polling, and the transcriber and the outbox abandon idle pauses and in-flight downloads, model calls, and sends; a download or transcription that is abandoned is retried on the next start, and an interrupted send may duplicate one reply. Database writes already started are never cancelled. After the services end, `start` closes the pool and then returns, so the Sentry guard flushes; a graceful stop exits with success. `stop_grace_period: 20s` in `compose.yaml` and `stopwaitsecs=15` in `docker/supervisord.conf` bound the wait, and supervisord forwards its own `SIGTERM` to the bot, which never sees the Compose signal directly. The transcriber and the outbox also stop pausing when work arrives: the ingestor pulses the transcriber when it stores an attachment, and the transcriber pulses the outbox when it queues a reply.

## Architecture decisions

Architecture decisions live in `docs/adr/`, one Markdown file per decision, named `NNNN-slug.md`.

- ADR 0001 records the decision to use Rust.
- ADR 0002 records the decision to read every configuration variable from the environment or from a file.
- ADR 0003 records the decision to receive updates by long polling.
- ADR 0004 records the decision to report errors and metrics to Sentry.
- ADR 0005 records the decision to store messages in a local SQLite database.
- ADR 0006 records the decision to store attachment metadata for later processing.
- ADR 0007 records the decision to send application logs, including message content, to Sentry.
- ADR 0008 records the decision to queue outgoing replies in an outbox table.
- ADR 0009 records the decision to transcribe audio in a separate service.
- ADR 0010 records the decision to stop the bot gracefully on a signal.
- ADR 0011 records the decision to extract a video's audio track with ffmpeg before transcribing it.
- ADR 0012 records the decision to keep one crate-wide error enum and one `Result` alias.
- ADR 0013 records the decision to wake an idle service instead of waiting out its pause.

Run `make format-docs` after editing anything under `docs/`.

## Continuous integration

Pull requests run `make check` plus a formatting check with `cargo fmt --all -- --check`.

Pushes to `master` publish `ghcr.io/umonkey/u502_bot:latest` to GHCR. The publish workflow does not re-run tests; it relies on the required PR checks.

## Secrets

Every variable resolves the same way: `<NAME>_FILE` when it is set, otherwise `<NAME>`, with the value trimmed and a blank value counting as unset. Prefer the file form for the bot token, the OpenRouter key, and the Sentry DSN anyway, because a container's environment is rendered by `docker inspect` and inherited by child processes such as `ffmpeg`, while a mounted file is neither.

The bot token can be supplied in three ways:

- `TELEGRAM_API_TOKEN`: the token itself, convenient for local runs.
- `TELEGRAM_API_TOKEN_FILE`: the path of a file holding the token.
- Compose: sets `TELEGRAM_API_TOKEN_FILE=/run/secrets/telegram_api_token` and mounts `.secrets/telegram_api_token` there, so the token never enters the container environment or `docker inspect`.

`TELEGRAM_API_TOKEN_FILE` wins when both variables are set, as every `<NAME>_FILE` does, and the value is trimmed so a trailing newline is harmless. Create the file before running Compose:

```bash
mkdir -p .secrets
printf '%s' "$TELEGRAM_API_TOKEN" > .secrets/telegram_api_token
chmod 600 .secrets/telegram_api_token
```

`.secrets/` and `.env` are git-ignored and docker-ignored. Never commit them or copy them into an image.

Never bake tokens into the image or source. The token is read from the environment at runtime.

The Sentry DSN resolves through `get_optional(VariableName::SentryDsn)` like every other variable, from `SENTRY_DSN_FILE` first and `SENTRY_DSN` second:

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

The OpenRouter key resolves the same way:

- `OPENROUTER_API_KEY`: the key itself, convenient for local runs.
- `OPENROUTER_API_KEY_FILE`: the path of a file holding the key.
- Compose: sets `OPENROUTER_API_KEY_FILE=/run/secrets/openrouter_api_key` and mounts `.secrets/openrouter_api_key` there, so the key never enters the container environment or `docker inspect`.

`OPENROUTER_API_KEY_FILE` wins when both variables are set, as every `<NAME>_FILE` does, and the value is trimmed so a trailing newline is harmless. Unlike the DSN, the key is required: a missing, empty, or whitespace-only value fails startup, and an empty file is not a valid opt-out, because an empty secret counts as a missing one. Create the file before running Compose:

```bash
mkdir -p .secrets
printf '%s' "$OPENROUTER_API_KEY" > .secrets/openrouter_api_key
chmod 600 .secrets/openrouter_api_key
```

`TRANSCRIBE_CHATS` is ordinary configuration read through `get_optional(VariableName::TranscribeChats)`, with the same file form as everything else: a comma-separated list of numeric chat ids, optional, where empty means "transcribe nothing" and every audio and video attachment is skipped and marked processed. The skip warning carries the chat id, so an operator starts with an empty list, reads the ids from the logs, and adds them. A non-numeric entry still fails startup. `TRANSCRIBE_MODEL_NAME` is optional too and read the same way; when it is unset or empty the compiled-in default `google/gemini-3.5-transcribe` is used, and startup logs `Using model <id> to transcribe; override with TRANSCRIBE_MODEL_NAME` so the model in use is visible.

## Storage

Messages live in a local SQLite database. The schema is embedded at build time with `include_str!` from `dev/schema.sql` and applied on every startup, so there is no schema path or environment variable to configure. The database is opened asynchronously at startup and accessed through the message repository in `src/domains/message.rs`, the attachment repository in `src/domains/attachment.rs`, and the outbox repository in `src/domains/outbox.rs`. The database path comes from `DATABASE_PATH`, read through `require_secret(VariableName::DatabasePath)` with the same `<NAME>_FILE` form as every other variable, and it remains required, so a missing value fails startup; Compose sets it to `/opt/data/database.sqlite`. The ingestor stores every delivered message in `messages` and skips a redelivery by its `(chat_id, message_id)` uniqueness. Every incoming audio or video message also records one row in `attachments`, keyed by the same `(chat_id, message_id)` pair, holding the Telegram `file_id`, the MIME type, the duration, the size, and a `transcribed` flag that means processed: transcribed or deliberately skipped. The transcriber scans that flag, reading audio and video rows whose message is at most an hour old, so the age of the message bounds both cost and retries; a partial index serves the pending scan. Outgoing replies live in `outbox`, one row per Telegram message, holding the chat, the message id to answer when there is one, the queue time, the delivery time, and the text. `sent_at` stays NULL until Telegram accepts a row, and a partial index serves the pending scan.

## Observability

Sentry receives every `ERROR`-level `tracing` event and every panic as an issue, and every `INFO`-or-higher event from this crate as a structured log; events from dependencies become issues but not logs. `RUST_LOG` controls console output only. The ingestor records the application counter `telegram.messages.received` once per incoming Telegram message and `telegram.messages.stored` once per successful storage statement, so the gap between them is the number of messages that failed to persist. It records `telegram.attachments.audio` and `telegram.attachments.video` once per received attachment of that family, and `telegram.attachments.stored` once per successful attachment statement, so the gap between them is the number of attachments that failed to persist.

The transcriber records `telegram.attachments.transcribed` once per attachment it finishes with, whether transcribed, silent, or skipped, and `outbox.messages.added` once per queued chunk. The outbox service records `outbox.messages.sent` once per row Telegram accepted, so the gap between added and sent is the number of replies not yet delivered. The OpenRouter client records `openrouter.calls` and `openrouter.duration` for every call, successful or not, and `openrouter.tokens.input`, `openrouter.tokens.output`, and `openrouter.cost_micro_usd` from the usage OpenRouter reports; cost is in micro-USD. The ffmpeg client records `ffmpeg.calls` and `ffmpeg.duration` for every extraction, successful or timed out, so the video path is visible even when it never reaches the model. Startup logs `Using model <id> to transcribe; override with TRANSCRIBE_MODEL_NAME` once, so the model that spends the credits is visible both in the console and in Sentry's structured logs.

The reported logs include message text, transcripts, chat ids, sender names, file ids, MIME types, and file names. `send_default_pii` stays off, so the SDK adds no automatic enrichment, and no metric carries attributes. The DSN, the bot token, and the OpenRouter key are never logged. Without a DSN the integration is disabled and the bot logs a single warning; see Secrets.

## Out of scope

- Project level skills live in `.agents/skills`.

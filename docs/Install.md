# Installation

u502 Bot runs as a single Docker image. `supervisord` is PID 1: it supervises `cron -f` as root and the bot as the unprivileged `u502` user. The runtime image ships `ffmpeg` for video transcription and the `sqlite3` CLI for inspecting the database. No ports are published, because updates arrive by long polling, so there is no webhook, public URL, or ingress to set up.

## Prerequisites

- Docker with the Compose plugin
- A Telegram bot token from [BotFather](https://t.me/BotFather), created with `/newbot`
- An OpenRouter API key, required for transcription
- The Telegram user id of the bot's owner, required: it is the only account that may approve a chat for transcription
- A Sentry DSN, optional: without one the bot runs and reports nothing

A bot that joins a group also needs its privacy mode changed; see [communication](features/communication/README.md). The DSN is covered in [observability](features/observability/README.md).

## Secrets

Compose reads three files under `.secrets/`. It fails before the container starts until the token and the key exist, and until the Sentry file does too, so create all three:

```bash
mkdir -p .secrets
printf '%s' "$TELEGRAM_API_TOKEN" > .secrets/telegram_api_token
printf '%s' "$OPENROUTER_API_KEY" > .secrets/openrouter_api_key
printf '%s' "$SENTRY_DSN" > .secrets/sentry_dsn
chmod 600 .secrets/*
```

Leave `.secrets/sentry_dsn` empty to disable reporting. `.secrets/` is both git-ignored and docker-ignored: never commit it and never copy it into an image.

The owner id is not a secret, so it has no file under `.secrets/`: Compose passes `OWNER_USER_ID` through from the host environment or `.env`, and an empty value counts as missing, so `docker compose up` fails until it is set.

## Run

`docker compose up --build` builds from this checkout and runs in the foreground. `docker compose pull` fetches the published image instead, and `docker compose up -d` starts it detached. CI publishes `ghcr.io/umonkey/u502_bot:latest` on every push to `master`, so Compose pulls it unless `--build` is given.

Two repository shortcuts wrap the same commands: `make start` runs the bot in a one-off container and removes it when the bot exits, and `make shell` opens a shell in the image.

## What Compose sets

- the token, the Sentry DSN, and the OpenRouter key through `TELEGRAM_API_TOKEN_FILE`, `SENTRY_DSN_FILE`, and `OPENROUTER_API_KEY_FILE`, each mounted from `.secrets/` at `/run/secrets/`
- `DATABASE_PATH=/opt/data/database.sqlite`
- `RUST_LOG`, defaulting to `info`
- `OWNER_USER_ID`, required and passed through from the host environment or `.env`; an empty value fails startup
- `TRANSCRIBE_MODEL_NAME`, passed through from the host environment or `.env` and empty when unset, which means "use the built-in model"
- `RETENTION_DAYS`, passed through the same way and empty when unset, which keeps the built-in 30 days; `0` keeps every row instead of pruning

What each variable means, how its `_FILE` form resolves, and which ones are required is in [configuration](features/configuration/README.md). The owner id and the chat approvals are in [access](features/access/README.md).

## Finding your user id

Telegram reports a private chat's id as the user's own id, so the owner can read their id from the log line of any message they send the bot. An id bot such as [`@userinfobot`](https://t.me/userinfobot) reports it without a log line. The owner has to write to the bot once before it can send them a notice or an answer, because a bot cannot open a conversation.

## Data

The `u502_data` named volume holds `/opt/data`, so replacing or rebuilding the container keeps the database and its messages. `docker compose down -v` deletes the volume. See [storage](features/storage/README.md).

## Operations

```bash
docker compose logs -f bot                    # follow the logs
docker compose down                           # stop the bot
docker compose pull && docker compose up -d   # update to the published image
```

## Shutdown

Compose allows the container 20 seconds to stop, and supervisord waits 15 before it kills the bot. In that window the bot stops polling, stores the messages it had already received, closes the database, and flushes Sentry; see [communication](features/communication/README.md) for what happens to work in flight.

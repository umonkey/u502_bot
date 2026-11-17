# u502 Bot

This is a Telegram bot which listens to a chat group and transcribes audio and video messages.

## Installation

The bot runs in a Docker container. You normally run it via Docker Compose; see `compose.yaml` for examples.

## Configuration

Configuration options are read from the environment.

- `TELEGRAM_API_TOKEN`: the API token, see <https://t.me/BotFather> command `/newbot`.
- `SENTRY_DSN`: ingestion token for Sentry to report crashes, errors, logs, and metrics.
- `DATABASE_PATH`: path of the SQLite file that stores incoming messages and the metadata of their audio and video attachments, kept for later transcription. Required; Compose uses `/opt/data/database.sqlite`.

The reported logs include message text, chat and sender identifiers, and media metadata, so point `SENTRY_DSN` at a project whose members may see that content.

The initial schema is embedded in the binary at build time from `dev/schema.sql`, so there is nothing to mount or configure. Compose keeps the database in the `u502_data` named volume, so removing the container does not lose stored messages.

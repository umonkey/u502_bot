# u502 Bot

This is a Telegram bot which listens to a chat group and transcribes audio and video messages.

## Installation

The bot runs in a Docker container. You normally run it via Docker Compose; see `compose.yaml` for examples.

## Configuration

Configuration options are read from the environment.

- `TELEGRAM_API_TOKEN`: the API token, see <https://t.me/BotFather> command `/newbot`.
- `SENTRY_DSN`: ingestion token for Sentry to report crashes and metrics.

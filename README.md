# u502 Bot

A Rust Telegram bot for a local car community. It logs group messages into SQLite, transcribes audio and video from the chats its owner approves through OpenRouter, and replies with the transcript when there is one.

## Documentation

- [Installation](docs/Install.md): running the bot in Docker and Compose, the secrets it needs, and how to update or stop it
- [Configuration](docs/features/configuration/README.md): every variable, the `<NAME>_FILE` form, and which values are required
- [Communication](docs/features/communication/README.md): the Telegram connection, BotFather setup, ingestion, the outbox, replies, and shutdown
- [Transcription](docs/features/transcription/README.md): the audio and video path, ffmpeg, the model, and the retry window
- [Access](docs/features/access/README.md): the owner approves the chats that are transcribed
- [Storage](docs/features/storage/README.md): what SQLite holds, where it lives, and what is kept
- [Observability](docs/features/observability/README.md): logging, Sentry, metrics, and what the reported logs contain

Architecture decisions are recorded in [`docs/adr/`](docs/adr/).

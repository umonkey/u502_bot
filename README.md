# u502 Bot

This is a Telegram bot which listens to a chat group, stores incoming messages, and transcribes audio and video messages from white-listed chats, replying to each one with its transcript.

## Installation

The bot runs in a Docker container. You normally run it via Docker Compose; see `compose.yaml` for examples.

## Configuration

Configuration options are read from the environment.

- `TELEGRAM_API_TOKEN`: the API token, see <https://t.me/BotFather> command `/newbot`.
- `SENTRY_DSN`: ingestion token for Sentry to report crashes, errors, logs, and metrics. Optional.
- `OPENROUTER_API_KEY`: key for OpenRouter, which transcribes audio and video. Required.
- `FFMPEG_PATH`: path of the `ffmpeg` binary used to extract the audio track from video. Optional; the default `ffmpeg` is resolved through `PATH`.
- `TRANSCRIBE_MODEL_NAME`: id of the OpenRouter Speech-to-Text model used to transcribe. Optional; the default is `google/gemini-3.5-transcribe`.
- `TRANSCRIBE_CHATS`: comma-separated numeric ids of the chats whose audio and video are transcribed. Optional; empty transcribes nothing.
- `DATABASE_PATH`: path of the SQLite file that stores incoming messages, the metadata of their audio and video attachments, and the replies waiting to be sent. Required; Compose uses `/opt/data/database.sqlite`.

Every variable in that list can also be supplied through the file named by its `_FILE` variant, such as `OPENROUTER_API_KEY_FILE` or `DATABASE_PATH_FILE`, which wins when both are set. The file is read and decoded as UTF-8, the value is trimmed of surrounding whitespace, and a value that is blank after trimming counts as unset. `RUST_LOG` is the one exception: the logging filter reads it, not this list, so it is the only setting without a `_FILE` variant.

Prefer the file form for the bot token, the OpenRouter key, and the Sentry DSN: a container's environment is rendered by `docker inspect` and inherited by child processes such as `ffmpeg`, while a mounted file is neither.

The bot refuses to start when `OPENROUTER_API_KEY` is missing: an unconfigured client would fail every transcription with an authentication error while the process looked healthy. An empty `openrouter_api_key` file is not a valid opt-out, unlike an empty `sentry_dsn` file, because an empty value counts as a missing key. Sentry and `TRANSCRIBE_CHATS` stay optional.

`TRANSCRIBE_CHATS` is empty by default, which transcribes nothing. Start the bot, send a voice message to the chat you care about, and read the chat id from the warning the bot logs for the skipped attachment; then add it to `TRANSCRIBE_CHATS`. Audio and video received before the id is listed are skipped and not transcribed later.

At startup the bot logs the model it will use, for example `Using model google/gemini-3.5-transcribe to transcribe; override with TRANSCRIBE_MODEL_NAME`, so the model that spends the credits is visible in the logs. Set `TRANSCRIBE_MODEL_NAME` to change it; an empty value falls back to the default. It also probes `ffmpeg` once and logs one warning when the binary is missing, in which case audio is still transcribed and every video is skipped.

The reported logs include message text, chat and sender identifiers, media metadata, and transcripts, so point `SENTRY_DSN` at a project whose members may see that content. Audio is downloaded to a temporary file and sent to OpenRouter untouched, and the transcript is stored by Sentry and delivered back to the chat. A video is downloaded the same way, then its first audio track is extracted with `ffmpeg`, which the runtime image installs, into 16 kHz mono Ogg/Opus; that file then follows the audio path. A video above the Bot API download ceiling of 20 MB, or one that carries no audio track, is skipped and marked processed instead of being retried.

The bot declares Ogg on the wire: it is what a Telegram voice note already carries, and OpenRouter's Speech-to-Text API accepts `ogg` as an `input_audio.format`, so the video branch reuses a known-good format. Ogg is not universal, and OpenRouter does not report per-format support that could be checked at runtime, so a provider that rejects it answers with HTTP 400 and the attachment is retried until it ages out of the window. Switching `TRANSCRIBE_MODEL_NAME` to a model that accepts only mp3 or wav requires changing both the format the video is extracted to and the format the bot declares. The override must name a model served by the Speech-to-Text API, which is a separate catalogue from the chat models: `GET https://openrouter.ai/api/v1/models?output_modalities=transcription` lists them.

The initial schema is embedded in the binary at build time from `dev/schema.sql`, so there is nothing to mount or configure. Compose keeps the database in the `u502_data` named volume, so removing the container does not lose stored messages.

## Running with Compose

Compose needs `.secrets/telegram_api_token`, `.secrets/sentry_dsn` (which may be empty to disable Sentry), and `.secrets/openrouter_api_key`. Without the token or the key `docker compose up` fails before the container starts. `TRANSCRIBE_CHATS` is optional and can be added later, once the ids are known.

```bash
mkdir -p .secrets
printf '%s' "$OPENROUTER_API_KEY" > .secrets/openrouter_api_key
chmod 600 .secrets/openrouter_api_key
TRANSCRIBE_CHATS=-1001234567890 docker compose up
```

`TRANSCRIBE_MODEL_NAME` is passed through when it is set and left empty otherwise, which means the built-in default.

The bot stops cleanly on Ctrl-C and on the signal Compose sends when it stops or restarts the container: it stops polling, stores the messages it had already received, closes the database, and flushes monitoring before exiting.

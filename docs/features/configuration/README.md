# Configuration

Every setting comes from the environment; there is no config-file format and no command-line flag to set one.

## Sources

Each variable resolves the same way: `<NAME>_FILE` is consulted first, and its contents win over a direct `<NAME>`. The file is decoded as UTF-8, the resolved value is trimmed of surrounding whitespace, and a value that is blank after trimming counts as unset. So `OPENROUTER_API_KEY_FILE=/run/secrets/openrouter_api_key` and `OPENROUTER_API_KEY=...` both work, and the file wins when both are set.

`RUST_LOG` is the one exception: the logging filter reads it directly, it has no `_FILE` form, and it affects console output only. See [observability](../observability/README.md).

## Variables

| Variable                | Required | Default                          | Purpose                                                                                     |
| ----------------------- | -------- | -------------------------------- | ------------------------------------------------------------------------------------------- |
| `TELEGRAM_API_TOKEN`    | yes      | none                             | BotFather token; see [communication](../communication/README.md)                            |
| `OPENROUTER_API_KEY`    | yes      | none                             | OpenRouter key for Speech-to-Text; see [transcription](../transcription/README.md)          |
| `DATABASE_PATH`         | yes      | none                             | SQLite file; Compose uses `/opt/data/database.sqlite`; see [storage](../storage/README.md)  |
| `SENTRY_DSN`            | no       | unset                            | error and log reporting; blank disables it; see [observability](../observability/README.md) |
| `TRANSCRIBE_CHATS`      | no       | empty, which transcribes nothing | comma-separated chat ids; see [access](../access/README.md)                                 |
| `TRANSCRIBE_MODEL_NAME` | no       | `google/gemini-3.5-transcribe`   | Speech-to-Text model id; see [transcription](../transcription/README.md)                    |
| `FFMPEG_PATH`           | no       | the `ffmpeg` on `PATH`           | ffmpeg binary used for video; see [transcription](../transcription/README.md)               |
| `RUST_LOG`              | no       | `info`                           | console filter only, with no `_FILE` form; see [observability](../observability/README.md)  |

## Required versus optional

The bot refuses to start when a required value is missing. The OpenRouter key is required because an unconfigured client would fail every transcription with an authentication error while the process looked healthy. An empty `openrouter_api_key` file is not an opt-out, unlike an empty `sentry_dsn` file, because an empty value counts as a missing key.

## Secrets

Prefer the file form for the bot token, the OpenRouter key, and the Sentry DSN: a container's environment is rendered by `docker inspect` and inherited by child processes such as `ffmpeg`, while a mounted file is neither. The concrete `.secrets/` layout and the Compose mounts are in [installation](../../Install.md).

## Startup failures

A missing required value, an unreadable or non-UTF-8 configuration file, and a non-numeric entry in `TRANSCRIBE_CHATS` each fail startup with an error naming the variable or the path, so the supervisor restarts the process and the log shows why. An unusable `FFMPEG_PATH` is the exception: a non-UTF-8 or unreadable value logs one warning and falls back to the `ffmpeg` on `PATH`, which may be a different binary than intended.

## Further reading

- [ADR 0002: read configuration from the environment or a file](../../adr/0002-secret-configuration.md)

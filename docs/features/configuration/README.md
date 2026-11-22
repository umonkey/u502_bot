# Configuration

Every setting comes from the environment; there is no config-file format and no command-line flag to set one.

## Sources

Each variable resolves the same way: `<NAME>_FILE` is consulted first, and its contents win over a direct `<NAME>`. The file is decoded as UTF-8, the resolved value is trimmed of surrounding whitespace, and a value that is blank after trimming counts as unset. So `OPENROUTER_API_KEY_FILE=/run/secrets/openrouter_api_key` and `OPENROUTER_API_KEY=...` both work, and the file wins when both are set.

`RUST_LOG` and `TZ` are the exceptions, and neither has a `_FILE` form: the logging filter reads `RUST_LOG` directly and it affects console output only, while the date library reads `TZ` for `export`. See [observability](../observability/README.md) and [export](../export/README.md).

## Variables

| Variable                | Required | Default                          | Purpose                                                                                                                            |
| ----------------------- | -------- | -------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| `TELEGRAM_API_TOKEN`    | yes      | none                             | BotFather token; see [communication](../communication/README.md)                                                                   |
| `OPENROUTER_API_KEY`    | yes      | none                             | OpenRouter key for Speech-to-Text; see [transcription](../transcription/README.md)                                                 |
| `DATABASE_PATH`         | yes      | none                             | SQLite file; Compose uses `/opt/data/database.sqlite`; see [storage](../storage/README.md)                                         |
| `SENTRY_DSN`            | no       | unset                            | error and log reporting; blank disables it; see [observability](../observability/README.md)                                        |
| `TRANSCRIBE_CHATS`      | no       | empty, which transcribes nothing | comma-separated chat ids; see [access](../access/README.md)                                                                        |
| `TRANSCRIBE_MODEL_NAME` | no       | `google/gemini-3.5-transcribe`   | Speech-to-Text model id; see [transcription](../transcription/README.md)                                                           |
| `RETENTION_DAYS`        | no       | `30`                             | days of messages, attachments, and delivered outbox rows kept; `0` keeps every row; see [storage](../storage/README.md)            |
| `RUST_LOG`              | no       | `info`                           | console filter only, with no `_FILE` form; see [observability](../observability/README.md)                                         |
| `TZ`                    | no       | system zone                      | local day boundary and clock times for `export`; read by the date library, with no `_FILE` form; see [export](../export/README.md) |

`export` reads `DATABASE_PATH` and `TZ` only. It does not require the bot token, the OpenRouter key, the Sentry DSN, or any transcription setting, so it can run against a copy of the database on another machine. See [export](../export/README.md).

## Failure policy

The bot resolves every variable once at startup and applies one policy to all of them:

- A required value that is absent or blank fails startup.
- An optional value that is absent or blank uses its default: an empty chat list transcribes nothing, an unset model uses the compiled-in one, and an unset retention keeps 30 days.
- A value that is present but unusable — an unreadable or non-UTF-8 file, a non-Unicode variable, a non-numeric chat id, a `RETENTION_DAYS` that is not a non-negative whole number — fails startup, whether the variable is required or optional.
- `SENTRY_DSN` is the one exception: a missing, blank, unreadable, or invalid DSN only logs one warning and disables reporting, so observability never keeps the bot from running; see [observability](../observability/README.md).

The bot refuses to start when a required value is missing. The OpenRouter key is required because an unconfigured client would fail every transcription with an authentication error while the process looked healthy. An empty `openrouter_api_key` file is not an opt-out, unlike an empty `sentry_dsn` file, because an empty value counts as a missing key.

## Secrets

Prefer the file form for the bot token, the OpenRouter key, and the Sentry DSN: a container's environment is rendered by `docker inspect` and inherited by child processes such as `ffmpeg`, while a mounted file is neither. The concrete `.secrets/` layout and the Compose mounts are in [installation](../../Install.md).

## Startup failures

A missing required value, an unreadable or non-UTF-8 configuration file, a non-numeric entry in `TRANSCRIBE_CHATS`, and a `RETENTION_DAYS` that is not a non-negative whole number each fail startup with an error naming the variable or the path, so the supervisor restarts the process and the log shows why. The file case covers optional variables too: a broken `TRANSCRIBE_MODEL_NAME_FILE` fails startup rather than falling back to the default, and only an absent or blank optional value takes its default. A configuration failure that happens before logging is set up is still reported to Sentry when the DSN is usable.

## Further reading

- [ADR 0002: read configuration from the environment or a file](../../adr/0002-secret-configuration.md)
- [ADR 0014: resolve configuration into a typed Settings](../../adr/0014-typed-settings.md)

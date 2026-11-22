# Export

`u502 export` is a one-shot command that reads the SQLite database and writes one Markdown log per chat for one day. It sends nothing to Telegram and does not touch the running services: the files are for reading, searching, or handing a day of conversation to someone.

## Usage

```bash
DATABASE_PATH=/opt/data/database.sqlite u502 export [YYYYMMDD] [CHAT_ID] --out DIR
```

Export reads only `DATABASE_PATH`. The Telegram token, the OpenRouter key, the Sentry DSN, and every transcription setting are not needed, so the command works on a machine that has the database file alone.

## Defaults

- `YYYYMMDD`: yesterday in the system local time zone.
- `CHAT_ID`: every chat with at least one message that day, in ascending chat-id order. A negative id selects a group, exactly as Telegram assigns it.
- `--out DIR`: the current working directory.

## Time zone

The day is the operator's local day, not a UTC day: the command converts local midnight and the next local midnight to the instants the database stores, and it renders clock times in the same zone. The zone comes from `TZ` when it is set, otherwise from the system configuration such as `/etc/localtime`.

An unknown or unreadable zone fails the export instead of falling back to UTC, because a silent fallback would put evening messages in the wrong file. `TZ` is read by the date library, not by the bot's configuration code, and it has no `_FILE` form. The runtime image includes the zone database, so names such as `Europe/Moscow` resolve; see [installation](../../Install.md).

## Format

```text
# Chat Log: 2026-11-22

[08:14] (#10421) Alexey: Guys, white smoke on cold start.
    Pulled the codes, nothing yet.
[08:17] (#10422) Dmitry: Check your HPFP or leaking injector.
```

- The header is the ISO date.
- Messages are ordered by arrival time, then by message id.
- `(#id)` is the Telegram message id.
- The sender is the stored display name, or `Unknown` when Telegram supplied none.
- Every line after the first is indented by exactly four spaces. Interior blank lines become four spaces, and trailing newlines are removed, so a message that ends in a newline leaves no stray indented line.
- Rows without text are skipped: uncaptioned photos, stickers, and service events produce no line. Captions are stored as text, so a captioned photo is exported normally.

## Files and chat ids

Each chat becomes `{YYYYMMDD}-{chat_id}.md`, for example `20261122-42.md`. A negative chat id gives a double hyphen, such as `20261122--100123.md`. An existing file is overwritten, so re-running an export for the same day and chat is idempotent.

The paths are printed to standard output in export order. A chat with no renderable text produces no file.

## Notices and exit codes

- A day with no messages, or an explicitly requested chat with none, prints a notice to standard error and exits successfully.
- A missing database file is an error: export refuses to create an empty database, so a typo in `DATABASE_PATH` cannot look like a quiet day.
- An invalid date, such as `2026-11-22` or `20260230`, is rejected by the argument parser before the command runs, with exit code 2.
- Any other failure prints `error: ...` to standard error and exits with a failure code.

## Running in Docker

```bash
docker compose run --rm -e TZ=Europe/Moscow bot u502 export --out /opt/data/export
```

The container's working directory is `/`, which the unprivileged bot user cannot write, so pass an output directory that is writable. `/opt/data` is the `u502_data` volume and survives the container, while another host directory has to be mounted. The bot must be stopped for nothing: export only reads, and the database is opened with write-ahead logging, so it sees the last committed snapshot while the bot ingests.

## Positional caveat

The date comes first and the chat second, so a single chat still needs a date: `u502 export 20261121 -100123`. A future optional flag could remove the placeholder; the current shape keeps both positionals predictable.

## Further reading

- [ADR 0017: export one local day of a chat to Markdown](../../adr/0017-export-chat-day.md)
- [Storage](../storage/README.md): what the database holds and for how long
- [Configuration](../configuration/README.md): the variables export reads

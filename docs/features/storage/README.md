# Storage

One local SQLite database holds everything the bot keeps. There is no external database and no server to run beside it.

## What is stored

- messages: every message Telegram delivers, with its text or caption
- attachments: one row per audio or video message, holding the Telegram `file_id`, the MIME type, the duration, the size, and a `transcribed` flag
- outbox: one row per queued reply, holding the chat, the message it answers when there is one, the queue time, the delivery time, and the text

## Duplicates

Incoming messages are unique by `(chat_id, message_id)`, so a redelivery is skipped instead of stored twice. Attachments are keyed by the same pair. Only a conflict on that pair is skipped: any other constraint failure fails the delivery and rolls the transaction back, so it is reported as an error rather than as a repeat.

### Atomicity

A message and its attachment are written in one transaction, so they commit together: when the database rejects the attachment, the message is rolled back and neither row is kept. A rejected write is the only way a delivered message goes missing, because Telegram acknowledges an update when it is fetched and does not deliver it again. Rows written before this behavior existed can still hold a message without its attachment; a redelivery of that message writes the missing row.

## Attachments

The `transcribed` flag means processed: transcribed, silent, or deliberately skipped. It is not "has a transcript". The pending scan reads rows whose message is at most an hour old, and a partial index serves it, so the age of the message bounds both cost and retries. See [transcription](../transcription/README.md).

## Outbox

`sent_at` stays NULL until Telegram accepts a row, so a restart resends whatever was queued but not delivered. Delivery is at-least-once; see [communication](../communication/README.md).

## Schema

The schema is embedded from `dev/schema.sql` at build time with `include_str!` and applied on every startup, so there is nothing to mount or configure and no migration step to run. A schema change is an idempotent statement in that file, and the file itself is not shipped in the image.

## Location

`DATABASE_PATH` is required; Compose sets it to `/opt/data/database.sqlite`, on the `u502_data` named volume. Removing or rebuilding the container keeps the data, while `docker compose down -v` deletes the volume and the database with it. The runtime image includes the `sqlite3` CLI, so the file can be inspected from a shell in the container. See [installation](../../Install.md) for the volume and the shell.

## Retention

Rows are not pruned automatically: the database grows with the traffic the bot sees, and clearing old messages is an operator's decision. Transcription has its own bound, the one-hour window, so an old pending attachment is never transcribed later even though its row remains.

## Further reading

- [ADR 0005: keep received messages in a local database](../../adr/0005-local-sqlite-storage.md)
- [ADR 0006: record attachment metadata for later processing](../../adr/0006-attachment-storage.md)
- [ADR 0008: queue outgoing replies in an outbox](../../adr/0008-outbox-queue.md)

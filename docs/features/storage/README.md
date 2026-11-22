# Storage

One local SQLite database holds everything the bot keeps. There is no external database and no server to run beside it.

## What is stored

- messages: every message Telegram delivers, with its text or caption
- attachments: one row per audio or video message, holding the Telegram `file_id`, the MIME type, the duration, the size, and a `transcribed` flag
- outbox: one row per queued reply, holding the chat, the message it answers when there is one, the queue time, the delivery time, and the text
- chats: one row per chat the bot has received a message from, holding its status and when it was first and last seen

## Duplicates

Incoming messages are unique by `(chat_id, message_id)`, so a redelivery is skipped instead of stored twice. Attachments are keyed by the same pair. Only a conflict on that pair is skipped: any other constraint failure fails the delivery and rolls the transaction back, so it is reported as an error rather than as a repeat.

### Atomicity

A message and its attachment are written in one transaction, so they commit together: when the database rejects the attachment, the message is rolled back and neither row is kept. A rejected write is the only way a delivered message goes missing, because Telegram acknowledges an update when it is fetched and does not deliver it again. Rows written before this behavior existed can still hold a message without its attachment; a redelivery of that message writes the missing row.

## Attachments

The `transcribed` flag means processed: transcribed, silent, or deliberately skipped. It is not "has a transcript". The pending scan reads rows whose message is at most an hour old, and a partial index serves it, so the age of the message bounds both cost and retries. See [transcription](../transcription/README.md).

## Outbox

`sent_at` stays NULL until Telegram accepts a row, so a restart resends whatever was queued but not delivered. Delivery is at-least-once; see [communication](../communication/README.md).

## Chat registry

`chats` holds one row per chat the bot has received a message from: the chat id, the kind Telegram reported, the title, a status of `pending`, `approved`, or `blocked`, and the first and last observation times. A new chat starts `pending`, and later messages refresh only the kind, the title, and the last-seen time, so neither an approval nor a block is overwritten by activity. A status the bot does not recognise reads back as pending, so a hand-edited row fails closed. See [access](../access/README.md).

Retention leaves this table alone. It holds one row per chat ever seen, so it stays tiny, and the owner's decisions are operator state that must not age out.

## Concurrency

The file is opened with WAL journaling, so a read sees the last committed snapshot while another connection is writing. The pool holds up to four connections, so the transcriber's pending scan and the outbox's pending scan do not wait for the ingestor's commit. SQLite still admits one writer at a time: a second writer waits on the five-second busy timeout instead of failing, so a transaction writes before it reads rather than holding a read snapshot open across its first write. In-memory test databases keep one connection, because each `:memory:` connection is a separate database.

## Schema

The schema is embedded from `dev/schema.sql` at build time with `include_str!` and applied on every startup, so there is nothing to mount or configure and no migration step to run. A schema change is an idempotent statement in that file, and the file itself is not shipped in the image.

## Location

`DATABASE_PATH` is required; Compose sets it to `/opt/data/database.sqlite`, on the `u502_data` named volume. Removing or rebuilding the container keeps the data, while `docker compose down -v` deletes the volume and the database with it. The runtime image includes the `sqlite3` CLI, so the file can be inspected from a shell in the container. See [installation](../../Install.md) for the volume and the shell.

## Retention

A fourth long-running service prunes the store: once at startup, then once an hour. One pass deletes delivered outbox rows whose `sent_at` is older than the retention, then attachments whose message is missing or older than the retention, then messages older than the retention. The `transcribed` flag is not consulted, because past the transcriber's one-hour window an unprocessed attachment can never be attempted, so it ages out with its message. Unsent outbox rows and rows inside the retention are never touched.

`RETENTION_DAYS` sets the retention in whole days; it is optional, defaults to 30, and `0` keeps every row. Startup logs the effective value and how to disable pruning. See [configuration](../configuration/README.md).

The work is bounded: one statement deletes at most 500 rows and one table runs at most 20 statements per pass, so a wake deletes at most 10 000 rows per table and an existing backlog drains over hours instead of holding the write lock. A pass is idempotent, so a second one over the same data deletes nothing.

Two indexes serve the age scans: `messages_received_at` and the partial `outbox_sent`. Both are created by the embedded schema on startup, so an existing file needs no manual upgrade.

Deletion bounds growth but does not shrink the file: SQLite keeps the freed pages. Reclaiming disk space is an operator's `VACUUM`, run with the bot stopped.

## Further reading

- [ADR 0005: keep received messages in a local database](../../adr/0005-local-sqlite-storage.md)
- [ADR 0006: record attachment metadata for later processing](../../adr/0006-attachment-storage.md)
- [ADR 0008: queue outgoing replies in an outbox](../../adr/0008-outbox-queue.md)
- [ADR 0016: bound the stored rows with a retention and a pruning service](../../adr/0016-retention-and-pruning.md)

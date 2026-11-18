# ADR 0016: Bound the stored rows with a retention and a pruning service

- Status: accepted
- Date: 2026-11-18

## Context

Every table in the store only grows. The ingestor writes every delivered message, the transcriber
marks attachments and queues replies, and the outbox marks rows sent, but nothing ever deletes: a
delivered reply, a finished attachment, and a message from last year are kept forever. The host disk
is small, and the bot runs unattended, so an unbounded file eventually fills it and the process fails
on a write rather than on a decision.

Almost all of that data is already dead. Every service reads only recent rows: the transcriber scans
attachments whose message is at most an hour old, and the outbox delivers only rows with `sent_at`
NULL, which are usually seconds old. A delivered outbox row, a message past the transcriber's window,
and the attachment next to it can never be read again by any code path. Only an operator inspecting
the database is a reader, and that is not work the bot owes.

## Decision

Add a fourth long-running service, `Pruner`, beside the ingestor, the transcriber, and the outbox. It
is a loop in the `start` process, over the same connection pool, not a separate process: no second
pool, no second writer against the same file, and no scheduling for an operator to install.
`docker/cron/u502` stays a placeholder. A one-pass `u502 prune` subcommand for cron or for reclaiming
space on demand is a possible follow-up; it is not part of this decision.

The service prunes once immediately at startup, so an existing backlog starts draining on the first
start after the upgrade, and then once an hour on a constant interval. The interval is a constant
rather than a variable, because rows age out in days and no other service produces work for the
pruner.

Use one retention, in whole days, from `RETENTION_DAYS` or `RETENTION_DAYS_FILE` through the same
resolver as every other variable (ADR 0002, ADR 0014). It is optional, defaults to 30 days, and `0`
disables pruning entirely: the resolved value is `None`, so no caller compares a number against zero.
A value that is present but not a non-negative whole number fails startup, like every other unusable
value.

Apply the retention to three tables, each keyed by the timestamp it already has, so no column is
added and no existing file needs a manual upgrade (ADR 0005):

- delivered outbox rows, `sent_at IS NOT NULL AND sent_at < cutoff`;
- attachments whose message is missing or whose message's `received_at < cutoff`, joined on
  `(chat_id, message_id)` exactly as the pending scan already joins them;
- messages, `received_at < cutoff`.

The attachment's `transcribed` flag is deliberately not consulted. Past the transcriber's one-hour
window an unprocessed row can never be attempted, so the flag stops separating live work from dead
weight; keeping those rows would leak them forever, and once their message is pruned they could not
even be joined again. The join's left side also matches a legacy row written before a message and its
attachment committed together, which has no timestamp to age and which no pending scan can reach.

Delete the tables in the order outbox, attachments, messages. Delivered rows go first because they
are the bulk and nothing joins them; attachments go before messages because the age predicate reads
`messages`, so every attachment is aged while its message still exists. Each table is an independent
statement, so no transaction spans the three and a failure in one does not roll back the others.

Bound the work. One statement deletes at most 500 rows and one table runs at most 20 statements per
pass, so one hourly wake deletes at most 10 000 rows per table and a large existing file drains over
hours instead of holding SQLite's single write lock for one long pass. Every statement is an implicit
transaction, deleting by primary key with `ORDER BY id LIMIT`, so a pass is idempotent: a second pass
over the same data finds nothing.

Record `telegram.messages.pruned`, `telegram.attachments.pruned`, and `outbox.messages.pruned`, one
counter increment per deleted row, from the deleting statement (ADR 0004). A statement that deletes
nothing records nothing, so the counters do not fill with zeros. Success is otherwise silent.

Add two indexes, created by the embedded schema on startup like every other additive statement:
`messages_received_at` on `messages (received_at)`, which serves the message prune and the attachment
join, and the partial `outbox_sent` on `outbox (sent_at) WHERE sent_at IS NOT NULL`, which serves the
outbox prune. Without them each batch scans the whole table.

Document that deletion bounds growth but does not shrink the file: SQLite keeps freed pages in the
file, and reclaiming disk space is `VACUUM`, an operator's operation run with the bot stopped. An
automated `VACUUM` is not part of this decision.

## Consequences

- the file stops growing without bound: rows no service can read again disappear after the retention
- unsent outbox rows and rows inside the retention are never touched, so retention cannot lose a
  queued reply
- the database file on disk does not shrink by itself; only an operator's `VACUUM` returns the freed
  pages to the filesystem
- a pending outbox reply older than the retention loses its `transcription.reply.latency` sample,
  because the source message it joins is gone; accepted, since the row is days old and the
  alternative is keeping every message forever
- the retention applies to the whole store at once: an operator who wants to keep messages longer
  than delivered replies cannot say so
- the hourly interval is not configurable; a different cadence is a code change
- the counters show how much each pass deleted, and a busy pass is visible without a log line per row
- one new optional variable, two additive indexes, and no schema column change or manual upgrade
- each statement holds the write lock for at most 500 deletes, so the ingestor waits behind a short
  statement rather than a long pass
- the default deletes data an operator did not ask to lose; the startup log states the effective
  retention and how to disable it, and `RETENTION_DAYS=0` restores the unbounded behaviour

## Alternatives considered

- Prune only from cron, through a one-pass subcommand: no new service, but it needs a second process,
  a second pool, and a second writer against the same file, and the image's cron entry is still a
  placeholder nothing starts.
- Prune on write, deleting old rows as part of the ingestor's or the outbox's statement: no new loop,
  but it puts retention in a path that must stay fast and predictable, and a quiet bot would never
  prune at all.
- Delete everything in one unbounded pass per wake: the simplest pass, but it holds the write lock for
  as long as the backlog is large, which is exactly when the bot must keep ingesting.
- Automate a `VACUUM` after a pass: returns the space without an operator, but it rewrites the whole
  file under an exclusive lock, which is the one operation that must not run while the bot serves
  traffic.
- A retention per table, or a retention per attachment state: finer control, but three more variables
  and a rule for every combination, for a difference no reader of the store can use.
- Default off, opt in by setting a value: safer against surprise deletion, but ships a job that fixes
  nothing for the operator who never reads the configuration page.

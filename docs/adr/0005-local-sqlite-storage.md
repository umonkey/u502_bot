# ADR 0005: Keep received messages in a local database

- Status: accepted
- Amended: 2026-11-18, to record the pool settings and the additive-only schema rule.
- Amended: 2026-11-18, to allow pooled readers under WAL.
- Date: 2026-11-17

## Context

Messages arrive continuously, but the work that follows them is slow, can fail on its own, and must
survive a restart. The running process cannot hold them until then, so anything not written down
durably is lost when the container is replaced. The host is small and shared, has no database service
to connect to, and stored content must stay on it.

## Decision

Keep messages in one database file on a persistent volume, so they outlive the container and a
replacement finds them.

Separate receiving from storing: the platform connection only delivers messages, and the command that
starts the bot stores them, so the platform client knows nothing about storage. Access the store
asynchronously, so a slow disk delays the next message instead of stalling the connection. Apply the
stored structure at startup, so a fresh volume needs no preparation and an existing one keeps
working. Open the file with WAL journaling, a five-second busy timeout, and foreign keys, over a pool
of up to four connections: WAL admits one writer and many readers, so a read sees the last committed
snapshot while another connection writes; embed the schema in the binary at build time. Recognise a
message delivered again and leave the stored copy alone. There is no migration mechanism yet: startup
only creates missing objects, so an additive statement ships with the application, while altering an
existing table needs a manual upgrade.

## Consequences

- receiving and later processing are decoupled, so slow or failing work never holds up messages
- messages survive restarts, and a redelivery is harmless
- a read on one connection no longer waits for a write on another, and it sees the last committed
  snapshot; writers still queue behind SQLite's single write lock, waiting up to the busy timeout
- a transaction must write first, because a deferred `BEGIN` takes the write lock on its first write,
  while a read followed by a write can instead fail with `SQLITE_BUSY_SNAPSHOT`, which the busy
  timeout does not retry
- a backup is a copy of one file, and there is no service to install, watch, or pay for
- old messages are never removed, so the file grows, and delivered outbox rows and processed
  attachments are kept too
- an edited message is not updated, and the first version is what remains stored
- a stored structure change needs a rebuild and a manual upgrade of existing files
- the arrangement fits one bot on one host, because a second instance would write the same file

## Alternatives considered

- An external database service: concurrent access and tooling, but a service to install, back up,
  watch, and secure, plus inbound access the host does not have.
- Plain files or an append-only log: no dependency, but no lookup by state, no protection against a
  concurrent writer, and no duplicate detection.
- An embedded key-value store: fast, but no way to select work by its shape, so processing state
  would need a hand-built index.
- Synchronous access that blocks receiving: the least machinery, but a slow disk would stop the
  connection and the backlog would pile up in memory.

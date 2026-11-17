# ADR 0005: Keep received messages in a local database

- Status: accepted
- Date: 2026-11-17

## Context

u502_bot receives Telegram messages by long polling and, before this decision, did little more than
log them. A later stage has to transcribe them and deliver the result, and that work cannot happen
while a message arrives: it is slow, it can fail on its own, and it must survive a restart. Nothing
in the running process can hold messages until then, so a message that is not written down somewhere
durable is lost when the container is replaced, and can never be transcribed.

The host is small and shared with this bot. There is no database service to install or operate, no
inbound access through which to reach one, and the project keeps its footprint small. Stored message
content must stay on the host.

## Decision

Keep messages in a single database file on a persistent volume, so they outlive the container and a
replacement finds the same messages.

Separate receiving from storing. The Telegram connection only delivers messages; the command that
starts the bot receives them and stores them. Neither side has to know how the other works, and the
Telegram client carries no storage code.

Store through a message model and a repository in the domain layer, so the clients stay free of
storage detail and the storage behaviour can be tested without Telegram.

Access the database asynchronously, so disk latency delays the next message instead of stalling the
connection, and incoming traffic is never stuck behind a slow write.

Treat the structure of the stored data as part of the application and apply it at startup, so a fresh
volume needs no manual preparation and an existing one keeps working. Recognise a message that
arrives again after a restart and leave the stored copy alone instead of writing a second one. There
is no migration mechanism yet: a change to that structure ships with the application.

## Consequences

Positive:

- receiving and later processing are decoupled, so a slow or failing transcription cannot hold up
  incoming messages
- messages survive restarts, and a redelivery after a restart is harmless
- a backup is a copy of one file
- a slow disk delays one message rather than stopping the connection, so receiving keeps up
- there is no service to install, upgrade, watch, or pay for

Negative:

- a change to the stored structure needs a rebuild and a redeploy, plus a manual upgrade of existing
  files until migrations exist
- nothing removes old messages, so the file and its volume grow
- an edited message is not updated; the first version is what remains stored
- the arrangement fits one bot on one host, because a second instance would write the same file

Neutral:

- message content stays on the host
- a failed write is reported and does not stop receiving; later messages are still accepted

## Alternatives considered

An external database service:

- offers concurrent access, remote clients, and operational tooling, but adds a service to install,
  back up, watch, and secure, plus the inbound access the host does not have; disproportionate for one
  community's bot.

Plain files, one per message or one append-only log:

- no dependency and easy to inspect, but no way to ask for messages by their state, no protection
  against a concurrent writer, no duplicate detection, and corruption or partial writes would have to
  be handled by hand.

An embedded key-value store:

- fast and simple to embed, but it offers no way to pick out messages by their shape or state, so
  processing state and lookups would need a hand-built index.

Synchronous access that blocks receiving:

- the least machinery, but a slow disk would stop the Telegram connection, and the backlog of updates
  would pile up in the process instead of the store.

## Revisit

Revisit when the delayed processing needs to record its own state, such as attempts and the last
error; when retention or pruning becomes necessary; when edited messages should be updated rather
than ignored; when a change to the stored structure justifies a migration mechanism; or when more
than one instance must share the stored messages.

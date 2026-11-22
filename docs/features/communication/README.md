# Communication

How the bot talks to Telegram: it receives every message in the chats it belongs to, answers the owner's commands directly, and delivers transcripts through a queue.

## Telegram connection

The bot connects with long polling through `teloxide`. It opens an outgoing connection and asks Telegram for new updates, so there is no webhook to register, no public URL, and no TLS certificate. That is also why the container publishes no ports; see [installation](../../Install.md).

The token comes from `TELEGRAM_API_TOKEN` or its `_FILE` form, and `docker compose up` fails before the container starts when neither is set; see [configuration](../configuration/README.md).

## BotFather

[BotFather](https://t.me/BotFather) creates the bot: `/newbot` yields the token. Two group settings decide what the bot can see:

- group privacy mode is on by default, so a bot in a group receives only messages addressed to it. Send `/setprivacy` to BotFather, disable it, then remove the bot from the group and add it back, because the change applies to new memberships only
- `/setjoingroups` controls whether the bot may be added to groups at all

## Ingestion

Every message Telegram delivers is logged and stored in SQLite; a redelivery of the same message is skipped by the `(chat_id, message_id)` uniqueness. Text and captions are stored as they arrive. Audio and video also record the metadata transcription needs: a `file_id`, a MIME type, a duration, and a size. Other media is stored as a message with its caption, without being downloaded or recorded as an attachment.

Ingestion never waits for transcription and vice versa: transcription runs in its own loop and reads what ingestion stored. See [storage](../storage/README.md) for the columns and [transcription](../transcription/README.md) for what happens to the attachments.

## Owner commands

The ingestor also carries the owner's control over transcription. A message in the owner's direct conversation that begins with a slash command is answered at once: a recognised command does its work, and any other one returns the help text. Answers are sent directly rather than queued, so the outbox stays reserved for transcripts. The four commands are in [access](../access/README.md).

A command from anywhere else, and ordinary text in the owner's direct conversation, is ignored; the message is still stored like any other. The bot also tells the owner, by direct message, when it records a chat for the first time. Both sends are best effort: a failure is logged and nothing is retried, because a lost answer can be repeated with another command.

## Outbox

The transcriber never sends a message itself. It queues the reply in an `outbox` table, and a delivery loop sends pending rows in order, paced to stay under Telegram's rate limits, marking each row as sent once Telegram accepts it. The delivery loop pauses only when the queue is empty, and queuing a reply ends that pause at once. The replies of one transcript and the marker that completes its attachment are written in a single transaction.

## Replies

Delivery is at-least-once: a crash or an interrupted send can deliver one reply twice rather than lose it. A long transcript is split into several messages; the first answers the source message and the rest follow in that thread.

## Shutdown

`SIGINT` (Ctrl-C) and `SIGTERM` (Compose, Docker, supervisord) request one cooperative shutdown. Each service checks the flag at its next work boundary: the ingestor stops polling but stores what the dispatcher had already queued, the transcriber and the outbox abandon idle pauses and in-flight work, finished database writes are never cancelled, and once the services end the database closes and Sentry flushes. An interrupted transcription is retried on the next start; an interrupted send may duplicate one reply.

## Further reading

- [ADR 0003: receive updates by long polling](../../adr/0003-long-polling.md)
- [ADR 0010: stop the bot gracefully on a signal](../../adr/0010-graceful-shutdown.md)

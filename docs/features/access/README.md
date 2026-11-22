# Access

Transcription is opt-in per chat. It spends OpenRouter credits and sends the media outside the bot, so a chat is transcribed only after the bot's owner approves it.

## The chat registry

The bot records every chat it receives a message from, one row per chat, and keeps a status for each:

- pending: recorded, waiting on a decision; its audio and video are skipped
- approved: its audio and video are transcribed
- blocked: deliberately refused; its audio and video are skipped

A chat is recorded the first time the bot receives a message in it, so adding the bot to a group and writing one message there is enough. Later messages refresh the title, the kind, and the last-seen time, never the status: a block survives, and a status the bot does not recognise reads back as pending.

## Owner commands

Only the configured owner can decide, and only in a direct conversation with the bot. A command anywhere else is ignored, and so is ordinary text in the owner's direct conversation; the message is still stored like any other. Answers are sent directly, not through the outbox, so the outbox stays reserved for transcripts.

- `/chats` lists the pending and approved chats, pending first, with each title, kind, and id. Blocked chats are not listed.
- `/approve <id>` transcribes the chat with that id.
- `/block <id>` stops transcribing the chat with that id.
- `/help` shows the commands.

An id the bot has not recorded yet answers that the chat is not recorded, so a chat must have written to the bot at least once before it can be approved. The owner's own conversation is recorded but not announced; every other chat recorded for the first time produces one direct message to the owner, carrying its id and how to approve it. Nothing is resent, so a lost notice is recovered with `/chats`.

## The owner

`OWNER_USER_ID` is required: without it no chat could ever be approved, so the bot would never transcribe. It must be a positive whole number, and a missing, blank, non-numeric, zero, or negative value fails startup. It is not a secret and stays in the environment, though it resolves with the same `_FILE` form as every other variable; see [configuration](../configuration/README.md).

Telegram reports a private chat's id as the user's own id, so the owner finds their id in the log line of a message they send the bot; `@userinfobot` shows it too. The owner must have written to the bot once before it can send them a notice or an answer, because a bot cannot start a conversation. See [installation](../../Install.md) for the Compose value.

## Finding a chat id

`/chats` shows the id of every recorded chat. A skipped attachment also logs its chat id:

```text
Attachment from chat <id> skipped: the chat is not approved, use /approve <id> from the owner's direct messages.
```

## No backfill

An attachment received before its chat was approved is skipped and marked processed, so it is never transcribed later, even after the approval. Only attachments that arrive afterwards are transcribed.

## What the registry does not affect

Ingestion and storage still record messages and attachment metadata from every chat the bot is in; the registry gates transcription alone. The transcriber never downloads media from a chat that is not approved, which is what keeps the cost bounded.

## Upgrade

`TRANSCRIBE_CHATS` no longer exists and a stale value is ignored. Every chat in an existing database starts pending, so an upgrade needs one approval per chat before transcription resumes.

Which model runs on the approved chats is in [transcription](../transcription/README.md); the variable table is in [configuration](../configuration/README.md).

## Further reading

- [ADR 0009: transcribe audio in a separate service](../../adr/0009-audio-transcription.md)
- [ADR 0017: configure the owner and approve chats from a direct message](../../adr/0017-configured-owner-approves-chats.md)

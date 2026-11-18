# Access

Transcription is opt-in per chat. It spends OpenRouter credits and sends the media outside the bot, so a chat is transcribed only when an operator lists it.

## The allow-list

`TRANSCRIBE_CHATS` holds a comma-separated list of numeric chat ids. Entries are trimmed and duplicates collapse; a non-numeric entry fails startup. Unset or empty means transcribe nothing: the bot still starts, ingests and stores messages, and logs a warning for each attachment it skips for this reason.

## Finding a chat id

Start with the list empty, send a voice message to the chat you care about, and read the id from the warning the bot logs for the skipped attachment:

```text
Attachment from chat <id> skipped: not a white listed chat, add to TRANSCRIBE_CHATS if needed.
```

Add that id to `TRANSCRIBE_CHATS` and restart. With Compose, the variable is passed through from the host environment or `.env`; see [installation](../../Install.md).

## No backfill

An attachment received before its chat was listed is skipped and marked processed, so it is never transcribed later, even after the chat is added. Only attachments that arrive after the restart are transcribed.

## What the list does not affect

Ingestion and storage still record messages and attachment metadata from every chat the bot is in; the list gates transcription alone. The transcriber never downloads media from a chat that is not listed, which is what keeps the cost bounded.

Which model runs on the allowed chats is in [transcription](../transcription/README.md); the variable table is in [configuration](../configuration/README.md).

## Further reading

- [ADR 0009: transcribe audio in a separate service](../../adr/0009-audio-transcription.md)

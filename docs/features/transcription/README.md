# Transcription

The bot transcribes audio and video from the chats the owner approved, using OpenRouter's Speech-to-Text API, and queues the transcript as a reply to the source message. Transcription runs beside ingestion, so a slow download or model call never holds up storage.

## Audio

An audio attachment is downloaded to a temporary file and sent to OpenRouter untouched. The container format the bot declares comes from the MIME type Telegram reports, falling back to Ogg.

## Video

A video is downloaded the same way, then `ffmpeg` extracts its first audio track into 16 kHz mono Ogg/Opus, and that file follows the audio path. `ffmpeg` ships in the runtime image.

## Memory

An attachment is never held whole. The OpenRouter client streams the file from disk in 96 KiB reads and base64-encodes each chunk straight into one JSON body allocated at its final size, so a request peaks at that body alone: about 4/3 of the input, roughly 28 MB at the 20 MB limit. No full-file copy and no intermediate base64 string exist. Audio above that limit is rejected before anything is allocated; see [ADR 0015](../../adr/0015-bound-transcription-memory.md).

## Acknowledgement

The bot sets the 👀 reaction on a message when it begins transcribing its audio or video, before the download, so the eye appears before the slow part of the work. The reaction is best effort: a failure is logged and the transcription continues. It stays on the message after the transcript is delivered. A chat that does not allow the bot to react only produces a warning.

## Skipped, not retried

Some attachments are marked processed without a transcript, because a retry could not change the outcome:

- a chat the owner has not approved
- files above the Bot API download ceiling of 20 MB
- audio above the 20 MB transcription input limit, which is the Bot API download ceiling and therefore only reachable for the audio extracted from a long video
- videos with no audio track
- videos when no usable `ffmpeg` is available
- silent audio, whose empty transcript produces no reply

## Retries

A download or model failure is retried on every scan while the source message is at most an hour old. The one-hour window bounds both the cost of a failing attachment and the number of retries; an attachment that ages out is never transcribed later.

An attachment from a chat the owner had not approved is marked processed when it is first seen, so approving the chat later transcribes only what arrives afterwards.

Queueing the replies and marking the attachment processed are one transaction. A crash or a rejected statement before the commit leaves the attachment pending and queues nothing, so the retry starts from scratch instead of delivering a chunk twice. An attachment another writer already completed is skipped rather than queued again.

The loop pauses only when a scan finds nothing to transcribe, and storing a new attachment ends that pause at once. A failed attachment is still retried on the pause's cadence, rather than immediately.

## Reply latency

The delay between a source message's Telegram send time and the moment Telegram accepts the first chunk of its transcript is reported to Sentry as the `transcription.reply.latency` distribution, in seconds.

## Model selection

The default model is `google/gemini-3.5-transcribe`. `TRANSCRIBE_MODEL_NAME` overrides it, and an empty value falls back to the default; a configured but unreadable value, such as a broken `TRANSCRIBE_MODEL_NAME_FILE`, fails startup. Startup logs `Using model <id> to transcribe; override with TRANSCRIBE_MODEL_NAME` once, so the model that spends the credits is visible in the console and in Sentry's structured logs.

Speech-to-Text models are a separate catalogue from the chat models, so an override must name a transcription model. `GET https://openrouter.ai/api/v1/models?output_modalities=transcription` lists them.

## Format coupling

The bot declares Ogg on the wire because a Telegram voice note already carries Ogg/Opus and the video extractor produces Ogg as well. Ogg is not universal, and OpenRouter does not report per-format support, so a provider that rejects it answers with HTTP 400 and the attachment is retried until it ages out of the window. Moving to a model that accepts only mp3 or wav means changing both the format the video is extracted to and the format the bot declares, in code.

## Keys and approved chats

`OPENROUTER_API_KEY` is required; see [configuration](../configuration/README.md). Which chats are transcribed is the owner's decision; see [access](../access/README.md). What is stored about each attachment is in [storage](../storage/README.md), and replies are delivered by the outbox described in [communication](../communication/README.md).

## Further reading

- [ADR 0009: transcribe audio in a separate service](../../adr/0009-audio-transcription.md)
- [ADR 0011: extract video audio with ffmpeg](../../adr/0011-extract-video-audio-with-ffmpeg.md)

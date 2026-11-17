# ADR 0006: Record attachment metadata for later processing

- Status: accepted
- Date: 2026-11-17

## Context

Transcribing a voice or video message is slow, can fail on its own, and must survive a restart, so it
cannot run while the message arrives. The bot has to write down enough about each attachment for a
later, separate reader to fetch it and process it, and it must not fetch the content itself while
receiving.

The messages are already kept in the local store on the host, but nothing there describes the media
a message carried. Without that description, a message that arrived while the bot was running is the
only trace of its attachment, and a restart loses the chance to process it.

## Decision

Record one attachment description per message, alongside the message and keyed the same way, holding
the handle Telegram assigns to the file, the media type, the duration, the size, and a flag saying
whether the attachment has been processed.

Treat a redelivery of the same message as the same attachment rather than a second one, so a restart
cannot duplicate the work.

Leave the content on Telegram and download it only when processing starts. The stored handle is the
only link to the content; the bot fetches nothing while receiving.

Split the two media families, audio and video, so a later reader can choose the right treatment
without inspecting the file, and count both received and recorded attachments so a failed write is
visible as a gap.

## Consequences

Positive:

- receiving stays fast, because nothing is downloaded on arrival
- the volume stays small, because only a description is kept, not the content
- a later reader selects work by the pending flag instead of re-reading Telegram updates
- the audio and video split lets that reader pick a treatment without opening the file
- a failed write is visible in the counters and does not stop receiving

Negative:

- the file handle is the only link to the content, so it must stay valid until processing runs; if it
  expires, or the bot is recreated, the content may become unreachable
- nothing is transcribed yet, so every description stays pending in this stage and the processing
  side is still to be built
- a message and its attachment description are recorded independently, so an interruption between
  them can leave a message whose media is unrecorded until Telegram delivers the update again

Neutral:

- a message that carries no media records nothing extra
- an attachment description is never updated; the first version wins, as it does for messages
- the pending side of the store has no reader yet, so this change writes and stops there

## Alternatives considered

Downloading the content on arrival:

- keeps everything in one place, but slows receiving, grows the stored volume, and repeats work a
  later reader would have to do anyway.

Keeping the media details on the message itself:

- no second record to keep in step, but most messages carry no media, so the message shape would be
  dominated by empty fields that mean nothing for text.

A separate queue or message broker:

- a familiar way to hand work to a worker, but it is a service to install, watch, and back up for a
  single bot that already keeps its data on the host.

Recording only the media type and looking the file up by message later:

- the smallest record, but Telegram offers no such lookup, so the file handle has to be written down
  while the update is in hand.

## Revisit

Revisit when processing starts and needs its own state, such as attempts and the last error; when a
message can carry more than one attachment; when content must be retained or pruned; or when the file
handle turns out not to be durable enough.

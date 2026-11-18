# ADR 0006: Record attachment metadata for later processing

- Status: accepted
- Amended: 2026-11-18, to correct the redelivery consequence, record the no-FK and pending-scan decisions, and record that the message and its attachment are written in one transaction.
- Date: 2026-11-17

## Context

Transcribing a voice or video message is slow, can fail on its own, and must survive a restart, so it
cannot run while the message arrives. A later reader needs enough about each attachment to fetch and
process it, and receiving must not fetch the content itself. Messages are already stored, but nothing
there describes the media they carried.

## Decision

Record one attachment description per message, alongside the message and keyed the same way, holding
the platform's file handle, the media type, the duration, the size, and a flag that says whether the
attachment has been processed. A redelivery is the same attachment, not a second one.

Leave the content on the platform and fetch it only when processing starts. Separate audio from video
so a reader can choose the treatment without opening the file, and count both received and recorded
attachments so a failed write is visible as a gap.

Record the media family in `kind` (`audio` or `video`) because a video note carries no MIME type.
There is deliberately no foreign key to `messages`: the ingestor writes the message and, when it
carries media, the attachment in one transaction, so an attachment failure rolls the message back and
the invariant holds without one. A constraint would need a manual upgrade of existing volumes,
because startup creates missing objects and never migrates (ADR 0005). `pending` joins `messages`
and reads rows whose message is at most an hour old; the `transcribed` flag means processed,
including a deliberate skip.

## Consequences

- receiving stays fast, because nothing is downloaded on arrival
- the store stays small, because only a description is kept
- a reader selects work by the pending flag instead of re-reading updates
- the file handle is the only link to the content, so it must stay valid until processing runs
- the message and its description commit together, so a failed description write leaves no
  half-stored message; the delivery is still lost, because the platform acknowledges an update when
  it is fetched and does not deliver it again
- an attachment from a chat that is not allow-listed is marked processed without being downloaded
- an attachment row written before the atomic write existed can lack its message row, and the pending
  scan's inner join hides it; a redelivery of that message writes the missing row

## Alternatives considered

- Download on arrival: keeps everything in one place, but slows receiving, grows the store, and
  repeats work a later reader would do anyway.
- Keep the media details on the message: one record fewer, but most messages carry no media, so the
  message shape would be dominated by empty fields.
- A separate queue or broker: a familiar way to hand work to a worker, but a service to install,
  watch, and back up for a bot that already stores its data locally.
- Record only the type and look the file up later: the smallest record, but the platform offers no
  such lookup, so the handle must be written down while the update is in hand.

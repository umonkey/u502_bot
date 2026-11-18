# ADR 0009: Transcribe audio in a separate service

- Status: accepted
- Date: 2026-11-17

## Context

Audio arrives as a file handle, and turning it into text means fetching the file and calling a
language model: a call that takes seconds, costs money, and fails for reasons the bot cannot control.
Doing that while a message arrives would stall receiving, and a restart in the middle would lose
fetched audio. The work also has to be bounded, because a backlog retried forever would spend credits
on old files nobody waits for. The audio is a private conversation, and it leaves the host when it is
transcribed.

## Decision

Run transcription as its own long-running task, beside receiving and delivery. It reads a small batch
of pending audio whose message is recent, downloads each file to a temporary location, asks the model
for a verbatim transcript, queues the text, and marks the attachment processed. It pauses when a scan
finds nothing to do, and the ingestor ends that pause at once when it stores a new attachment.

Bound the work by a one-hour window on the age of the message, so a failing file is retried only
while its message is recent. Treat "processed" as the flag that ends the work on every path,
including a file deliberately skipped.

Limit transcription to an explicit list of chats, so the bot never spends credits or discloses audio
on someone else's behalf. The list is optional and empty by default: an empty list transcribes
nothing, and each skipped attachment logs its chat id so an operator can learn the list. Require the
model provider's key and refuse to start without it, because an unconfigured transcriber would leave
every attachment pending while the process looked healthy.

## Consequences

- receiving never waits on a model call, so a slow or failing call costs no messages
- a transcript is queued durably, so a restart between the call and the reply loses nothing
- the age window bounds both cost and retries
- the chat list is a cost and privacy control at once
- audio is written to a temporary file, sent to a third party, and the transcript leaves the host, so
  every field sent is a deliberate disclosure
- a file that fails permanently is retried until its message ages out, and a pending attachment older
  than the window is never transcribed, with nothing reporting it
- the key is required, so a deployment that only receives messages cannot start without it
- audio that arrived while the chat list was empty is never transcribed retroactively
- the model is a third-party identifier that can be retired or repriced, so transcription can break
  without a change on our side

## Alternatives considered

- Transcribe on arrival: one place and no window, but every model call would sit in front of
  receiving, and a restart would lose the audio in flight.
- A scheduled job outside the process: keeps the work off the bot, but adds a second entry point, a
  second copy of the configuration, and a scheduler to watch.
- An optional transcriber that logs one warning: matches how monitoring degrades, but audio would sit
  pending while the bot looked healthy.
- Require the chat list before starting: the safest default, but the ids are learned from traffic, so
  an operator could not run the bot at all.
- Transcode to a preferred format: widens the accepted set, but adds a media tool, a second failure
  mode, and a CPU-heavy step.

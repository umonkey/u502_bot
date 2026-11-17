# ADR 0009: Transcribe audio in a separate service

- Status: accepted
- Date: 2026-11-17

## Context

Audio messages are recorded with the handle the platform assigns to the file, and nothing is
downloaded while they arrive. Turning that record into text means fetching the file and sending it
to a language model: a call that takes seconds, costs money per use, and fails for reasons the bot
cannot control, such as an unsupported container, a rate limit, or an expired file handle.

Doing that work while a message arrives would stall receiving behind every call, and a restart in
the middle would lose audio that had been fetched but not transcribed. The work also has to be
bounded: a backlog retried forever would spend credits on old files nobody is waiting for.

The audio is a private conversation. It leaves the host when it is downloaded and sent to the model
provider, and the transcript leaves it when it is logged or delivered.

## Decision

Run transcription as its own long-running task, beside receiving and beside delivery. It repeatedly
reads a small batch of audio attachments that are still pending and whose message is recent, and for
each one downloads the file to a temporary location, asks the model for a verbatim transcript,
records the text in the outgoing queue, and marks the attachment processed. It pauses for five
seconds whenever a scan finds nothing to do.

Bound the work by a one-hour window on the age of the message, so a file that keeps failing is
retried only while its message is recent and then left alone. Treat "processed" as the flag that
ends the work on every path, including a file deliberately skipped.

Limit transcription to an explicit list of chats, because a bot that transcribes into a chat nobody
named would spend credits and disclose audio on someone else's behalf. The list is configuration and is
optional: an empty list transcribes nothing, so the bot starts and ingests before the ids are known, and
each skipped attachment logs its chat id, which is how an operator learns the list. The list is checked
when a file is already selected, so a chat outside it is recorded as processed and logged once rather
than reconsidered on every pass.

Take the model from configuration, with a built-in default so the bot runs without it, and name the
model in use in the startup log. An operator must be able to tell which model spends the credits
without inspecting the deployment.

Require the model provider's key and refuse to start without it. A missing, empty, or whitespace-only
key is a startup failure, not a degraded mode, and it is validated before any task starts.

This last point is deliberately asymmetric with the optional monitoring credential, which a
deployment may leave empty to report nothing. The difference is what the component is: monitoring is
telemetry that can be absent without changing what the bot does, while transcription is the feature
this change adds. An unconfigured transcriber would leave every audio attachment pending for an hour
while the process looked healthy, and an empty key file would turn every call into an authentication
failure; refusing to start is loud where both of those are silent.

## Consequences

Positive:

- receiving never waits on a model call, so a slow or failing call costs no messages
- a transcript is queued durably, so a restart between the call and the reply loses nothing
- the age window bounds both cost and the number of retries a bad file can cause
- the chat list is a cost and privacy control at once
- the startup log names the model, so a change of model is visible without reading the deployment

Negative:

- a pending attachment whose message is older than the window is never transcribed, and nothing
  reports it as skipped
- a file that fails permanently is retried on every pass until its message ages out, so the failure
  and its cost are repeated
- the audio is written to the host's temporary directory and sent to a third party, and the
  transcript leaves the host entirely; this widens the existing policy that only what the code
  explicitly logs leaves the host, so every field added to that request is a deliberate disclosure
- the key is required configuration, so a deployment that only receives messages can no longer start
  without it; a missing key stops ingestion too, because validation precedes every task
- a chat is transcribed only after its id is listed, and audio that arrived while the list was empty
  is marked processed, so it is never transcribed retroactively
- the model is a third-party identifier that can be retired or repriced, or that can reject the
  container the platform produces, so transcription can break without a change on our side

Neutral:

- an empty transcript, from a file with no speech, is marked processed and produces no reply
- a transcript longer than the platform accepts is split into several replies, only the first of
  which answers the original message
- the temporary copy is removed when the attempt ends, including on the failure paths, so no cleanup
  job is needed

## Alternatives considered

Transcribing on arrival:

- one place and no window, but every model call would sit in front of receiving, and a restart would
  lose the audio in flight.

A scheduled job outside the process:

- keeps the work off the bot, but adds a second entry point, a second copy of the configuration, and
  a scheduler to watch, for work that only exists while the bot runs.

An optional transcriber that logs one warning and lets the bot run without it:

- matches how monitoring degrades, but audio would sit pending for an hour while the bot looked
  healthy, and nothing would say the feature was off. This differs from an empty chat list, which is
  a supported state: the transcriber still runs, skips visibly, and publishes the ids to add.

Requiring the chat list before the bot starts:

- the safest default, but the ids are learned from the traffic, so an operator would have to find them
  with another client or a one-off build before the bot could run at all.

Transcoding to a format the provider prefers:

- widens the accepted container set, but adds a media tool to the image, a second failure mode, and a
  CPU-heavy step, for a container the chosen endpoint already accepts.

## Revisit

Revisit when a failing file needs an attempt count or an explicit give-up, when transcripts should be
pruned, when the chat list should come from the store rather than configuration, or when the
provider's retention and privacy terms change.

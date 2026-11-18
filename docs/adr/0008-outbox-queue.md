# ADR 0008: Queue outgoing replies in an outbox

- Status: accepted
- Date: 2026-11-17

## Context

A transcript is the result of a slow, costly call and must reach the chat that sent the audio.
Producing and delivering that text are not equally reliable: sending can fail, be throttled, or be
slower than the work that produced it, and text held only in the process is lost when the container
is replaced. The reply must also answer the original message.

## Decision

Record every outgoing reply durably before it is sent, and deliver those records from a separate,
long-running sender. A record holds the destination, the text, the time it was queued, and the
message it answers; the time it was delivered is the only marker of success.

Send in insertion order, in small batches, pausing between sends so a burst does not trip the
platform's rate limit. Leave a record unsent when delivery fails, so the next pass retries it.

## Consequences

- a reply survives a restart, a crash, and a slow or unavailable platform
- producing and delivering fail independently, so a rejected message does not stop transcription
- the queue is one more table in the store the bot already keeps, so nothing new is installed
- delivery is at-least-once: an interruption between a successful send and the marker that follows
  re-sends that one message
- a record the platform rejects forever is retried on every pass, because nothing counts attempts
- delivered records are kept, so the queue only grows

## Alternatives considered

- Send inline from the producer: the fewest moving parts, but it couples transcription to the
  platform's availability and loses the text on a restart.
- A message broker: retries and acknowledgements built in, but a service to install, watch, and back
  up for a bot whose durable store is already local.
- An in-memory retry queue: no new table, but a restart loses everything queued, which is the
  failure this decision exists to prevent.

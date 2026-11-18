# ADR 0008: Queue outgoing replies in an outbox

- Status: accepted
- Amended: 2026-11-18, to record rendering, batching, the source-time join, and producer atomicity.
- Amended: 2026-11-18, to record the retention and pruning service.
- Date: 2026-11-17

## Context

A transcript is the result of a slow, costly call and must reach the chat that sent the audio.
Producing and delivering that text are not equally reliable: sending can fail, be throttled, or be
slower than the work that produced it, and text held only in the process is lost when the container
is replaced. The reply must also answer the original message.

## Decision

Record every outgoing reply durably before it is sent, and deliver those records from a separate,
long-running sender. A record holds the destination, the text as escaped Telegram HTML rendered by
the producer, the time it was queued, and the message it answers; the time it was delivered is the
only marker of success.

Send in insertion order, in small batches, pausing between sends so a burst does not trip the
platform's rate limit. Leave a record unsent when delivery fails, so the next pass retries it.

Read rows in id order in batches of ten, pause two seconds between sends as the rate-limit bound, and
pause five seconds when the queue is empty. The source message's Telegram time is read through a left
join on `messages`, not stored, so the latency metric needs no schema change.

A transcript's reply rows and the marker that completes its attachment are written in one
transaction, the marker first. A run that finds the marker already set queues nothing.

## Consequences

- a reply survives a restart, a crash, and a slow or unavailable platform
- producing and delivering fail independently, so a rejected message does not stop transcription
- the queue is one more table in the store the bot already keeps, so nothing new is installed
- delivery is at-least-once: an interruption between a successful send and the marker that follows
  re-sends that one message
- a record the platform rejects forever is retried on every pass, because nothing counts attempts
- a transcript's replies and its completion marker commit together, so a producer that fails before
  the commit leaves nothing queued and a retry cannot duplicate the first attempt's rows
- delivered rows are pruned once they are older than the retention, so the queue does not grow
  forever (ADR 0016); unsent rows are never pruned, and a pending reply older than the retention
  loses its latency sample because its source message is gone

## Alternatives considered

- Send inline from the producer: the fewest moving parts, but it couples transcription to the
  platform's availability and loses the text on a restart.
- A message broker: retries and acknowledgements built in, but a service to install, watch, and back
  up for a bot whose durable store is already local.
- An in-memory retry queue: no new table, but a restart loses everything queued, which is the
  failure this decision exists to prevent.

# ADR 0008: Queue outgoing replies in an outbox table

- Status: accepted
- Date: 2026-11-17

## Context

The bot now produces replies: a transcript of an audio message is the result of a slow, costly call
to a language model, and it has to reach the chat that sent the audio. Producing that text and
delivering it are not equally reliable. Sending can fail, can be throttled, and can be slower than
the work that produced the text, while a transcript held only in the running process is lost when
the container is replaced, which is exactly the case transcription was built to survive.

Delivering from the component that produces the text would tie transcription to the platform's
availability: a rejected message would stall the next transcript, and a restart in the middle would
drop the text without a trace. The reply also has to answer the original message, so the sending
side has to remember which message a given text belongs to.

## Decision

Record every outgoing reply durably before it is sent, and deliver those records from a separate,
long-running sender. A record holds the destination chat, the text, when it was queued, and the
message it answers; the time it was delivered is the only marker of success, so an unsent record is
one without that time.

Send in insertion order, one small batch at a time, pausing between sends so a burst does not trip
the platform's rate limit. Answer the original message when a record names one, and send a plain
message otherwise. Leave a record unsent when delivery fails, so the next pass retries it rather
than losing it.

## Consequences

Positive:

- a produced reply survives a restart, a crash, and a slow or unavailable platform
- producing and delivering fail independently, so a rejected message does not stop transcription
- the queue is one more table in the store the bot already keeps, so there is nothing new to install
  or operate
- a backlog is readable from the store at any moment, and delivery is observable as the gap between
  the queued counter (`outbox.messages.added`) and the delivered counter (`outbox.messages.sent`)

Negative:

- delivery is at-least-once: an interruption between a successful send and the marker that follows
  re-sends that one message on the next pass
- there is no ordering guarantee beyond insertion order, so a retried record is delivered after
  records queued later; with one sender working one batch at a time this is rare but not excluded
- a message the platform rejects forever is retried on every pass, because nothing counts attempts
  or gives up
- the reply target is a message identifier, not a forum topic, so a chat with topics would need a
  second field

Neutral:

- delivered records are kept, not deleted, so the queue only grows
- a persistent gap between the two counters is a stuck or failing sender, not a lost message

## Alternatives considered

Sending inline from the component that produces the text:

- the fewest moving parts, but it couples transcription to the platform's availability, loses the
  text on a restart, and makes a rejected message stall the work behind it.

A message broker:

- a familiar way to hand work to a sender, with retries and acknowledgements built in, but it is a
  service to install, watch, and back up for a single bot whose durable store is already on the host.

An in-memory retry queue inside the producing process:

- no new table, but a restart loses everything still queued, which is the failure this decision
  exists to prevent.

## Revisit

Revisit when a rejected reply must be abandoned after a number of attempts, when more than one
sender runs at once, when delivery order across chats matters, or when forum topics must be
addressed separately from the message being answered.

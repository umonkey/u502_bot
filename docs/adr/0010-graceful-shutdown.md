# ADR 0010: Stop the bot gracefully

- Status: accepted
- Date: 2026-11-18

## Context

A restart or an interrupt ended the process where it stood: the local store was never closed,
monitoring events were not flushed, and messages the platform had already acknowledged could be lost.
Work is already durable and delivery is at-least-once, so a stop need not finish every item; it needs
to reach a point with nothing in flight, inside the short grace period a container allows.

## Decision

Treat a stop as one cooperative request. The interactive interrupt and the platform's termination
signal ask the same, and the request is sticky, so a signal that arrives before the services are
ready is honoured when they start. Services stop at a work boundary: they begin nothing new, abandon
idle pauses and calls to other systems, and never interrupt a write already begun. Receiving stores
what the platform had already handed over and only then stops polling. When every service has ended,
the local store is closed and monitoring is flushed, last of all. A graceful stop reports success.

## Consequences

- a stop is fast and bounded, so it does not delay a restart
- acknowledged updates are stored, and the local store is closed cleanly
- an abandoned call to another system is repeated on the next start, so a transcription already paid
  for can be paid for twice, and an interrupted delivery may be repeated, which at-least-once
  delivery already accepts
- the supervisor's grace period stays the hard bound, and a second signal does not force an exit

## Alternatives considered

- Finish in-flight work and lengthen the grace period: safest per item, but every restart would wait
  for the slowest external call.
- Rely on the runtime's teardown: no code, but the store is never closed and monitoring is lost.
- Handle only the interrupt: covers development, but leaves the container's signal as a hard kill.

## Revisit

Revisit when in-flight work must be drained rather than abandoned, or when a second signal must force
an immediate exit.

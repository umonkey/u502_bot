# ADR 0013: Wake an idle service instead of waiting out its pause

- Status: accepted
- Amended: 2026-11-18, so the two directions of an edge are distinct.
- Date: 2026-11-18

## Context

The transcriber and the outbox each end a pass that found nothing to do with a fixed five-second
pause, and a reply crosses both stages. A voice note stored just after the transcriber's scan waits
out that pause, and a transcript queued just after the outbox's scan waits out another, so a reply can
sit ten seconds before any work starts even though the work is already written. The pause still has a
purpose: it is the retry cadence for a failing attachment and the bound that keeps an idle loop from
spinning.

## Decision

Give each edge two one-way values over one counter, shaped like the shared shutdown flag: one side
announces work and the other waits for it. The two directions are separate values, so a consumer
cannot pulse and a producer cannot wait. The ingestor pulses the transcriber after it stores an
attachment, and the transcriber pulses the outbox after it queues a reply. A pause ends at once when
its pulse has arrived, including a pulse that arrived before the pause began, and each pulse is
consumed once, so a stale one cannot turn the pause into a spin.

Keep the five-second pause as the backstop that ends an idle wait when no pulse comes. A failed
attachment stays on that cadence, because no failure path pulses, so the pause remains the retry
bound. The two-second pause between outbox sends is untouched, since it is a rate limit rather than an
idle wait.

## Consequences

- a reply starts as soon as the work for it exists, instead of up to five seconds per stage later
- the composition creates and wires two more pairs, and each edge names the work it announces
- a pulse is a hint rather than a command: the worst a mistaken one costs is one extra scan
- the pause remains a deliberate rate and retry bound, not a leftover
- no new dependency, schema, or configuration variable
- a mistaken direction is a compile error rather than a review finding

## Alternatives considered

- Scan more often: shortens the wait for everything, but every idle service reads the store on the
  shorter interval and a failing attachment is retried more often.
- Share one wake between both edges: half the handles, but a pulse meant for the outbox would wake
  the transcriber, which then scans an empty batch and loses its own pause.
- Pass a bare notification primitive and select on it in both loops: the same latency with fewer
  types, but the primitive is duplicated and a pulse that arrives before the wait is easy to lose.

# ADR 0010: Stop the bot gracefully

- Status: accepted
- Date: 2026-11-18

## Context

The bot is a long-lived process that receives messages, stores them locally, transcribes audio, and
delivers queued replies. It runs in a container that is restarted on redeploys and stopped from a
terminal during development. Until now neither an interrupt nor the platform's termination signal was
handled, so the default disposition ended the process where it stood: local storage was never closed
cleanly, monitoring events still in flight were not flushed, and messages the messaging platform had
already acknowledged could be lost.

Outgoing work is already durable and delivery is at-least-once, so a stop does not have to finish
every item: the queue outlives the process and the next start retries it. What the process does have to
reach is a point where no work is in flight and the resources it owns are released, and it has to get
there inside the short grace period a container runtime allows between the signal and the kill.

## Decision

Treat a stop as one cooperative request per process. The interactive interrupt and the platform's
termination signal produce the same request, so a terminal and a container behave alike. The request is
sticky: it is remembered even when it arrives before the services are ready, and every service sees it
when it next looks.

Services check the request at a boundary between work items. They start no new message, transcript, or
delivery, they abandon idle pauses and calls to other systems instead of waiting them out, and they
never interrupt a local write that has already begun. Receiving stops by refusing new input, stores
whatever the messaging platform had already handed over, and only then stops polling. Once every
service has ended, the local store is closed and monitoring is flushed; the monitoring flush is the
last thing the process does. A graceful stop reports success, and a failure to start still reports
failure.

The supervisor's grace period is the hard bound on the whole sequence. The bot is expected to finish
comfortably inside it, and a second signal is not treated as a request to force an immediate exit.

## Consequences

Positive:

- a stop is fast and bounded, so it does not delay a restart
- acknowledged updates are stored instead of dropped
- local storage is closed cleanly, so its on-disk state needs no recovery
- monitoring flushes before exit, so the last events of a run are not lost
- the exit status distinguishes a clean stop from a failed start

Negative:

- a call to another system in flight is dropped, so a transcription already paid for is paid for again
  on the next start
- a delivery interrupted between the platform accepting it and the record of that acceptance may be
  repeated on the next start; at-least-once delivery already accepts this
- a message acknowledged by the platform but not yet stored when the process must be killed is still
  lost, because the grace period, not the bot, is the final bound

Neutral:

- a service that ends on its own does not stop the others; only a signal does, which keeps a single
  failing loop from taking the process down
- the request lives for the life of the process, so nothing can be restarted in place after it

## Alternatives considered

Finish the work in flight, including calls to other systems, and lengthen the grace period to match:

- the safest per item, but every restart would wait for the slowest external call, and the runtime's
  bound would have to grow with it.

Rely on the runtime's teardown:

- no code at all, but the store is never closed and monitoring is lost, and an acknowledged message
  can fail to be stored, which is the outcome this decision exists to prevent.

Handle only the interactive interrupt:

- keeps the development experience, but leaves the signal a container actually sends on the default
  disposition, so the production case would still be a hard kill.

## Revisit

Revisit when in-flight work must be drained rather than abandoned, when a second signal must force an
immediate exit, when the platform's grace period changes, or when the bot grows another long-running
task that needs the same boundary.

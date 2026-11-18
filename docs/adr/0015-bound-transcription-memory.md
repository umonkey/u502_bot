# ADR 0015: Bound transcription memory and keep filesystem work off the runtime

- Status: accepted
- Date: 2026-11-18

## Context

Transcribing a maximum-size attachment held several whole copies of it at once, so the peak of a
request was an unstated multiple of the input and grew with the largest file the messaging platform
allows. The transcription client itself had no ceiling, and a caller could reach it with an
unmeasured file.

The same paths performed blocking filesystem calls on async workers, stalling unrelated tasks for
the duration of each syscall.

## Decision

The transcription client owns the input ceiling and rejects a larger file permanently rather than
retrying it. It reads the file incrementally and encodes each piece directly into a request body
allocated once at its final size, so the audio is never held whole beside an encoded copy.

Filesystem work moves off async workers: configuration and directory creation use asynchronous I/O,
and operations without an async form run on blocking threads.

## Consequences

- a request peaks at roughly its encoded body, about a third above the input, however the client is
  called
- a file above the ceiling is marked processed and skipped, because no retry can change its size
- the ceiling tracks the platform's download limit, so raising that limit reopens this decision
- configuration loading is asynchronous, which makes its callers and their tests asynchronous too
- no new dependency, and observable behaviour, telemetry, and shutdown are unchanged

## Alternatives considered

- Stream the request body: the same peak, but it needs a new dependency and an adapter to encode
  across an asynchronous read, and it loses the known request length.
- Move the whole configuration load to a blocking thread: less test churn, but it adds an error case
  the shared error type has no honest place for.
- Read the file whole and encode it separately: the body is still sized correctly, but the copies
  this decision removes survive.
- Enforce the ceiling only in the caller: it leaves the client unbounded when called directly or
  with an unknown size.

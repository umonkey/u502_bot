# ADR 0007: Send application logs to Sentry

- Status: accepted
- Amended: 2026-11-18, to enumerate what actually leaves the host.
- Date: 2026-11-17

## Context

Errors and counters reach monitoring, but ordinary operations are visible only in container logs that
nobody reads, so diagnosing anything means shell access to the production host. The earlier decision
that kept chat content on the host is now the obstacle: the interesting question is what the bot
actually did, and that is the content that was excluded.

## Decision

Forward the application's structured logs at informational severity and above, including message
text and captions, transcripts, sender ids and names, chat ids, file ids, MIME types, and file names.

Keep console verbosity independently configurable, so local detail never widens what is transmitted.
Keep the integration optional, and keep automatic person-identifying enrichment and metric attributes
off, so only what the code logs explicitly leaves the host.

Forward only events whose target starts with this crate's name; dependency events become issues
through the error layer but are not forwarded as logs.

## Consequences

- an operator can read activity in monitoring instead of opening a shell on the host
- message text, transcripts, identifiers, and media metadata are stored by a third party for its
  retention period and are visible to everyone with access to the project
- log volume grows with traffic and consumes a quota separate from errors
- a failure appears twice, once as a log and once as an issue
- the old guarantee that content stays on the host is now only a policy: every new log line is a
  deliberate decision about what leaves the host

## Alternatives considered

- Errors and metrics only: the status quo, which is why a shell was still needed.
- Breadcrumbs: content leaves only alongside a failure, so normal operation stays invisible until
  something breaks.
- Sanitised metadata without text or identifiers: keeps the old boundary but loses the answer to what
  was said.
- Self-hosted monitoring: keeps the data on owned infrastructure, but adds a service to operate.

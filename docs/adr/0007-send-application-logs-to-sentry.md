# ADR-0007: Send application logs to Sentry

- Status: accepted
- Date: 2026-11-17

## Context

The bot runs unattended on a shared host. Errors and panics reach the monitoring service, and a
handful of counters describe activity, but ordinary operations are visible only in the host's
container logs, which nobody watches. Diagnosing anything, from a silently ignored message to a
misbehaving deployment, therefore requires shell access to the production host.

An earlier reporting decision deliberately kept the content of chats on that host, so no message
text, identifiers, or media metadata were transmitted. That boundary is now the obstacle: the
interesting part of an operator's question, "what did the bot actually do", is exactly the content
that was excluded, and opening a shell to answer it is the thing the monitoring service exists to
avoid.

## Decision

Forward the application's structured logs at informational severity and above to the monitoring
service, including the content of the messages the bot handles and the identifiers of the chats and
users involved.

Keep the console verbosity independently configurable, so raising detail locally never widens what
is transmitted. Keep the integration optional: an unconfigured deployment reports nothing. Keep
automatic user-identifying enrichment disabled and the metrics free of attributes; only what the
code explicitly logs is sent.

## Consequences

Operators can read the bot's activity in the monitoring service and no longer need host access to
understand it.

Message text, chat and user identifiers, and media metadata are stored by a third party for its
retention period and are visible to everyone with access to the project. Log volume now grows with
traffic and consumes a quota separate from errors. A failing operation appears twice, once as a log
and once as an issue. An unconfigured deployment behaves exactly as before.

The old guarantee that content stays on the host is no longer a property of the system, only a
policy: nothing in the code enforces it, so every new log line is a deliberate decision about what
leaves the host and must be reviewed as such.

## Alternatives considered

Errors and metrics only:

- the status quo; it is why a shell on the host was still needed.

Breadcrumbs:

- content leaves only alongside a failure, so normal operation stays invisible until something
  breaks.

Sanitised metadata without text or identifiers:

- keeps the old boundary but loses the answer to "what was said", which is the reason to read the
  logs at all.

Self-hosted monitoring:

- keeps the data on owned infrastructure but adds a service to operate, which the project
  deliberately avoids.

## Revisit

Revisit when data-retention or access-control requirements tighten, when log volume or the quota
becomes a problem, or when per-environment log levels are needed.

# ADR 0003: Receive Telegram updates by long polling

- Status: accepted
- Date: 2026-11-17

## Context

u502_bot runs as a single small container on a modest, shared host. The host has no inbound ingress,
no public certificate, and no reverse proxy, and the bot must recover unattended after restarts
without an operator present.

Telegram offers two ways to receive updates: the bot can fetch them by polling, or Telegram can push
them to a public HTTPS endpoint. The transport choice fixes what the deployment must expose and what
has to be operated, so it needs to be recorded before the bot loop lands.

## Decision

Receive updates by long polling from the single bot process, with update dispatch handled in-process
and with TLS provided by a pure-Rust stack, so the image needs no system TLS library.

The container therefore exposes no ports, serves no inbound traffic, and owns no certificates.

## Consequences

Positive:

- no ingress, load balancer, or certificate renewal becomes part of the deployment
- no webhook endpoint means no path secret to generate, rotate, or leak
- rollback is a process restart, because there is no registration with Telegram to undo
- the runtime image stays smaller, with no reverse proxy and no system TLS stack

Negative:

- updates arrive with added latency compared with a pushed webhook
- the update offset lives only in memory, so a restart can deliver the last batch again; duplicate
  log lines are accepted
- one long-lived outbound connection is held for the life of the process

Neutral:

- polling retries internally after transient network failures, so a long outage produces repeated
  error logs rather than a restart

## Alternatives considered

Webhooks:

- lower delivery latency, and the intended design for busy bots, but they require a public HTTPS
  endpoint, a reverse proxy, and a path secret, which are disproportionate for one small container
  that handles a community's traffic.

A third-party relay that accepts webhooks and exposes them for polling:

- removes the ingress requirement, but adds an external service to trust, pay for, and operate, for a
  benefit this bot does not need.

## Revisit

Revisit when the deployment gains a public endpoint for another reason, or when delivery latency
becomes visible to users.

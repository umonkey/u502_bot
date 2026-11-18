# ADR 0003: Receive updates by long polling

- Status: accepted
- Amended: 2026-11-18, so the redelivery consequence matches the deduplicating store.
- Date: 2026-11-17

## Context

The bot runs as one small container on a shared host with no inbound ingress, no public certificate,
and no reverse proxy, and it must recover unattended after a restart. The platform can either push
updates to a public endpoint or have the bot fetch them, and that choice fixes what the deployment
must expose.

## Decision

Fetch updates by long polling from the single bot process, with dispatch handled in process and
transport security provided inside the binary. The container opens no ports, serves no inbound
traffic, and owns no certificates.

## Consequences

- no ingress, load balancer, certificate, or endpoint secret enters the deployment
- rollback is a restart, because there is no registration to undo
- updates arrive with more latency than a pushed update, and one long-lived outbound connection is
  held for the life of the process
- the update offset lives in memory, so a restart can deliver the last batch again; the store
  recognises the duplicate by `(chat_id, message_id)`, skips the second copy, and logs it once
  (ADR 0005)
- transient failures are retried internally, so a long outage produces repeated errors rather than a
  restart

## Alternatives considered

- Webhooks: lower latency and the right design for a busy bot, but they need a public endpoint, a
  proxy, and a path secret, which is disproportionate for one small container.
- A relay that accepts pushes and exposes them for polling: removes the ingress requirement, but adds
  a service to trust, pay for, and operate.

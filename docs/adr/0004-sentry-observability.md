# ADR 0004: Report errors and metrics to Sentry

- Status: accepted
- Amended: 2026-11-18, to record distributions and the relationship to ADR 0007.
- Date: 2026-11-17
- Related: ADR 0007 supersedes the Context's no-chat-content restriction and adds structured logs; this ADR owns issues and metrics.

## Context

The bot runs unattended, and failures are visible only in container logs that nobody reads: a crash
surfaces later as a restart, and a failed update can pass unnoticed. The host has no inbound ingress,
so no monitoring endpoint can be scraped from outside. Reporting must not add a service to operate,
must not depend on a system TLS library, and must not send chat content.

## Decision

Send errors, panics, counters, and distributions for duration, tokens, cost, and latency to a hosted
monitoring service, configured optionally: a missing or invalid destination logs one warning and the
bot runs normally.

Report every error-level event and every panic without per-call-site work, and keep automatic
person-identifying enrichment and metric attributes off, so only what the code logs explicitly leaves
the host. Keep the transport inside the binary so the runtime image needs no system TLS library, and
exit through the normal return path so queued events flush before the process ends.

## Consequences

- failures are reported without extra call sites, and activity is visible as counters and
  distributions
- an unconfigured bot pays nothing: no client, no transport, no reporting
- a third party holds error data and controls delivery, so reporting depends on its availability
- a typo in the destination is silent apart from one warning, which looks like a quiet system
- the integration changes global logging setup, so it is exercised at runtime rather than in unit
  tests

## Alternatives considered

- The library's default async transport: it builds an extra HTTP stack and a native TLS dependency
  the image cannot carry.
- A metrics endpoint scraped by something else: the standard for metrics, but it needs ingress, a
  scraper, and a certificate the deployment does not have.
- Self-hosted monitoring: the same protocol with data on owned infrastructure, but one more service
  to operate.
- Structured logs only: no new dependency, but nobody reads them, so errors still go unseen.

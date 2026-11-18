# ADR 0001: Use Rust for the bot

- Status: accepted
- Date: 2026-11-17

## Context

The bot runs unattended as one small container on modest, shared hardware. It must hold a small
footprint, stay up without attention, treat every incoming message as untrusted, and ship as a single
binary that is easy to deploy.

## Decision

Implement the bot in Rust, compiled to a single binary and run as the only application process in the
container. No garbage collector or language runtime stays resident, and the type system makes
unhandled paths and null values a compile-time concern, at the cost of slower builds and a steeper
learning curve.

## Consequences

- idle memory is bounded by the binary's own allocations, with no collector pauses
- error paths are explicit and checked by the compiler, so failures stay visible in review
- the runtime image needs only the C library, so the published image stays small
- builds are slow, and toolchain and dependency upgrades need periodic review

## Alternatives considered

- Python: quickest to iterate, but a resident interpreter makes it the heaviest footprint, and type
  and error checking happen at runtime.
- Go: a comparable footprint with simpler concurrency, but a larger runtime and nil-able values.
- Node.js: good library support, but a resident heap and a much larger image.
- A shell script: no credible path to safe concurrent update handling.

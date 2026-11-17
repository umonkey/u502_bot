# ADR 0001: Use Rust for the u502 Telegram bot

- Status: accepted
- Date: 2026-11-17

## Context

u502_bot is a Telegram bot for a local car community. It runs as a single small container on modest,
shared hardware and must stay up unattended: updates are handled continuously, and periodic
maintenance runs from cron inside the same image.

The constraints that drive the language choice are:

- footprint: the container should leave most of a small host's memory and CPU free
- uptime: restarts are exceptional events, not routine, because each one costs availability
- input handling: every message arrives from the network and from untrusted chat text
- distribution: a single binary keeps the image small and the deployment boring
- maintainability: the bot logic is small, so compile-time correctness is affordable

The project bootstrap (issue #1) already targets stable Rust, edition 2024, and a
`debian:trixie-slim` runtime image.

## Decision

Implement u502_bot in Rust on the stable channel with edition 2024, distributed as a single binary
linked against the trixie runtime's glibc, and supervised by `supervisord` inside the container.

Supporting choices: `clap` for CLI parsing, `tracing` with `tracing-subscriber` for logging, and
`teloxide` for the Telegram client when the bot loop is implemented.

Rust is selected because it gives the lowest combined runtime resource use and defect rate among the
realistic candidates: no garbage collector or language runtime stays resident, `Result` and
exhaustive `match` make unhandled paths a compile-time concern, and there are no null pointers. The
price is compile time and a steeper learning curve.

## Consequences

Positive:

- idle memory is bounded by the binary and its own allocations, with no collector pauses or heap
  growth over time
- `Result`-based error handling plus exhaustive matching, enforced by `clippy -D warnings` in CI,
  keeps error paths visible instead of implicit
- the runtime image needs only libc and `ca-certificates`, so the published image stays small
- a single binary is trivial to supervise, restart, and replace on rollback

Negative:

- compile times are the slowest of the candidates; mitigated by a dependency-cache layer in the
  Dockerfile and `Swatinem/rust-cache` in CI
- the async and Telegram ecosystems move quickly, so dependency upgrades need review; `Cargo.lock`
  is committed to keep builds reproducible
- toolchain updates are a maintenance duty; a new stable release can introduce clippy lints that
  fail CI until addressed
- the binary depends on matching Debian versions between builder and runtime stages

Neutral:

- no interactive runtime for ad-hoc experiments; exploratory work happens through `cargo run`
  subcommands and tests

## Alternatives considered

Python with a Telegram client library:

- fastest to iterate and likely the most familiar option, but a resident interpreter plus
  dependencies is the heaviest footprint, and type and error checking happen at runtime.

Go:

- comparable footprint and simpler concurrency, but a larger runtime than Rust's, a weaker type
  system with no exhaustive matching, and nil-able values; errors are explicit but easy to ignore.

Node.js with TypeScript:

- good Telegram library support, but a resident V8 heap with garbage collection and a much larger
  image.

POSIX shell:

- rejected outright; no credible path to safe concurrent update handling.

## Validation

The resource claim is an acceptance criterion, not an assumption. After the bot loop lands, record
here:

- idle RSS of the bot process in the running container
- published image size for `latest`
- restart count over one week of operation

If idle RSS or image size exceeds the host budget, supersede this ADR rather than bending the
deployment.

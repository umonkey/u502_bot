# ADR 0004: Report errors and metrics to Sentry

- Status: accepted
- Date: 2026-11-17

## Context

u502_bot runs unattended as a single container on a modest, shared host. Errors and panics are
visible only in the container's logs, which nobody watches: a failure surfaces later as a
`supervisord` restart, and a failed update can pass unnoticed entirely. The project has no APM, no
metrics backend, and no inbound ingress through which to expose one (ADR 0003).

The bot needs error reporting and a little operational signal without adding a service to operate,
without a system TLS dependency (ADR 0003), and without sending chat content or user identifiers off
the host.

## Decision

Add the `sentry` crate, version 0.49, with `default-features = false` and the `backtrace`,
`contexts`, `metrics`, `panic`, `tracing`, `ureq`, and `rustls` features. The integration is
optional and non-fatal.

Configuration: the DSN is read through `crate::infra::config::get_secret`, so `SENTRY_DSN_FILE` wins
over `SENTRY_DSN` exactly as `<NAME>_FILE` does for the Telegram token (ADR 0002). The value is
parsed explicitly into `sentry::types::Dsn` before initialization, because `sentry::init` panics on
an invalid DSN. A missing, empty, or invalid DSN only logs one warning, emitted after the `tracing`
subscriber is installed, and the bot runs normally; the DSN itself is never logged. Compose sets
`SENTRY_DSN_FILE=/run/secrets/sentry_dsn` and mounts `.secrets/sentry_dsn`, where an empty file is
the documented way to run without Sentry.

Transport: `ureq`, sentry's synchronous transport, which sends on its own thread, with sentry's
`rustls` feature and the `ring` crypto provider. `ring` was already in the dependency graph through
teloxide's rustls path, so the Dockerfile is unchanged: no system TLS library, no `libssl`, and no
`cmake`.

Sentry's defaults are deliberately not used. The default `transport` feature is `reqwest` plus
`native-tls`; `native-tls` needs OpenSSL headers in the builder and `libssl` at runtime, which
contradicts ADR 0003. Choosing `reqwest` plus `rustls` instead would enable `aws-lc-rs`, a
CMake-built C dependency the slim builder image cannot build, and would add a second HTTP stack next
to teloxide's.

Errors: `src/infra/sentry.rs` owns the single `tracing` subscriber, composed of `registry()`, an
`fmt` layer filtered by `RUST_LOG`, and `sentry::integrations::tracing::ErrorLayer` filtered to
`ERROR`, plus Sentry's panic hook. Only `ERROR` events and panics are reported; `RUST_LOG` controls
console verbosity only.

Metrics: one application counter, `telegram.messages.received`, incremented at the top of
`handle_message` with no attributes. `send_default_pii` stays off, so message text, chat ids, and
usernames are never attached, and the INFO-level "received message" log does not reach Sentry.

Shutdown: `main`, `cli::run`, and `start::run` return `std::process::ExitCode` instead of calling
`process::exit`, so the client guard drops and flushes queued events before the process ends.

## Consequences

Positive:

- every `tracing::error!` event and every panic is reported without extra call sites
- message volume is visible as a counter while no message content leaves the host
- an unconfigured bot pays nothing: no client, no `ErrorLayer`, and no transport thread
- TLS stays pure Rust, so the runtime image gains no system library and the builder gains no CMake
- the DSN follows the existing secret convention and never enters the container environment

Negative:

- a third-party service receives error data, and delivery depends on its availability and quota
- `ureq` adds a second HTTP stack and a blocking transport thread; contained, but more to maintain
- the dependency tree and binary grow, and a future `reqwest` swap would reintroduce `aws-lc-rs` and
  break the Docker build
- an unset or invalid DSN is silent apart from one warning, so a typo in the secret looks like a
  quiet system
- the global `tracing` subscriber can be initialized only once, so the integration point is not
  unit-tested

Neutral:

- `Cargo.lock` lists `aws-lc-rs` and `aws-lc-sys` as unactivated optional dependencies of `rustls`,
  because Cargo resolves optional dependencies for all targets; they are never compiled, and the
  compiled dependency graph contains no `aws-lc` and no `native-tls`
- the binary has no compile-time switch for Sentry, only runtime configuration
- `RUST_LOG` no longer decides what is reported, only what is printed

## Alternatives considered

`reqwest` with `rustls`:

- async, and already in the tree through teloxide, but it enables `aws-lc-rs`, a CMake-built C
  dependency, and pulls a second `reqwest` next to teloxide's; not worth it for a low-traffic bot.

`native-tls`, sentry's default transport:

- the least configuration, but it links the platform TLS stack, requiring `libssl-dev` and
  `pkg-config` in the builder and `libssl` at runtime, which reverses ADR 0003.

A Prometheus endpoint scraped by something else:

- the standard for metrics, but it needs inbound ingress, a scraper, and a certificate, all of which
  ADR 0003 rules out; the bot's traffic is far too small to justify it.

Self-hosted Sentry or GlitchTip:

- speaks the same protocol and works with the same DSN and code, so it stays available if data must
  live on owned infrastructure; not chosen because it adds a service to operate.

Structured logs only:

- no new dependency, but nobody reads the logs, and there is no alerting or aggregation, so it does
  not satisfy "report all errors".

## Revisit

Revisit when tracing or performance spans are wanted, when log volume justifies sending logs through
`LogLayer`, or when error volume outgrows the free quota and sampling becomes necessary.

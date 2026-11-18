# ADR 0014: Resolve configuration into a typed Settings

- Status: accepted
- Date: 2026-11-18
- Related: amends ADR 0002; ADR 0004 owns the Sentry exception.

## Context

Configuration was resolved wherever a value was consumed: the transcriber read its own whitelist,
the OpenRouter client read its own model, and `start` read the token, the key, and the database path.
Each reader also decided what an unusable optional value meant. `TRANSCRIBE_MODEL_NAME` logged a
warning and used its default when its file could not be read, `TRANSCRIBE_CHATS` failed startup for a
non-numeric entry, and the two required secrets failed startup for absence. Nothing named the rule,
and a typo in one optional file silently disabled transcription while the process looked healthy.

## Decision

Read every variable once, in `src/infra/config.rs`, into one `Settings` value, validate it there, and
inject typed values into the clients and the services. `Settings::load` is the only place the
environment is read; the resolver behind it stays private, and `src/shared/config.rs` keeps only the
vocabulary.

One failure policy covers every field:

- a required value that is absent or blank fails startup
- an optional value that is absent or blank yields its documented default
- a value that is present but unusable (an unreadable or non-UTF-8 file, a non-Unicode variable, a
  non-numeric chat id) fails startup, for required and optional values alike
- anything wrong with `SENTRY_DSN` only disables reporting, after one warning, because ADR 0004 keeps
  observability from stopping the bot

The Sentry client takes its destination as a value, so it stops reading the environment. The
transcription model default moves beside the settings, because it is a configuration decision rather
than a client one. `Settings` and the DSN type hold secrets and implement no formatting traits, so a
secret cannot reach a log through `{:?}`. A configuration failure happens before the `tracing`
subscriber exists, so startup re-resolves the DSN once on that already-failing path so a usable
destination still receives the error.

## Consequences

- no module outside `src/infra/config.rs` reads the environment, and the complete set of settings is
  visible in one struct
- a broken optional file now fails startup instead of silently falling back, which is the behavior
  change: a configured but unreadable value means the operator intended something the bot cannot
  honor
- the model default lives with the configuration, and the OpenRouter client only guards against an
  empty value passed by a direct constructor
- `Settings` and the DSN type implement no `Debug`, `Display`, or `Clone`, so tests and handlers must
  match on fields instead of printing the value
- startup configuration failures are reported best effort: the DSN is read twice on that path, and
  without a usable DSN the failure reaches only the console after the subscriber is installed
- the tests exercise parsing through an injected lookup and never touch the process environment

## Alternatives considered

- Keep `Settings` in `src/shared/config.rs`: the loader would either live in `shared`, which would
  then name a layer above it, or be an `impl` split across modules.
- Keep per-module resolution: the status quo, with the inconsistency this ADR removes.
- An optional value never fails startup; an unusable one warns and uses its default: it would
  silently disable transcription on a typo, and the observability exception would become
  indistinguishable from the rule.
- `Settings.sentry_dsn: Option<String>` with an optional argument to the client: an unreadable
  `SENTRY_DSN_FILE` would be downgraded to `None` silently, which is the fallback this decision
  rejects.

# ADR 0002: Load secrets from the environment

- Status: accepted
- Date: 2026-11-17

## Context

u502_bot needs a Telegram bot token at runtime. The token must never be committed, baked into the
image, or written to logs. Two deployment styles must work: local `cargo run` invocations, where
exporting a variable is convenient, and the container, where Docker Compose and orchestrators
commonly inject configuration either as a plain environment variable or as a path to a mounted
secret file.

The project keeps its dependency list deliberately small: `clap`, `tracing`, and
`tracing-subscriber`, with no configuration crate yet. The bot loop does not exist, so for now
`u502 start` only loads and prints the token to demonstrate the loader.

## Decision

Read secrets through `crate::infra::config::get_secret(name)`, which resolves a variable in this
order:

- `<NAME>_FILE`: when set, its value is a filesystem path and the secret is that file's contents.
- `<NAME>`: the secret value itself.

`<NAME>_FILE` wins when both are set, so a mounted secret stays authoritative and a stale direct
variable cannot silently override it. File contents have one trailing `\n` or `\r\n` removed,
because files written by shells and editors usually end in a newline; direct environment values are
returned verbatim.

Failures are reported through a typed `SecretError` with `Missing`, `NotUnicode`, and `ReadFile`
variants implementing `Display` and `std::error::Error`, instead of an `Option`, a panic, or a
dependency such as `anyhow`. The CLI logs the error with `tracing::error!` and exits non-zero so
`supervisord` restarts the container.

`u502 start` prints the loaded token to stdout as temporary scaffolding for issue #2. That print is
never routed through `tracing` and must be removed when the bot loop consumes the token.

## Consequences

Positive:

- the same binary works with exported variables and with mounted secret files, without extra
  tooling or image changes
- no secret value reaches the logs; only the error is recorded
- the resolver is a pure helper with injected lookups, so the unit tests cover precedence, trimming,
  and error mapping without mutating `std::env`
- no new dependency, keeping the image and the audit surface small

Negative:

- a stray empty `<NAME>_FILE=""` is a read error instead of a silent fallback to `<NAME>`; this
  surfaces misconfiguration but can surprise operators
- the real `std::env` and `std::fs` wiring is not unit-tested; it is a three-line wrapper covered by
  manual smoke tests
- printing the token puts it in stdout and CI logs until the bot loop removes the scaffolding
- `read_to_string` reads the whole mounted file; acceptable because secret files are small and
  operator-controlled

Neutral:

- `supervisord` sees the non-zero exit as a crash and restarts the bot, so a missing secret shows up
  as a restart loop rather than a hung process

## Alternatives considered

Return `Option<String>`:

- simpler signature, but it collapses a missing variable, a non-Unicode value, and an unreadable
  file into one `None`, leaving the CLI unable to explain the failure.

Let the direct variable win over `<NAME>_FILE`:

- avoids surprising operators who export a variable while a file is mounted, but lets a stale
  environment value shadow the intended mounted secret.

Add `dotenvy` or a configuration crate:

- provides `.env` loading and layered configuration, but adds dependencies for a single token and
  duplicates what supervisors and Compose already do.

`unwrap()` or panics on a missing secret:

- shortest code, but violates the project rule against `unwrap()` in runtime paths and hides which
  variable is missing.

## Validation

Verify manually until the bot loop lands:

- `TELEGRAM_BOT_API=dummy cargo run -- start` prints `dummy`
- `TELEGRAM_BOT_API_FILE=/tmp/opencode/telegram_bot_api cargo run -- start` prints the file contents
  without the trailing newline
- `env -u TELEGRAM_BOT_API -u TELEGRAM_BOT_API_FILE cargo run -- start` exits non-zero and logs the
  missing secret

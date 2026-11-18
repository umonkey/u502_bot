# ADR 0002: Load secrets from the environment

- Status: accepted
- Date: 2026-11-17

## Context

The bot needs a bot token at runtime, and the token must never be committed, baked into the image, or
logged. Two deployment styles must work: a local run, where exporting a variable is convenient, and a
container, where the platform injects configuration either as a value or as a mounted secret file.

## Decision

Read each secret through one helper that accepts a direct value or a file path, and returns the
file's contents when both are present. The file wins, so a mounted secret cannot be silently
overridden by a stale value, and one trailing newline is removed because files usually end with one.
Direct values are returned verbatim.

A missing secret is a typed error that names the failure, never a panic or an empty value, and the
command exits non-zero so the supervisor restarts the process. No secret value is logged.

## Consequences

- the same binary works with exported values and mounted files, with no extra tooling
- a secret never reaches the logs, and a missing one fails loudly at startup
- a set-but-empty file path is an error rather than a fallback, which surfaces misconfiguration
- the helper is tested without touching the real environment; the thin wiring around it is not

## Alternatives considered

- Return an optional value: simpler, but it collapses a missing value, an unreadable file, and an
  encoding error into one indistinguishable absence.
- Let the direct value win: avoids surprising an operator who exports over a mount, but lets a stale
  value shadow the intended secret.
- Add a configuration library: layered loading for one token, and a dependency the platform already
  covers.
- Panic when the secret is missing: shortest code, but it hides which secret is absent.

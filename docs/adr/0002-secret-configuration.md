# ADR 0002: Read configuration from the environment or a file

- Status: accepted
- Date: 2026-11-17
- Amended: 2026-11-18, so the direct-or-file rule covers every configuration variable, not only secrets

## Context

The bot needs a bot token at runtime, and the token must never be committed, baked into the image, or
logged. Two deployment styles must work: a local run, where exporting a variable is convenient, and a
container, where the platform injects configuration either as a value or as a mounted file.

The same need applies to the rest of the configuration: the database path, the chat whitelist, the
model id, and the ffmpeg binary. Giving only the secrets a file form forced a per-variable rule onto
each of the others; four different blank checks accumulated, and the documentation had to explain why
one variable resolved differently from another.

## Decision

Resolve every configuration variable through one algorithm: `<NAME>_FILE` first, then `<NAME>`. The
file wins, so a mounted value cannot be silently overridden by a stale direct one. The resolved value
is trimmed of surrounding whitespace, and a value that is blank after trimming counts as unset.

Values are UTF-8. A non-Unicode variable, an unreadable file, and a file that is not UTF-8 are typed
errors that name the variable or the path, never a panic and never an empty value.

`VariableName` names every variable, `get_optional` is the resolver, and
`require_secret` turns absence into an error for the variables startup cannot run without, so a
missing required value exits non-zero and the supervisor restarts the process. No secret value is
logged.

`RUST_LOG` is the exception: `tracing_subscriber`'s `EnvFilter` reads it, not this crate, so it has no
file form.

## Consequences

- one resolver, one trim, one interpretation of blank, and one error type serve every variable; the
  same binary works with exported values and mounted files, with no extra tooling
- a secret never reaches the logs, and a missing required value fails loudly at startup
- any variable can be mounted; a leftover `<NAME>_FILE` in a shared environment overrides the direct
  value for every variable, not only the secrets
- paths with meaningful surrounding whitespace are no longer representable, so an operator has to
  encode around the trimming
- a non-UTF-8 `FFMPEG_PATH` logs one warning and falls back to the `ffmpeg` on `PATH`, which may be a
  different binary than intended
- `<NAME>_FILE` no longer marks a value as sensitive, so the guidance to prefer the file form for the
  token, the key, and the DSN lives in `docs/features/configuration/README.md`; the environment is
  rendered by `docker inspect` and inherited by child processes such as `ffmpeg`, while a mounted file
  is neither
- the resolver is tested without touching the real environment; the thin wiring around it is not

## Alternatives considered

- Give only the secrets a file form: narrower, but it forces two code paths, two trim rules, and a
  permanent explanation for each excluded variable. This was the original decision.
- Return an optional value: simpler, but it collapses a missing value, an unreadable file, and an
  encoding error into one indistinguishable absence.
- Let the direct value win: avoids surprising an operator who exports over a mount, but lets a stale
  value shadow the intended configuration.
- Add a configuration library: layered loading for a handful of variables, and a dependency the
  platform already covers.
- Panic when a required value is missing: shortest code, but it hides which variable is absent.
- Read every variable as an operating-system string: preserves unusual paths, but pushes byte handling
  into every consumer and makes a file and a direct value behave differently.
- Return raw untrimmed values and let each caller trim: closest to the previous behavior, but that is
  exactly the four-policy situation this decision removes.

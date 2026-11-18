# ADR 0012: Use one application error type

- Status: accepted
- Date: 2026-11-18

## Context

Nine modules that can fail each defined their own error type, with the same message and conversion
boilerplate written once per module. Nothing in the tree shows the complete set of ways the bot can
fail, and a caller that wants to act on one failure has to know which module produced it. Every new
failure repeats the boilerplate and adds another type to learn.

## Decision

Define one application error type and one result type for the whole program, and return them from
every fallible function. Each entry names one failure mode and keeps its underlying cause, so a
failure keeps its identity as it crosses layers. Library failures with one obvious meaning are
converted once; failures that need context, such as input and output errors, are translated where
they happen, so the message names what failed. No dependency is added for this.

A transport failure that no caller acts on is carried by `External`, whose `operation` is an
`Operation` value naming what was being done and whose `source` keeps the cause, so no caller or test
compares text. A failure a caller must act on gets its own variant even when it is specific to one
integration; the current set is `HttpStatus`, `ReadAudioFile`, `MessageIdOutOfRange`,
`FfmpegUnavailable`, `FfmpegStatus`, `FfmpegIo`, `NoAudioStream`, and `FfmpegTimeout`. Configuration
failures carry the `VariableName` that failed, and the key text is rendered from that enum. Each
`From` conversion lives in the module that owns the client producing it.

## Consequences

- every fallible signature reads the same, and no module exports an error type of its own
- adding a failure is one entry and one message in one place
- a caller can name and act on the semantic failures, which are the ones that drive behaviour; a
  transport failure is one `External` variant
- `External::operation` is a closed `Operation` enum, so a caller and a test tell one client failure
  from another by matching a variant, and the rendered message comes from the same variant, so the
  two cannot drift
- configuration failures carry `VariableName`, so a test asserts the variable by matching the variant
  and the key text is rendered from the enum
- the shared enum names no integration crate, so a dependency upgrade does not force a rename, but a
  new failure mode does add a variant
- adding an integration puts its conversions beside the client and adds its semantic failures to the
  shared list, so the list grows with each integration
- the shared set grows with the program and needs curating, because it is one flat list
- a wrapped library failure hides the cases a library upgrade adds until code matches on them
- the command boundary is the only place a failure becomes a process exit code: every fallible
  function returns the shared result type, and the command reports one failure and maps it to an
  unsuccessful exit

## Alternatives considered

- A derive macro library: removes the hand-written boilerplate, but adds a dependency for one type.
- Keeping the module types behind one outer type: keeps each module's vocabulary, but keeps the
  boilerplate and the imports this decision removes.
- An untyped or boxed error: the least code, but naming and discovering failures is what this
  decision is for.

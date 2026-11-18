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

A failure of an external client is carried by one `External` variant, whose `context` names the
operation that failed and whose `source` keeps the cause, so the shared enum names no integration
crate. Each `From` conversion lives in the module that owns the client producing it.

## Consequences

- every fallible signature reads the same, and no module exports an error type of its own
- adding a failure is one entry and one message in one place
- a caller can name and act on the semantic failures, which are the ones that drive behaviour; a
  client failure is one `External` variant, matched on its context
- the shared enum names no integration crate, so it does not change when a client crate does
- adding an integration is a conversion beside the client rather than a variant in the shared list
- the shared set grows with the program and needs curating, because it is one flat list
- a wrapped library failure hides the cases a library upgrade adds until code matches on them

## Alternatives considered

- A derive macro library: removes the hand-written boilerplate, but adds a dependency for one type.
- Keeping the module types behind one outer type: keeps each module's vocabulary, but keeps the
  boilerplate and the imports this decision removes.
- An untyped or boxed error: the least code, but naming and discovering failures is what this
  decision is for.

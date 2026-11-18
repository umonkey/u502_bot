---
name: adr
description: Use when creating or editing an architecture decision record under docs/adr/.
---

# Architecture decision records

An ADR answers two questions: what changed architecturally, and why it was
needed. It records the forces at play, the choice made, and the consequences
accepted, at a level a future reader can act on without reading the code. One
decision per file, named `NNNN-short-slug.md`.

## Format

```markdown
# ADR-NNNN: Short imperative title

- Status: proposed | accepted | superseded | deprecated
- Date: YYYY-MM-DD
```

Use exactly these two metadata lines and no index or extra keys such as
`Deciders`, `Related`, or `Tags`. An ADR must not reference another ADR, by
number or title, because each decision must stand on its own; the only
reference allowed is a supersession, expressed in `Status` as
`superseded by NNNN-slug.md` and explained in the new ADR's Context.

## Sections

- `## Context`: the forces at play and why a decision is needed now.
- `## Decision`: what we will do, stated plainly.
- `## Consequences`: what is affected, what becomes easier or harder, and what
  we accept.
- `## Alternatives considered`: what was rejected and why.
- `## Revisit` (optional): the trigger that should reopen the decision.

## Shape

Aim for well under a page: one short paragraph per narrative section and no
more than a few bullets. A complete ADR is often this small:

```text
Context: one paragraph on the forces and why now.
Decision: one paragraph on the choice.
Consequences: a handful of plain bullets.
Alternatives: one line each.
```

- State the decision once, in `## Decision`; do not restate it in Context or
  rephrase it as a benefit in Consequences.
- Consequences add only what the reader cannot infer, as plain bullets rather
  than positive, negative, and neutral groupings.
- One line per alternative: what it was and why it was rejected.
- Omit any section that carries no information; `Revisit` stays only when it
  names a real trigger.
- Prefer deleting a sentence to shortening it.

## Leave out implementation

Write in business language for a reader who was not in the discussion. Do not
include:

- code snippets, symbols, functions, types, constants, enum values, modules, or
  file paths;
- products, libraries, versions, and feature flags;
- schemas, queries, tables, columns, and indexes;
- endpoints, interfaces, and the modules that own them;
- configuration keys, log and metric names, status codes, thresholds, formats,
  and field names;
- issue, ticket, or pull-request identifiers, and the incident that prompted the
  record;
- worked examples and step-by-step procedures.

Describe the shape of the problem, not the incident that revealed it. If a
sentence needs a symbol or a number to make its point, it describes a design;
put that detail in feature docs, reference docs, or the change description.

## Checklist before finishing

- Context names the forces, not the decision.
- The decision appears once.
- Every consequence follows from the decision and is not restated elsewhere.
- Every alternative fits on one line.
- Deleting any sentence would lose meaning.
- A reader who missed the discussion can act on it.

## Rules

- Supersede, do not rewrite history. Add a new ADR and update the old Status.
- Do not cross-reference another ADR: a decision stands on its own, and the
  only reference allowed is the supersession link that `Status` records.
- One decision per file; split unrelated choices into separate ADRs.
- Follow the project markdown style: minimal bold, no numbered headings,
  lowercase text after colons in list items, and blank lines around lists.
- Run `make format-docs` after editing files under `docs/`.

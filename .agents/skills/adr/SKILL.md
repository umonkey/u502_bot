---
name: adr
description: Use when creating or editing an architecture decision record under docs/adr/.
---

# Architecture decision records

An ADR records why a decision was made and what it affects: the forces at play,
the choice made, and the consequences accepted. It explains why the system is
the way it is, not how to build it. One decision per file, named
`NNNN-short-slug.md`.

## Format

```markdown
# ADR-NNNN: Short imperative title

- Status: proposed | accepted | superseded | deprecated
- Date: YYYY-MM-DD
```

Use exactly these two metadata lines and no index or extra keys such as
`Deciders`, `Related`, or `Tags`. Cross-references belong in the body.
Supersession is expressed in `Status` as `superseded by NNNN-slug.md` and
explained in the new ADR's Context.

## Sections

- `## Context`: the forces at play and why a decision is needed now.
- `## Decision`: what we will do, stated plainly.
- `## Consequences`: what is affected, what becomes easier or harder, and what
  we accept.
- `## Alternatives considered`: what was rejected and why.
- `## Revisit` (optional): the trigger that should reopen the decision.

## Keep it high level

Document why the decision was made and what it affects. Write for a future
reader who needs the reasoning, not the implementation. An ADR answers two
questions only: what changed architecturally, and why it was needed. State the
decision and its trade-offs, then stop.

Leave out implementation detail and incident-specific facts:

- code snippets, function and type names, constants, enum values, and source
  module or file paths;
- configuration keys, log event names, tag or metric names, status codes,
  thresholds, formats, and field names;
- incident, issue, ticket, or pull-request identifiers, and the specific
  reproduction that prompted the record;
- worked examples, alert-rule settings, and step-by-step procedures.

Describe the class of failure or the shape of the problem, not the incident that
revealed it. Put the concrete names, values, and reproduction in the feature
docs, reference docs, code, or the change description, and point to them when
the detail matters. If the file needs a symbol or a number to make its point, it
is describing a design rather than a decision. If it runs past about a page, it
is probably a design too.

## Rules

- Supersede, do not rewrite history. Add a new ADR and update the old Status.
- One decision per file; split unrelated choices into separate ADRs.
- Follow the project markdown style: minimal bold, no numbered headings,
  lowercase text after colons in list items, and blank lines around lists.
- Run `make format-docs` after editing files under `docs/`.

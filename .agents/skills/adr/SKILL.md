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

## Be concise

An ADR is read to learn why something is the way it is, not to review the
discussion. Aim for well under a page: one short paragraph per narrative
section, and no more than a few bullets.

- State the decision once, in `## Decision`; do not restate it in Context
  or rephrase it as a benefit in Consequences.
- Keep only the trade-offs a reader would act on, and use plain bullets
  rather than positive, negative, and neutral groupings unless that split
  carries meaning.
- One line per alternative: what it was and why it was rejected.
- Omit any section that carries no information; `Revisit` stays only when it
  names a real trigger.
- Prefer deleting a sentence to shortening it, and drop any paragraph that
  restates an earlier one.

## Keep it high level

An ADR records the problem, the forces at play, the choice made, the trade-offs
accepted, and the impact, in business language. Document why the decision was
made and what it affects. Write for a future reader who needs the reasoning, not
the implementation. An ADR answers two questions only: what changed
architecturally, and why it was needed. State the decision and its trade-offs,
then stop.

Leave out implementation detail and incident-specific facts:

- code snippets, function and type names, constants, enum values, and source
  module or file paths;
- libraries, drivers, and storage engines named as products, with the versions
  or feature choices that selected them;
- SQL statements, schema definitions, and table, column, or index details;
- API and module surfaces: endpoints, public interfaces, and the modules that
  own them;
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

Finally, check the record against a reader who was not in the discussion: if
they could not act on the decision without reading the implementation, the ADR
is too detailed.

## Rules

- Supersede, do not rewrite history. Add a new ADR and update the old Status.
- Do not cross-reference another ADR: a decision stands on its own, and the
  only reference allowed is the supersession link that `Status` records.
- One decision per file; split unrelated choices into separate ADRs.
- Follow the project markdown style: minimal bold, no numbered headings,
  lowercase text after colons in list items, and blank lines around lists.
- Run `make format-docs` after editing files under `docs/`.

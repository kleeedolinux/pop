# Architecture Decision Records

Use one numbered Markdown file per accepted or rejected architectural decision:

```text
0001-short-decision-title.md
```

Each record contains:

```markdown
# ADR 0001: Title

- Status: proposed | accepted | superseded | rejected
- Date: YYYY-MM-DD
- Supersedes: none | ADR numbers

## Context

## Decision

## Consequences

## Alternatives considered

## Required conformance tests

## Documents/components affected
```

ADRs describe why a decision exists. The language specification and compiler
documentation remain the authoritative descriptions of current behavior.

Accepted architecture is binding. A conflicting implementation is a bug until a
new ADR supersedes the decision and updates dependent architecture/spec/tests.

# ADR 0021: Exhaustive Tagged-Union Match Statements

- Status: accepted
- Date: 2026-07-10
- Supersedes: none

## Context

Tagged unions are already native typed values, and Milestone 3 requires
exhaustive matching plus missing-case fixes. The architecture did not yet fix a
minimal Luau-shaped pattern surface or its first-version exhaustiveness rules.

## Decision

The initial construct is a statement with `match`, `when`, `then`, and `end`:

```luau
match result
when Result.Ok(value) then
    use(value)
when Result.Error(message) then
    report(message)
end
```

The scrutinee must have one statically known tagged-union type. Every arm names
a case of that union through resolved syntax and binds exactly its typed payload
arity. `_` may ignore one payload binding. Bindings are arm-local and immutable
unless ordinary typed local assignment applies.

Version one has no wildcard case, guard, alternative pattern, nested pattern,
or match expression. Every declared case must appear exactly once. Missing and
duplicate cases are errors; source order controls evaluation only after the
scrutinee is evaluated once.

HIR stores the resolved union and `UnionCaseId` for every arm. MIR lowers the
construct to one discriminant switch and typed payload projections into arm
block arguments. Neither representation performs name lookup or reconstructs
patterns at runtime.

The missing-case diagnostic carries resolved union/case identities and offers a
safe edit that inserts canonical empty `when` arms before the matching `end`.

## Consequences

- Exhaustiveness remains simple, deterministic, and statically checkable.
- Richer patterns or expression-valued matches can be added later without
  weakening this contract.
- Adding a public union case is an exhaustiveness-breaking API change.

## Alternatives considered

### Wildcard arms in version one

Rejected because they hide newly added cases and weaken the required missing-
case fix contract.

### Arrow-heavy expression syntax

Rejected because the first surface should preserve Luau's block/`then`/`end`
character.

### Runtime tag-name matching

Rejected as dynamic lookup and backend drift.

## Required conformance tests

- exhaustive matches with empty and payload cases;
- scrutinee-once evaluation and source-ordered selected-arm execution;
- missing, duplicate, foreign-case, wrong-payload, and scope diagnostics;
- exact safe missing-case edit and fix-all composition;
- HIR/MIR verifier rejection of incomplete or mistyped case tables;
- interpreter/optimized-MIR differential tests and no runtime string matching.

## Documents/components affected

Syntax, resolver, type checker, diagnostics/fixes, HIR, MIR, interpreter,
standard `Result` conformance, and backend differential tests.

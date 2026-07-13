# ADR 0048: Erased Type Aliases

- Status: Accepted
- Date: 2026-07-13

## Context

Pop Lang reserves namespace `type` declarations and documents type aliases as
ordinary namespace Items, but the bootstrap front end only indexes their names.
Signatures and bodies therefore cannot use an alias even though aliases require
no runtime representation.

Aliases must not become nominal wrapper types, runtime reflection entries, or
string-based type resolution. They also must not conceal a recursive type whose
representation has not been designed.

## Decision

The initial alias form is:

```luau
public type PlayerId = Guid
```

It has no type parameters. Its name follows normal namespace visibility and
type-space resolution. Resolving the name recursively resolves and substitutes
the target semantic type before HIR construction. Consequently an alias has
exactly the target's operations, layout, conversions, and runtime identity; no
alias node or operation reaches HIR/MIR.

Alias chains are permitted. Direct or indirect cycles are rejected
deterministically. Supplying type arguments to a non-generic alias is rejected.
Generic aliases remain deferred until generic declaration substitution is
complete.

## Consequences

- Aliases improve source readability with zero runtime or backend machinery.
- Records, unions, classes, interfaces, arrays, tables, tuples, optionals, and
  function types retain their distinct target semantics through an alias.
- Changing an alias target is a source/API contract change even though no new
  runtime type exists.

## Conformance requirements

Tests must cover primitive and compound targets, alias chains, use in function
signatures and bodies, visibility, wrong type arity, cycle rejection, and the
absence of alias-specific HIR/MIR operations.

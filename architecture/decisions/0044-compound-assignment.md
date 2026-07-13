# ADR 0044: Typed Compound Assignment

- Status: accepted
- Date: 2026-07-13
- Depends on: ADR 0019, ADR 0034, ADR 0040, and ADR 0041

## Context

Pop Lang's Luau relationship adopts compound assignment, but the architecture
does not define its operator set, typing, target evaluation, or lowering. A
textual rewrite such as `target += value` to `target = target + value` would
evaluate a field receiver or array/index expression twice. That would change
calls, mutation, traps, and allocation and would leave backends free to disagree.

## Decision

Pop Lang accepts the compound assignment operators whose underlying operations
are already part of the language:

```luau
total += amount
remaining -= consumed
scale *= factor
ratio /= divisor
offset %= width
message ..= suffix
```

`+=`, `-=`, `*=`, and `/=` require identical statically known numeric target and
right-hand-side types. `%=` additionally requires an integer type. `..=` requires
`String` on both sides. The operation uses the same checked arithmetic, division,
remainder, concatenation, allocation, failure, and result semantics as the
corresponding ordinary binary operator. Compound assignment never performs an
implicit numeric conversion or introduces a dynamic fallback.

The target must be assignable by the ordinary assignment rules: a mutable local
or capture, a mutable declared class field, or an indexed mutable array element.
Parameters, numeric `for` bindings, record fields, ordinary values, optional
array reads, and other immutable targets remain rejected.

Evaluation proceeds in this exact order:

1. evaluate the target receiver or array expression once;
2. for an indexed target, evaluate the index once;
3. load the current target value once;
4. evaluate the right-hand side once;
5. apply the corresponding typed binary operation;
6. store the result only if every preceding step succeeds.

An indexed compound assignment uses the array's non-optional element type and a
checked current-element load. An out-of-bounds index therefore traps with
`BoundsViolation` before the right-hand side executes and never grows the array.

`^=`, `//=`, bitwise compound forms, logical compound forms, increment/decrement,
and user-defined operator dispatch are not introduced because Pop Lang does not
yet define their underlying ordinary operators. They require their own accepted
contracts before compound spellings can exist.

Typed analysis and HIR retain the resolved target identity, operator, target
type, and right-hand side long enough to preserve single evaluation. Canonical
MIR lowers compound assignment to existing backend-neutral loads, typed binary
operations, traps/effects, barriers, and stores. MIR gains no compound-assignment
opcode, and backends never reconstruct this source-level rule.

## Consequences

- Common mutation remains concise and familiar to Luau programmers.
- Effectful receivers and indexes have deterministic single-evaluation behavior.
- Arithmetic overflow, division failure, bounds failure, string allocation, and
  managed-reference barriers remain visible in canonical MIR.
- The feature does not broaden assignment targets or operator overloading.

## Alternatives considered

### Rewrite the syntax tree textually

Rejected because it duplicates receiver/index evaluation and loses the source
target identity before typing can enforce mutability.

### Add a generic compound operation to MIR

Rejected because MIR already has the exact typed load, operation, and store
vocabulary. A source-sugar opcode would make optimizers and backends duplicate
front-end semantics.

### Add every Luau compound spelling immediately

Rejected because a compound spelling cannot define an underlying arithmetic or
bitwise operation by implication. Pop Lang grows the ordinary operator contract
first and derives compound forms from it deliberately.

## Required conformance tests

- lexer/parser acceptance for every supported spelling and rejection of
  unsupported compound spellings;
- positive typing for every numeric kind and `String`, with negative tests for
  unlike types, `%=` on floats, invalid operators, and immutable targets;
- exact once-only target, index, current-value, and right-hand-side evaluation;
- checked overflow, division, remainder, bounds, concatenation, allocation, and
  barrier behavior identical to the underlying ordinary operation;
- capture-cell mutation and nested closure visibility;
- HIR preservation, MIR verifier/text round trips, and absence of a compound
  opcode;
- construction/optimized MIR interpreter and LLVM differential behavior, with
  the supported C slice agreeing or rejecting an explicit capability.

## Documents/components affected

Language model, compiler pipeline, intermediate representations, roadmap,
closed decisions, Luau relationship, syntax/nomenclature, conformance matrix,
lexer/parser, typed statements, capture analysis, HIR, MIR lowering, interpreter,
LLVM, C capability tests, and formatter/tooling token handling.

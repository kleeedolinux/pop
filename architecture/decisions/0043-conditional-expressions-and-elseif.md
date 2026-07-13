# ADR 0043: Conditional Expressions and Elseif Chains

- Status: accepted
- Date: 2026-07-13
- Supersedes: none

## Context

Pop Lang's Luau feature inventory adopts if-expressions, but the integrated
architecture only specifies statement `if` with an optional `else`. Leaving the
expression form undefined would force the parser, type checker, and backends to
invent branch typing, evaluation, and representation independently.

## Decision

Pop Lang adds Luau's conditional expression:

```luau
local label = if ready then "ready" else "waiting"
```

The condition has exactly type `Boolean`. The `then` and `else` expressions
have one identical result type after the ordinary statically resolved implicit
conversions already accepted by the type system. There is no truthiness,
numeric-to-Boolean conversion, dynamic common type, or implicit union creation.
An expected type from a return, local annotation, argument, or enclosing
expression is checked against both branches.

The condition executes first and exactly one branch executes. Conditional
expressions associate through their explicit keywords, so the `else` branch may
be another conditional expression without parentheses. The expression always
requires `else`; it has no `end` because each branch is one expression.

Statement conditionals additionally accept the single Luau keyword `elseif`:

```luau
if first then
    useFirst()
elseif second then
    useSecond()
else
    useFallback()
end
```

Each condition is exactly `Boolean`, conditions execute from left to right only
until one succeeds, and every arm has its own lexical scope. One final `end`
closes the complete chain. `elseif` is not spelled `else if` in canonical Pop
source.

HIR preserves a typed conditional expression with its condition and both
branches. Canonical MIR lowers it to conditional control flow and one typed join
block argument. It adds no conditional-value opcode and backends do not
reconstruct source typing. Compile-time evaluation uses the same lazy branch
semantics within its existing deterministic budgets.

## Consequences

- Common conditional values remain compact and Luau-shaped.
- Branch laziness is shared by compile-time evaluation, the MIR interpreter,
  LLVM, and future VM behavior.
- Exact branch typing prevents conditional expressions from becoming a dynamic
  or implicit-union escape hatch.

## Alternatives considered

### Add `condition ? first : second`

Rejected because it is JavaScript/C-shaped punctuation and contradicts the
accepted Luau-first surface.

### Infer a union from unlike branches

Rejected for the initial form because it silently broadens types and complicates
overload and exhaustiveness behavior. Programs can construct an explicit tagged
union when alternatives are semantically different.

### Lower eagerly to a select operation

Rejected because eager evaluation changes effects, traps, allocation, and call
behavior. CFG lowering makes the lazy contract explicit.

## Required conformance tests

- parser acceptance, nesting/association, required `else`, `elseif` chains, and
  rejection of `?:` syntax;
- exact Boolean condition and common branch typing with no truthiness, dynamic
  fallback, or implicit union;
- branch laziness, left-to-right `elseif` evaluation, and lexical arm scopes;
- deterministic compile-time conditional evaluation and budget accounting;
- HIR preservation plus MIR typed join arguments, verifier checks, textual
  round trips, and absence of a conditional-value opcode;
- MIR interpreter and LLVM differential behavior, including a trapping or
  effectful unselected branch that must not execute.

## Documents/components affected

Language model, intermediate representations, roadmap, closed decisions, Luau
inventory, syntax and nomenclature, conformance matrix, syntax, type checking,
compile-time lowering, HIR, MIR lowering, optimizer, and backend tests.

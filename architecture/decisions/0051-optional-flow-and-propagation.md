# ADR 0051: Optional Flow and Propagation

- Status: accepted
- Date: 2026-07-13
- Supersedes: none

## Context

Pop Lang already accepts `nil` only through `T?`/`T | nil`, requires
flow-sensitive narrowing, and uses optional results for ordinary missing
collection entries and checked downcasts. The architecture did not yet fix the
source forms or portable lowering for binding, defaulting, or propagating an
optional value.

The missing contract cannot be filled independently by the parser, type
checker, or a backend. Doing so could accidentally use truthiness, evaluate a
fallback twice, keep a stale narrowing fact after mutation, or turn absence
into a dynamic operation. The initial surface must remain light and
Luau-shaped while preserving an exact backend-neutral meaning.

## Decision

An equality comparison between a stable optional place and `nil` narrows the
place on each reachable edge. `value ~= nil` narrows `value` to its non-`nil`
type on the true edge; `value == nil` narrows it on the false edge. Facts are
tied to the resolved versioned place and obey the invalidation rules already
accepted for writes, aliases, captures, calls, FFI/unsafe operations, and
suspension.

Pop Lang adds optional pattern binding to `if` and `while`:

```luau
if local player = findPlayer(id) then
    show(player)
else
    showMissing(id)
end

while local message = inbox.next() do
    handle(message)
end
```

The initializer must have type `T?`. It is evaluated exactly once per test.
Presence, not Boolean truthiness, selects the body, so a present `false` or zero
still enters it. The immutable binding has type `T`, exists only in the body,
and is created afresh for each `while` iteration. The `else` branch cannot see
the binding.

The right-associative `??` operator lazily supplies an optional default:

```luau
local port = configuredPort ?? DEFAULT_PORT
```

For a left operand of type `T?`, the right operand must be assignable to `T`
and the expression has type `T`. The left operand is evaluated once. The right
operand is evaluated only when the left operand is `nil`. `??` binds more
tightly than `or` and less tightly than `and`; parentheses remain available
when Boolean and optional control are mixed.

Postfix `?` propagates optional absence:

```luau
private function parentName(player: Player?): String?
    local present = player?
    return findParentName(present)
end
```

For an operand of type `T?`, `expression?` has type `T` on the continuing
edge. It is valid only inside a function with one optional result `U?`; `T` and
`U` need not be related because the early edge returns only `nil`. The operand
is evaluated once; `nil` returns `nil` from the enclosing function immediately.
It does not catch traps or panic, does not propagate `Result`, and does not
invoke a user overload. Result propagation is a separate error-model decision.

Typed HIR retains distinct optional comparison facts, optional binding,
optional default, and optional propagation nodes with the resolved inner and
enclosing result types. Canonical MIR lowers all four forms to explicit control
flow using typed `optionalIsPresent` and `optionalGet` operations. An
`optionalGet` is valid only on a control-flow path dominated by a matching
successful presence test. Default and propagation joins use typed block
arguments; propagation's absent edge contains an ordinary typed `return nil`.
No backend reconstructs optional source semantics or performs runtime type/name
lookup.

The native LLVM profile represents an optional as a private typed presence bit
plus payload. It must not use the payload's zero bits as the absence sentinel:
a present `false`, integer zero, or zero-valued enum remains present. Native
array and table lookup adapters therefore return presence separately from an
out payload. This advances the bootstrap stable-handle native ABI to version
1.7 without exposing the physical pair in HIR, MIR, PLRI values, or source.

## Consequences

- Optional control distinguishes absence from Boolean truthiness.
- Common lookup and downcast paths remain concise without adding unchecked
  unwraps or dynamic fallback.
- Lazy defaulting and early return have one deterministic CFG meaning across
  the MIR interpreter and LLVM.
- `?` in expression position is unambiguous with `T?` in type position.
- `Result` propagation, multi-result propagation, and user-defined carrier
  protocols remain unresolved and cannot reuse this contract implicitly.

## Alternatives considered

### Reuse `and`/`or` operand-returning truthiness

Rejected because Pop's `and`/`or` operators are Boolean and a present `false`
must not be confused with absence.

### Require a comparison and a second local declaration everywhere

Rejected because it duplicates evaluation for non-place expressions or forces
temporary ceremony around ordinary optional APIs.

### Add an unchecked unwrap operator

Rejected because absence would become a new runtime failure where static
control flow can express the requirement directly.

### Make postfix `?` propagate both `Option` and `Result` immediately

Rejected because the public `Result` identity, error conversion rule, cleanup
interaction, and documentation contract must be accepted with the typed-error
workflow rather than inferred from optional behavior.

## Required conformance tests

- lexer/parser tests cover `??`, postfix `?`, `if local`, `while local`,
  precedence, associativity, and recovery;
- type tests cover comparison narrowing, binding scope/type, lazy default type,
  optional-return compatibility, and present `Boolean` values;
- negative tests cover non-optional operands, mutable/stale places, binding use
  in `else`, propagation outside a function or into a non-optional result, and
  any attempted overload/dynamic fallback;
- HIR/MIR tests prove typed optional nodes, single evaluation, explicit
  presence branches, dominated `optionalGet`, lazy joins, and early `nil`
  return;
- MIR verifier negatives reject an undominated/mismatched `optionalGet`;
- MIR-interpreter and LLVM differential tests cover present, absent, false,
  integer zero, scalar collection lookup, nested default, and propagation
  paths; native ABI tests distinguish a missing entry from a present zero; the
  experimental C backend rejects unsupported optional operations
  deterministically.

## Documents/components affected

- `architecture/02-language-model.md`
- `architecture/04-intermediate-representations.md`
- `architecture/07-implementation-roadmap.md`
- `architecture/08.1-closed-design-questions.md`
- `architecture/12-type-system-architecture.md`
- `architecture/13-syntax-and-nomenclature.md`
- `architecture/19-architecture-conformance-and-regression-policy.md`
- syntax, typed bodies, HIR, MIR, verifier, interpreter, LLVM, C capability
  validation, native ABI/runtime, diagnostics, formatting, and differential
  tests
